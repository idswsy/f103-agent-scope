//! # scope-mcp —— MCP Server（P3 阶段）
//!
//! 把示波器能力以 **14 个粗粒度工具**暴露给 AI Agent。
//!
//! ## 当前状态：骨架已就位，工具实现待 P3
//!
//! 这个二进制现在做两件有用的事：
//! 1. `--list-tools` 打印工具清单与 JSON Schema（人工核对用）
//! 2. `--selftest` 走一遍模拟器，确认命令层可用
//!
//! ## 设计约束（为什么是"粗粒度"）
//!
//! 14 个工具 ≈ 14 个**Agent 意图**，而不是 30+ 个逐命令工具。
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

mod jsonrpc;
mod session;

use anyhow::{Context, Result};
use scope_core::{CaptureStore, CommandBus};
use scope_sim::{Scenario, SimDevice};
use serde_json::{json, Value};

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
        description: "列出系统中可用的串口。不指定 port 连接时用内置模拟器，无需硬件",
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
        description: "设备当前状态与生效配置（含链路描述）。任何状态可调",
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
        input_schema: r#"{"type":"object","properties":{"capture_id":{"type":"integer"},"metrics":{"type":"array","items":{"type":"string","enum":["vpp","min","max","mean","rms","freq","duty","rise"]}},"channel":{"type":"integer"}},"required":["capture_id"]}"#,
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
        description: "把全量波形落盘为 CSV。全量数据不进 LLM 上下文",
        input_schema: r#"{"type":"object","properties":{"capture_id":{"type":"integer"},"path":{"type":"string"},"format":{"type":"string","enum":["csv"],"description":"目前只支持 csv"}},"required":["capture_id","path"]}"#,
        protocol_cmds: "无（本地写文件）",
        condition: "总是",
    },
    ToolSpec {
        name: "scope_watch",
        description: "在一段时间里连续采集，返回采集次数与缺口统计，外加第一次采集（供后续测量/解码）。设备侧流模式尚未实现",
        input_schema: r#"{"type":"object","properties":{"duration_ms":{"type":"integer","default":1000},"max_points":{"type":"integer","default":128}},"required":[]}"#,
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

    // 默认：跑 stdio 上的 MCP server
    serve()
}

// ══════════════════════════════════════════════════════════════
// stdio 上的 MCP server
// ══════════════════════════════════════════════════════════════

/// 协议版本。客户端给别的值也照常工作，只是回我们支持的这一版。
const PROTOCOL_VERSION: &str = "2024-11-05";

