//! # scope-mcp —— MCP Server（P3 阶段）
//!
//! 把示波器能力以 **~12 个粗粒度工具**暴露给 AI Agent。
//!
//! ## 当前状态：骨架已就位，工具实现待 P3
//!
//! 这个二进制现在做两件有用的事：
//! 1. `--list-tools` 打印工具清单与 JSON Schema（人工核对用）
//! 2. `--selftest` 走一遍模拟器，确认命令层可用
//!
//! ## 设计约束（为什么是"粗粒度"）
//!
//! 12 个工具 ≈ 12 个**Agent 意图**，而不是 30+ 个逐命令工具。
//! 后者会让工具列表爆炸，且一个意图要多次往返。
//!
//! ## 三层 token 防护（硬性，不可协商）
//!
//! 1. `scope_capture` / `scope_watch` 默认只回**统计量 + ≤256 点 minmax 预览**
//! 2. `scope_read_waveform` 分页、默认 512 点、**硬顶 4096**、超出拒绝并提示存盘
//! 3. **全量数据只进 capture store 和磁盘，永不进上下文**
//!
//! 一次 4096 点采集 = 8192 字节 ≈ 4096 个 JSON 数字 ≈ 上万 token。
//! 把它塞给 LLM 既贵又没用。

#![deny(clippy::all)]

use anyhow::Result;
use scope_core::{CaptureStore, CommandBus};
use scope_sim::{Scenario, SimDevice};

/// 一个 MCP 工具的声明。
struct ToolSpec {
    name: &'static str,
    /// 一句话说明 —— 这是 LLM 决定要不要调用它的主要依据。
    description: &'static str,
    /// 输入参数的 JSON Schema。
    input_schema: &'static str,
    /// 对应哪些协议命令。
    protocol_cmds: &'static str,
    /// 是否只在某些条件下注册。
    condition: &'static str,
}

/// 工具清单 —— 与 docs/03-protocol.md §MCP 保持一致。
const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "scope_list_devices",
        description: "列出可用的示波器设备（枚举串口并短超时探测 GET_INFO）",
        input_schema: r#"{"type":"object","properties":{},"required":[]}"#,
        protocol_cmds: "GET_INFO",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_connect",
        description: "连接设备。不指定 port 时使用模拟器",
        input_schema: r#"{"type":"object","properties":{"port":{"type":"string"},"baud":{"type":"integer"},"transport":{"type":"string","enum":["serial","sim"]},"sim_scenario":{"type":"string"}},"required":[]}"#,
        protocol_cmds: "GET_INFO + GET_CONFIG",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_disconnect",
        description: "断开连接并停止流",
        input_schema: r#"{"type":"object","properties":{},"required":[]}"#,
        protocol_cmds: "STOP",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_status",
        description: "设备当前状态、配置摘要、溢出与错误计数。任何状态可调",
        input_schema: r#"{"type":"object","properties":{},"required":[]}"#,
        protocol_cmds: "GET_STATUS",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_configure",
        description: "一次性配置采样率/采集/触发/通道。返回实际生效值与警告",
        input_schema: r#"{"type":"object","properties":{"sample_rate_hz":{"type":"integer"},"capture_samples":{"type":"integer"},"mode":{"type":"string","enum":["single","stream"]},"format":{"type":"string","enum":["raw","packed12","minmax"]},"decimation":{"type":"integer"},"trigger":{"type":"object"},"channel":{"type":"object"}},"required":[]}"#,
        protocol_cmds: "SET_SAMPLE_RATE / SET_TRIGGER / SET_ACQ / SET_CHANNEL",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_capture",
        description: "采集一次并等待触发完成。默认只返回统计量与 ≤256 点 minmax 预览，不含全量波形",
        input_schema: r#"{"type":"object","properties":{"mode":{"type":"string","enum":["single"]},"timeout_ms":{"type":"integer","default":2000},"max_preview_points":{"type":"integer","default":256}},"required":[]}"#,
        protocol_cmds: "ARM + EVENT_TRIGGER + READ_BUFFER",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_read_waveform",
        description: "按需分页拉取波形样点。硬顶 4096 点；要全量请用 scope_save_capture",
        input_schema: r#"{"type":"object","properties":{"capture_id":{"type":"integer"},"start_sample":{"type":"integer"},"count":{"type":"integer"},"max_points":{"type":"integer","default":512},"format":{"type":"string"},"channel":{"type":"integer"}},"required":["capture_id"]}"#,
        protocol_cmds: "READ_BUFFER",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_measure",
        description: "对指定采集做测量：频率/峰峰值/均值/RMS/占空比/上升时间。返回纯数字+单位",
        input_schema: r#"{"type":"object","properties":{"capture_id":{"type":"integer"},"metrics":{"type":"array","items":{"type":"string"}},"channel":{"type":"integer"}},"required":["capture_id"]}"#,
        protocol_cmds: "主机侧计算（精度最高）；流模式可走 MEASURE",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_i2c_decode",
        description:
            "把采集解码为 I2C 帧序列（START/地址/ACK/数据/STOP），返回帧列表与信号质量评估",
        input_schema: r#"{"type":"object","properties":{"capture_id":{"type":"integer"},"scl_channel":{"type":"integer","default":0},"sda_channel":{"type":"integer","default":1},"vih_lsb":{"type":"integer"},"vil_lsb":{"type":"integer"},"debounce_ns":{"type":"integer"}},"required":["capture_id"]}"#,
        protocol_cmds: "主机侧解码（基于 READ_BUFFER 数据）",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_list_captures",
        description: "列出主机侧保存的最近采集，供引用而不必重抓",
        input_schema: r#"{"type":"object","properties":{},"required":[]}"#,
        protocol_cmds: "无（本地 capture store）",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_save_capture",
        description: "把全量波形落盘（csv/npy/bin）。数据不进 LLM 上下文",
        input_schema: r#"{"type":"object","properties":{"capture_id":{"type":"integer"},"path":{"type":"string"},"format":{"type":"string","enum":["csv","npy","bin"]}},"required":["capture_id","path"]}"#,
        protocol_cmds: "无（本地写文件）",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_watch",
        description: "start→收集→stop 一体化的流式观察，返回滚动摘要与缺口统计",
        input_schema: r#"{"type":"object","properties":{"duration_ms":{"type":"integer","default":1000},"bucket":{"type":"integer"},"max_points":{"type":"integer","default":128}},"required":[]}"#,
        protocol_cmds: "ARM(stream) + 分片推送 + STOP",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_sim_set_scenario",
        description: "切换模拟器波形场景 / 注入故障（仅模拟器模式）",
        input_schema: r#"{"type":"object","properties":{"scenario":{"type":"string"},"seed":{"type":"integer"},"inject":{"type":"object"}},"required":[]}"#,
        protocol_cmds: "无（模拟器内部）",
        condition: "仅 transport=sim",
    },
    ToolSpec {
        name: "scope_debug_raw",
        description: "开发者逃生门：发任意命令码与 payload。默认不向 LLM 暴露",
        input_schema: r#"{"type":"object","properties":{"cmd":{"type":"integer"},"payload_hex":{"type":"string"}},"required":["cmd"]}"#,
        protocol_cmds: "任意（含 MEM_READ / MEM_WRITE）",
        condition: "仅 SCOPE_MCP_DEBUG=1",
    },
];

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--list-tools") {
        print_tools();
        return Ok(());
    }

    if args.iter().any(|a| a == "--selftest") {
        return selftest();
    }

    print_tools();
    println!();
    println!("MCP Server 尚未实现（P3 阶段）。");
    println!("现在可用：");
    println!("  scope-mcp --list-tools   查看工具清单与 schema");
    println!("  scope-mcp --selftest     走一遍模拟器，确认命令层可用");
    println!();
    println!("P0–P2 阶段请先用 `scope-cli sim ...` 开发与验证。");
    Ok(())
}

fn print_tools() {
    println!("# scope-mcp 工具清单（{} 个）", TOOLS.len());
    println!();
    println!("三层 token 防护：");
    println!("  1. capture / watch 默认只回统计量 + ≤256 点 minmax 预览");
    println!("  2. read_waveform 分页、默认 512 点、硬顶 4096、超出拒绝");
    println!("  3. 全量数据只进 capture store 和磁盘，永不进上下文");
    println!();

    for t in TOOLS {
        println!("── {}", t.name);
        println!("   说明   : {}", t.description);
        println!("   协议   : {}", t.protocol_cmds);
        println!("   注册条件: {}", t.condition);
        println!("   schema : {}", t.input_schema);
        println!();
    }
}

/// 跑一遍完整链路，确认命令层真的可用。
fn selftest() -> Result<()> {
    println!("== scope-mcp 自检 ==");
    println!();

    let mut bus = CommandBus::new(SimDevice::new(Scenario::I2c100k));
    let info = bus.connect()?;
    println!(
        "[1/5] 连接成功: {} 通道, 上限 {} Hz",
        info.ch_count, info.rate_max_hz
    );

    let actual = bus.set_sample_rate(857_143)?;
    println!("[2/5] 采样率: 请求 857143 → 实际 {actual} Hz（已量化）");

    bus.set_trigger(1, 0, 0, 2048, 1024, 1000)?;
    bus.set_acq(0, 1024, 0, 1)?;
    println!("[3/5] 触发与采集参数已配置");

    bus.arm()?;
    let ev = bus
        .wait_trigger(std::time::Duration::from_millis(2000))?
        .ok_or_else(|| anyhow::anyhow!("模拟器未产生触发事件"))?;
    let capture_id = u16::from_le_bytes([ev[0], ev[1]]);
    println!("[4/5] 触发完成: capture_id={capture_id}");

    let mut store = CaptureStore::default();
    let mut cap = scope_core::Capture::new(capture_id, actual, 2, 1024);

    for ch in 0..2usize {
        let payload = bus.read_buffer(capture_id, 0, 1024, 0, ch as u8)?;
        let hdr = scope_proto::ChunkHeader::decode(&payload).unwrap();
        for pair in payload[scope_proto::CHUNK_HEADER_LEN..].chunks_exact(2) {
            cap.channels[ch].push(u16::from_le_bytes([pair[0], pair[1]]));
        }
        println!(
            "      CH{}: {} 点, format={}, decimation={}",
            ch + 1,
            hdr.count,
            hdr.format,
            hdr.decimation
        );
    }
    store.push(cap);

    let c = store.latest().unwrap();
    for ch in 0..2usize {
        if let Some(s) = c.summary(ch) {
            println!(
                "      CH{} 摘要: pp={} LSB, 上升沿 {}",
                ch + 1,
                s.pp_lsb,
                s.rising_edges
            );
        }
    }

    let preview = c.preview(0, 32).unwrap();
    println!(
        "[5/5] minmax 预览: {} 个桶 (每桶 {} 样点) — 这就是 Agent 默认能看到的粒度",
        preview.y_min.len(),
        preview.bucket
    );
    println!();
    println!("自检通过。命令层与模拟器均可用。");
    Ok(())
}