/// 跑 stdio 循环：一行一条 JSON-RPC 消息，读到 EOF 就退出。
fn serve() -> Result<()> {
    use std::io::{BufRead, Write};

    let debug_tools = std::env::var("SCOPE_MCP_DEBUG").is_ok();
    let mut session = session::Session::new();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    for line in stdin.lock().lines() {
        let line = line.context("读 stdin 失败")?;
        if line.trim().is_empty() {
            continue;
        }
        // IO 循环只做两件事：读一行、写一条。全部判断都在 `handle_line` 里 ——
        // 那边不碰 stdout，所以协议层的边界可以直接单测。
        if let Some(resp) = handle_line(&line, &mut session, debug_tools) {
            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// 处理一条消息。
///
/// 返回 `None` 表示这是**通知**，按 JSON-RPC 规矩不回响应。
///
/// 不碰 stdout —— 协议层的边界（通知怎么判、错误码怎么选、校验的次序）
/// 全在这里，可以单测。之前这些逻辑和 IO 混在一个循环里，只能靠手工
/// 灌 stdin 验证。
fn handle_line(
    line: &str,
    session: &mut session::Session,
    debug_tools: bool,
) -> Option<jsonrpc::Response> {
    use jsonrpc::{code, Response};

    // 先宽松解析成 Value 再收紧成 Request —— 这样「**是**合法 JSON 但
    // 不**是**合法请求对象」能报 -32600 并**回显 id**，而不是一律
    // -32700 且把 id 丢成 null。客户端靠 id 匹配响应，丢了它只能干等。
    let raw: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            // 这一步才是真的解析失败：连 id 都无从得知，按规范用 null
            return Some(Response::err(
                Value::Null,
                code::PARSE_ERROR,
                format!("JSON 解析失败: {e}"),
            ));
        }
    };
    let req: jsonrpc::Request = match serde_json::from_value(raw.clone()) {
        Ok(r) => r,
        Err(e) => {
            let id = raw.get("id").cloned().unwrap_or(Value::Null);
            return Some(Response::err(
                id,
                code::INVALID_REQUEST,
                format!("不是合法的 JSON-RPC 请求对象: {e}"),
            ));
        }
    };

    // --- 请求级校验：先确定「该不该回」，再确定「回什么」 ---
    //
    // 错误响应也要尽量带上 id（能拿到就带），客户端靠 id 匹配响应。
    let id_for_err = || raw.get("id").cloned().unwrap_or(Value::Null);

    // `jsonrpc` 是必填字段。之前「缺失或空串」被当作合法请求照常处理 ——
    // 一条根本不是 JSON-RPC 的消息也能拿到成功响应。
    if req.jsonrpc != "2.0" {
        return Some(Response::err(
            id_for_err(),
            code::INVALID_REQUEST,
            format!("jsonrpc 必须是 \"2.0\"，收到 {:?}", req.jsonrpc),
        ));
    }

    // 通知的判据是「原始 JSON 里**有没有 id 这个键**」，而不是
    // 「id 解析出来是不是 None」。`{"id":null}` 是合法请求（id 为 null），
    // 必须回响应 —— 之前 serde 把显式 null 折叠成 None，这类请求被当成
    // 通知静默丢弃，客户端永远等不到回复。
    //
    // `?` 在这里就是「没有 id 键 → 直接返回 None（通知，不回）」。
    raw.get("id")?;
    let id = req.id.clone().unwrap_or(Value::Null);

    // `params` 若给，必须是对象（命名参数）。给了数组会让后面的
    // `params.get("name")` 静默取到 None，冒出一个与实际不符的「缺少 name」。
    let params = req.params.clone();
    if !params.is_null() && !params.is_object() {
        return Some(Response::err(id, code::INVALID_PARAMS, "params 必须是对象"));
    }

    match req.method.as_str() {
        "initialize" => Some(Response::ok(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "scope-mcp",
                    "version": scope_core::VERSION,
                },
            }),
        )),

        "ping" => Some(Response::ok(id, json!({}))),

        "tools/list" => Some(Response::ok(id, json!({ "tools": tool_list(debug_tools) }))),

        "tools/call" => {
            let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = match params.get("arguments") {
                None => json!({}),
                Some(a) if a.is_object() => a.clone(),
                Some(_) => {
                    return Some(Response::err(
                        id,
                        code::INVALID_PARAMS,
                        "arguments 必须是对象",
                    ))
                }
            };
            if name.is_empty() {
                Some(Response::err(
                    id,
                    code::INVALID_PARAMS,
                    "tools/call 缺少 name",
                ))
            } else if !tool_enabled(name, debug_tools) {
                Some(Response::err(
                    id,
                    code::INVALID_PARAMS,
                    format!("工具 {name} 在当前会话里不可用（未注册）"),
                ))
            } else if !TOOLS.iter().any(|t| t.name == name) {
                // 未知工具名与「未注册」走同一类错误 —— 两条路径报不同
                // 种类的错会让客户端难以统一处理。
                Some(Response::err(
                    id,
                    code::INVALID_PARAMS,
                    format!("未知工具 {name}"),
                ))
            } else {
                Some(Response::ok(id, dispatch(session, name, &args)))
            }
        }

        other => Some(Response::err(
            id,
            code::METHOD_NOT_FOUND,
            format!("未知方法 {other}"),
        )),
    }
}

/// 这个工具在 `tools/list` 里出不出现。
///
/// **只按启动时定死的东西过滤**，不看连接状态。理由：MCP 客户端一般在启动时
/// `tools/list` 一次就缓存了，之后连接模拟器并不会让它重新拉列表 ——
/// 按 `sim` 过滤的话，`scope_sim_set_scenario` 在连上模拟器后依然看不见。
/// 所以它总是列出，真连了串口再调用时给一句明确的错误。
///
/// `scope_debug_raw` 是例外：它由环境变量决定，进程启动后就定死了，
/// 不会中途变化，过滤掉是安全的。
fn tool_enabled(name: &str, debug: bool) -> bool {
    match name {
        "scope_debug_raw" => debug,
        _ => true,
    }
}

/// 工具清单 —— 按会话状态过滤后返回。
fn tool_list(debug: bool) -> Vec<Value> {
    TOOLS
        .iter()
        .filter(|t| tool_enabled(t.name, debug))
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": serde_json::from_str::<Value>(t.input_schema)
                    .unwrap_or_else(|_| json!({ "type": "object" })),
            })
        })
        .collect()
}

/// 把一次 `tools/call` 派发到具体实现，并把结果包成 MCP 的 content 形状。
fn dispatch(session: &mut session::Session, name: &str, args: &Value) -> Value {
    use jsonrpc::{tool_error, tool_result};

    let r = match name {
        "scope_list_devices" => session.list_devices(),
        "scope_connect" => session.connect(args),
        "scope_disconnect" => session.disconnect(),
        "scope_status" => session.status(),
        "scope_configure" => session.configure(args),
        "scope_capture" => session.capture(args),
        "scope_read_waveform" => session.read_waveform(args),
        "scope_measure" => session.measure(args),
        "scope_i2c_decode" => session.i2c_decode(args),
        "scope_list_captures" => session.list_captures(),
        "scope_save_capture" => session.save_capture(args),
        "scope_watch" => session.watch(args),
        "scope_sim_set_scenario" => session.sim_set_scenario(args),
        "scope_debug_raw" => session.debug_raw(args),
        other => {
            return tool_error(format!("未知工具 {other}"), None);
        }
    };

    match r {
        Ok(v) => tool_result(&v, false),
        Err(e) => tool_error(e.message, e.hint),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use jsonrpc::code;

    /// 走一遍协议层，返回 (响应, 是不是通知)。
    fn call(line: &str) -> (Option<jsonrpc::Response>, bool) {
        let mut s = session::Session::new();
        let r = handle_line(line, &mut s, false);
        let is_notification = r.is_none();
        (r, is_notification)
    }

    fn err_code(line: &str) -> Option<i32> {
        call(line).0?.error.map(|e| e.code)
    }

    #[test]
    fn unparseable_json_is_a_parse_error_with_a_null_id() {
        let (r, _) = call("这不是 JSON");
        let r = r.expect("解析失败也要回响应");
        assert_eq!(r.error.unwrap().code, code::PARSE_ERROR);
        assert_eq!(r.id, Value::Null, "连 id 都无从得知，按规范用 null");
    }

    #[test]
    fn valid_json_that_is_not_a_request_echoes_the_id() {
        // 回归：曾经一律报 -32700 并把 id 丢成 null，客户端靠 id 匹配响应，
        // 丢了它只能干等。
        let (r, _) = call(r#"{"jsonrpc":"2.0","id":42}"#);
        let r = r.unwrap();
        assert_eq!(r.error.unwrap().code, code::INVALID_REQUEST);
        assert_eq!(r.id, json!(42), "能拿到 id 就要回显");
    }

    #[test]
    fn jsonrpc_field_is_required() {
        // 回归：缺 jsonrpc 的裸消息曾经被当作合法请求照常处理并回成功
        assert_eq!(
            err_code(r#"{"id":1,"method":"ping"}"#),
            Some(code::INVALID_REQUEST)
        );
        assert_eq!(
            err_code(r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#),
            Some(code::INVALID_REQUEST)
        );
    }

    #[test]
    fn explicit_null_id_is_a_request_not_a_notification() {
        // `{"id":null}` 是合法请求（id 为 null），必须回响应。
        // 回归：serde 把显式 null 折叠成 None，这类请求被当成通知静默丢弃。
        let (r, is_notification) = call(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#);
        assert!(!is_notification, "显式 null 的 id 仍要回");
        assert!(r.unwrap().error.is_none());
    }

    #[test]
    fn a_request_without_the_id_key_is_a_notification() {
        let (_, is_notification) =
            call(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(is_notification, "没有 id 键才是通知");
    }

    #[test]
    fn container_types_are_validated() {
        // params 给数组 → 否则后面 params.get("name") 静默取到 None，
        // 冒出一个与实际不符的「缺少 name」
        assert_eq!(
            err_code(r#"{"jsonrpc":"2.0","id":1,"method":"ping","params":[1,2]}"#),
            Some(code::INVALID_PARAMS)
        );
        assert_eq!(
            err_code(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"scope_status","arguments":[1]}}"#
            ),
            Some(code::INVALID_PARAMS)
        );
    }

    #[test]
    fn unknown_method_and_unknown_tool_are_distinct_but_consistent() {
        assert_eq!(
            err_code(r#"{"jsonrpc":"2.0","id":1,"method":"没这个方法"}"#),
            Some(code::METHOD_NOT_FOUND)
        );
        assert_eq!(
            err_code(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"没这个工具"}}"#
            ),
            Some(code::INVALID_PARAMS)
        );
        // 未注册的工具与未知工具同一类错误 —— 两条路径报不同种类的错
        // 会让客户端难以统一处理
        assert_eq!(
            err_code(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"scope_debug_raw"}}"#
            ),
            Some(code::INVALID_PARAMS)
        );
    }

    #[test]
    fn initialize_reports_the_protocol_and_server() {
        let (r, _) = call(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
        let v = r.unwrap().result.unwrap();
        assert_eq!(v["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(v["serverInfo"]["name"], "scope-mcp");
        assert!(v["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_matches_the_tools_constant() {
        let (r, _) = call(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let tools = r.unwrap().result.unwrap()["tools"].clone();
        let arr = tools.as_array().unwrap();
        // 不带 SCOPE_MCP_DEBUG 时少一个 scope_debug_raw
        assert_eq!(arr.len(), TOOLS.len() - 1);
        // 每个工具都要有 name / description / inputSchema，且 schema 是合法 JSON
        for t in arr {
            assert!(t["name"].is_string(), "缺 name: {t}");
            assert!(t["description"].is_string(), "缺 description: {t}");
            assert!(t["inputSchema"].is_object(), "schema 不是对象: {t}");
        }
    }

    #[test]
    fn every_tool_schema_parses_as_json() {
        // TOOLS 里的 input_schema 是字符串字面量 —— 拼错了要到运行时才炸。
        // 这条测试把它提前到编译期后立刻暴露。
        for t in TOOLS {
            serde_json::from_str::<Value>(t.input_schema)
                .unwrap_or_else(|e| panic!("{} 的 input_schema 不是合法 JSON: {e}", t.name));
        }
    }

    #[test]
    fn tool_names_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(seen.insert(t.name), "工具名重复：{}", t.name);
        }
    }
}
