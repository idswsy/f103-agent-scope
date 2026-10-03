//! # scope-cli —— 命令行上位机
//!
//! P1 阶段的验收工具：能接硬件抓波形落 CSV，也能对着模拟器开发。
//!
//! ```text
//! # 没有硬件也能跑
//! scope-cli sim info
//! scope-cli sim capture --scenario sine_1k_3v3 -o wave.csv
//!
//! # 有硬件
//! scope-cli serial ports
//! scope-cli serial --port COM3 capture -n 2048 -o wave.csv
//! ```

#![deny(clippy::all)]

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use scope_core::{
    AcquireParams, Capture, CaptureStore, ChannelScale, CommandBus, DevicePort, I2cDecodeConfig,
    Levels, State,
};
use scope_sim::{Scenario, SimDevice};
use scope_transport_serial::{baud, SerialDevice};
use std::io::Write;

#[derive(Parser, Debug)]
#[command(
    name = "scope-cli",
    version,
    about = "F103 Agent Scope 命令行上位机",
    long_about = "可被 AI Agent 控制的数字示波器 / I2C 总线分析仪的调试工具。\n\
                  不带硬件时用 `sim` 子命令，全部功能都可用。"
)]
struct Cli {
    #[command(subcommand)]
    target: Target,
}

#[derive(Subcommand, Debug)]
enum Target {
    /// 对着内置模拟器运行（**不需要硬件**）
    Sim {
        #[command(subcommand)]
        action: Action,
    },

    /// 通过串口连接真实设备
    Serial {
        /// 端口名（如 COM3）
        #[arg(long, default_value = "COM3")]
        port: String,

        /// 波特率
        #[arg(long, default_value_t = baud::B_921600)]
        baud: u32,

        #[command(subcommand)]
        action: Action,
    },

    /// 列出系统中可用的串口
    Ports,
}

#[derive(Subcommand, Debug)]
enum Action {
    /// 读取设备信息与能力
    Info,

    /// 读取当前状态
    Status,

    /// 采集一次并落盘 / 打印摘要
    Capture {
        /// 波形场景（**仅模拟器有效**；接真机时忽略）
        #[arg(long, value_enum, default_value_t = ScenarioArg::Sine1k3v3)]
        scenario: ScenarioArg,

        /// 采样点数（F103 上限 4096）
        #[arg(short = 'n', long, default_value_t = 1024)]
        samples: u16,

        /// 采样率（Hz）。设备会量化到最近档位并回显实际值。
        #[arg(long, default_value_t = 857_142)]
        rate: u32,

        /// 触发电平（ADC LSB，12-bit 范围 0..4095）
        #[arg(long, default_value_t = 2048)]
        level: u16,

        /// 超时（毫秒）
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,

        /// 输出 CSV 路径（省略则只打印摘要，不落盘）
        #[arg(short = 'o', long)]
        out: Option<String>,
    },

    /// 抓一次总线并解码出 I2C 帧序列
    I2c {
        /// 波形场景（**仅模拟器有效**）
        #[arg(long, value_enum, default_value_t = ScenarioArg::I2c100k)]
        scenario: ScenarioArg,

        /// 采样点数（F103 上限 4096）
        #[arg(short = 'n', long, default_value_t = 4096)]
        samples: u16,

        /// 采样率（Hz）
        #[arg(long, default_value_t = 857_142)]
        rate: u32,

        /// SCL 通道（0 起）。与 --sda 一起省略时自动判定
        #[arg(long)]
        scl: Option<usize>,

        /// SDA 通道（0 起）
        #[arg(long)]
        sda: Option<usize>,

        /// 高电平门限（ADC LSB）。省略用 0.7·VDD
        #[arg(long)]
        vih: Option<u16>,

        /// 低电平门限（ADC LSB）。省略用 0.3·VDD
        #[arg(long)]
        vil: Option<u16>,

        /// 去抖时间（ns）
        #[arg(long, default_value_t = 50)]
        debounce_ns: u32,

        /// 超时（毫秒）
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,

        /// 输出解码文本路径（`i2c_decode.txt`）
        #[arg(short = 'o', long)]
        out: Option<String>,
    },

    /// 链路自检：PING 往返 + 吞吐
    Ping,

    /// 测量统计（对当前采集）
    Measure,
}

impl Action {
    /// 取出本动作隐含的场景；其他动作返回默认场景。
    ///
    /// 模拟器需要在跑动作之前就把设备建出来，所以得先问动作要场景。
    fn scenario_or_default(&self) -> ScenarioArg {
        match self {
            Action::Capture { scenario, .. } | Action::I2c { scenario, .. } => *scenario,
            _ => ScenarioArg::Sine1k3v3,
        }
    }
}

/// 场景名与 `Scenario::name()` 逐字一致 —— 文档、CLI、MCP 三处用同一套字符串，
/// 免得用户照着文档敲却在 CLI 上报「invalid value」。
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ScenarioArg {
    #[value(name = "sine_1k_3v3")]
    Sine1k3v3,
    #[value(name = "square_50k")]
    Square50k,
    #[value(name = "pulse_glitch")]
    PulseGlitch,
    #[value(name = "noise")]
    Noise,
    #[value(name = "dc")]
    Dc,
    #[value(name = "am")]
    Am,
    #[value(name = "i2c_100k")]
    I2c100k,
    #[value(name = "i2c_400k")]
    I2c400k,
    /// 地址被应答、但随后的一笔写被 NACK —— 用来演示「为什么 NACK」
    #[value(name = "i2c_nack")]
    I2cNack,
}

impl From<ScenarioArg> for Scenario {
    fn from(a: ScenarioArg) -> Scenario {
        match a {
            ScenarioArg::Sine1k3v3 => Scenario::Sine1k3v3,
            ScenarioArg::Square50k => Scenario::Square50k,
            ScenarioArg::PulseGlitch => Scenario::PulseGlitch,
            ScenarioArg::Noise => Scenario::Noise,
            ScenarioArg::Dc => Scenario::Dc,
            ScenarioArg::Am => Scenario::Am,
            ScenarioArg::I2c100k => Scenario::I2c100k,
            ScenarioArg::I2c400k => Scenario::I2c400k,
            ScenarioArg::I2cNack => Scenario::I2cNack,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.target {
        Target::Ports => {
            let ports = scope_transport_serial::list_ports();
            if ports.is_empty() {
                println!("没有找到任何串口。");
                println!("提示：插上板子后重试；或对着模拟器开发（不需要硬件）：");
                println!("      scope-cli sim capture --scenario sine_1k_3v3 -n 1024 -o wave.csv");
                return Ok(());
            }
            println!("{:<10} {:<40} 疑似目标", "端口", "描述");
            println!("{}", "-".repeat(70));
            for (name, desc, likely) in ports {
                let mark = if likely { "是" } else { "" };
                println!("{name:<10} {desc:<40} {mark}");
            }
            Ok(())
        }

        Target::Sim { action } => {
            let mut dev = SimDevice::new(action.scenario_or_default().into());
            println!("# 模拟器: {}", dev.describe());
            run(&mut dev, action)
        }

        Target::Serial { port, baud, action } => {
            let mut dev = SerialDevice::open(&port, baud, 5)?;
            println!("# 链路: {}", dev.describe());
            run(&mut dev, action)
        }
    }
}

fn run<P: DevicePort>(port: &mut P, action: Action) -> Result<()> {
    let mut bus = CommandBus::new(RefPort(port));

    match action {
        Action::Info => {
            let info = bus.connect().context("连接失败（GET_INFO 无响应）")?;
            println!("协议版本   : 0x{:02X}", info.proto_ver);
            println!(
                "固件版本   : {}.{}.{}",
                (info.fw_ver >> 16) & 0xFF,
                (info.fw_ver >> 8) & 0xFF,
                info.fw_ver & 0xFF
            );
            println!("型号       : 0x{:04X}", info.model);
            println!("UID        : {}", hex(&info.uid));
            println!("ADC        : {} bit", info.adc_bits);
            println!("通道数     : {}", info.ch_count);
            println!(
                "采样率范围 : {} .. {} Hz",
                info.rate_min_hz, info.rate_max_hz
            );
            println!("单次上限   : {} 点", info.capture_max_samples);
            println!(
                "payload    : 上行 {} B / 下行 {} B",
                info.max_rx_payload, info.max_tx_payload
            );
            println!("建议分片   : {} 点", info.preferred_chunk_samples);
            println!("能力位     : 0x{:08X}", info.caps);

            if info.rate_max_hz > scope_core::f103::MAX_INTERLEAVED_HZ {
                println!();
                println!("⚠ 设备上报的采样率上限超过 F103 的物理极限，可能是固件 bug。");
            }
            Ok(())
        }

        Action::Status => {
            bus.connect()?;
            let s = bus.get_status()?.state;
            println!("状态: {} ({})", state_name(s), s as u8);
            if let Some(cfg) = &bus.config {
                println!("采样率: {} Hz", cfg.rate_hz);
                println!(
                    "采集模式: {} | 样点: {} | 格式: {} | 抽点: {}",
                    cfg.acq_mode, cfg.capture_samples, cfg.format, cfg.decimation
                );
                println!(
                    "触发: mode={} src={} edge={} level={} LSB",
                    cfg.trigger_mode, cfg.trigger_source, cfg.trigger_edge, cfg.trigger_level_lsb
                );
            }
            Ok(())
        }

        Action::Ping => {
            bus.connect()?;
            println!("PING 往返测试（10 次）");
            let mut total = std::time::Duration::ZERO;
            let mut ok = 0;
            for i in 0..10u8 {
                let payload = [i, i.wrapping_mul(7), i.wrapping_add(3)];
                let (echoed, rtt) = bus.ping(&payload)?;
                if echoed == payload {
                    ok += 1;
                    total += rtt;
                    println!(
                        "  #{} {:>8.2} ms  echo ✓",
                        i + 1,
                        rtt.as_secs_f64() * 1000.0
                    );
                } else {
                    println!("  #{} 回显不符: {:?}", i + 1, echoed);
                }
            }
            if ok > 0 {
                println!(
                    "平均往返: {:.2} ms",
                    total.as_secs_f64() * 1000.0 / ok as f64
                );
            }
            Ok(())
        }

        Action::Capture {
            // 场景在 main() 里已经用来构造模拟器了，这里用不上
            scenario: _,
            samples,
            rate,
            level,
            timeout_ms,
            out,
        } => {
            let capture = capture_once(&mut bus, samples, rate, level, timeout_ms)?;
            let ch_count = capture.channels.len();
            let capture_id = capture.id;

            // 摘要（这是 Agent 默认能看到的东西）
            println!();
            println!(
                "采集摘要  capture_id={}  {} 点  {:.3} ms",
                capture_id,
                capture.len(),
                capture.duration_us() as f64 / 1000.0
            );
            for ch in 0..ch_count {
                if let Some(s) = capture.summary(ch) {
                    // rms = 相对 ADC 零点的真 RMS；ac_rms = 扣除直流后的波动（标准差）
                    println!(
                        "  CH{}: min={} max={} pp={} mean={:.1} rms={:.1} ac_rms={:.1}  上升沿={} 下降沿={}",
                        ch + 1,
                        s.min_lsb,
                        s.max_lsb,
                        s.pp_lsb,
                        s.mean_lsb,
                        s.rms_lsb,
                        s.ac_rms_lsb,
                        s.rising_edges,
                        s.falling_edges
                    );
                }
            }

            if let Some(path) = out {
                let scales: Vec<ChannelScale> =
                    (0..ch_count).map(|_| ChannelScale::default()).collect();
                let csv = capture.to_csv(&scales);
                let mut f =
                    std::fs::File::create(&path).with_context(|| format!("无法创建 {path}"))?;
                f.write_all(csv.as_bytes())?;
                println!();
                println!(
                    "已写出 {} ({} 字节, {} 行)",
                    path,
                    csv.len(),
                    capture.len() + 1
                );
                println!("提示：CSV 全量落盘，不进 LLM 上下文 —— 这是三层 token 防护的第三层。");
            } else {
                println!();
                println!("提示：加 -o wave.csv 可导出全量数据供绘图或喂给解码器。");
            }

            let _ = bus.stop();
            let mut store = CaptureStore::default();
            store.push(capture);
            Ok(())
        }

        Action::I2c {
            scenario: _,
            samples,
            rate,
            scl,
            sda,
            vih,
            vil,
            debounce_ns,
            timeout_ms,
            out,
        } => {
            let capture = capture_once(&mut bus, samples, rate, 2048, timeout_ms)?;

            // I2C 解码至少要两条线。单通道场景（如 dc / sine_1k_3v3）在这里
            // 就要说清楚，而不是等 decode_capture 报一个「通道越界」让人猜。
            if capture.channels.len() < 2 {
                bail!(
                    "I2C 解码需要至少 2 个通道，当前场景只有 {} 个。\n\
                     可以试试：\n  \
                     • 换用双通道场景：--scenario i2c_100k 或 i2c_400k\n  \
                     · 接真机时确认 GET_INFO 上报的 ch_count ≥ 2",
                    capture.channels.len()
                );
            }

            let levels = match (vih, vil) {
                (Some(h), Some(l)) => Levels {
                    vih_lsb: h,
                    vil_lsb: l,
                },
                (Some(h), None) => Levels {
                    vih_lsb: h,
                    vil_lsb: Levels::default_ratio().vil_lsb,
                },
                (None, Some(l)) => Levels {
                    vih_lsb: Levels::default_ratio().vih_lsb,
                    vil_lsb: l,
                },
                (None, None) => Levels::default_ratio(),
            };
            println!(
                "# 门限: VIH={} VIL={} LSB（判决带内不硬判 0/1）",
                levels.vih_lsb, levels.vil_lsb
            );

            // 通道：显式指定优先；都省略时自动判定，并把判定结果告诉用户
            let (scl_ch, sda_ch) = match (scl, sda) {
                (Some(a), Some(b)) => (a, b),
                (Some(a), None) => (a, if a == 0 { 1 } else { 0 }),
                (None, Some(b)) => (if b == 0 { 1 } else { 0 }, b),
                (None, None) => match scope_core::detect_channels(&capture, levels, debounce_ns) {
                    Some((a, b)) => {
                        println!("# 自动判定: SCL=CH{}  SDA=CH{}", a + 1, b + 1);
                        (a, b)
                    }
                    None => {
                        println!("# 自动判定失败（两条线都没有边沿）；按默认 CH1=SCL CH2=SDA 继续");
                        (0, 1)
                    }
                },
            };

            let cfg = I2cDecodeConfig {
                scl_channel: scl_ch,
                sda_channel: sda_ch,
                levels,
                debounce_ns,
            };
            let result = scope_core::decode_capture(&capture, &cfg)?;

            println!();
            print!("{}", result.to_text());

            let bytes = result.all_bytes();
            if !bytes.is_empty() {
                let hex: Vec<String> = bytes.iter().map(|b| format!("0x{b:02X}")).collect();
                println!("\n数据字节: [{}]", hex.join(", "));
            }

            if let Some(path) = out {
                let text = result.to_text();
                std::fs::write(&path, text.as_bytes())
                    .with_context(|| format!("无法写入 {path}"))?;
                println!("已写出 {path}");
            }

            Ok(())
        }

        Action::Measure => {
            bus.connect()?;
            println!("测量需要先有一次采集，见 `capture` 子命令。");
            println!("（P2 会在这里接上主机侧的定点测量：freq / vpp / duty / rise_time）");
            Ok(())
        }
    }
}

/// 跑一次完整采集：连接 → 配置 → 武装 → 等触发 → 分片拉取。
///
/// `capture` 与 `i2c` 两个动作共用 —— 采集逻辑只写一遍，
/// 免得解码那条路上的分片处理与落盘那条路走出两个版本的 bug。
fn capture_once<D: DevicePort>(
    bus: &mut CommandBus<D>,
    samples: u16,
    rate: u32,
    level: u16,
    timeout_ms: u64,
) -> Result<Capture> {
    bus.connect()?;

    // 采集编排在 core 里（`scope_core::acquire`）—— 它替我们处理了
    // EVENT_TRIGGER 的 18 字节解析、分片上限、以及「没有触发点」的哨兵映射。
    // CLI / GUI / MCP 三端共用这一份，不各写一遍。
    let params = AcquireParams {
        samples,
        rate_hz: rate,
        trigger_level_lsb: level,
        timeout: std::time::Duration::from_millis(timeout_ms),
    };
    let capture = scope_core::acquire(bus, &params)?;

    if capture.rate_hz != rate {
        println!(
            "# 采样率被量化: {} Hz → {} Hz（时间轴以 {} Hz 为准）",
            rate, capture.rate_hz, capture.rate_hz
        );
    }
    match capture.trigger_index {
        Some(i) => println!(
            "# 触发: capture_id={} index={} tick={} µs",
            capture.id, i, capture.device_tick_us
        ),
        // 软触发 / 未找到触发点时设备发哨兵 0xFFFF_FFFF —— core 已映射成 None
        None => println!(
            "# 触发: capture_id={} **无触发点**（软触发或未找到）tick={} µs",
            capture.id, capture.device_tick_us
        ),
    }

    if capture.overrun {
        println!("⚠ 采集期间发生溢出 —— 这份数据不完整，不能当作完整波形用");
    }
    Ok(capture)
}

fn state_name(s: State) -> &'static str {
    match s {
        State::Idle => "空闲",
        State::Armed => "已武装",
        State::Streaming => "流推送中",
        State::Done => "采集完成",
        State::Fault => "故障",
    }
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// 让 `run` 能接受 `&mut P` 而不是夺取所有权 ——
/// 调用者（`main`）需要在 `run` 返回后继续持有设备。
struct RefPort<'a, P: DevicePort>(&'a mut P);

impl<P: DevicePort> DevicePort for RefPort<'_, P> {
    fn write_all(&mut self, bytes: &[u8]) -> Result<(), scope_core::LinkError> {
        self.0.write_all(bytes)
    }
    fn read_some(&mut self) -> Result<Vec<u8>, scope_core::LinkError> {
        self.0.read_some()
    }
    fn describe(&self) -> String {
        self.0.describe()
    }
    fn is_simulated(&self) -> bool {
        self.0.is_simulated()
    }
    fn byte_rate(&self) -> u32 {
        self.0.byte_rate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    /// CLI 的场景清单必须与 `scope_sim::Scenario` **逐字一致**。
    ///
    /// CLI 这里有一份自己的枚举（为了 clap 的命令行补全与帮助文本），
    /// 而 `Scenario` 那边有 `ALL` 与 `name()` —— 同一份东西三处各写一遍。
    ///
    /// 回归：从前没有任何东西保证它们对得上。给模拟器加了 `i2c_nack` 之后，
    /// **CLI 与 GUI 都静默地少了一个场景**，没有一条测试会红。
    /// GUI 那边已经改成直接遍历 `Scenario::ALL`（把重复消灭掉）；
    /// CLI 因为要用 clap 的 `ValueEnum`，保留枚举，改用这条测试卡住。
    #[test]
    fn scenario_list_matches_the_simulator() {
        let cli: Vec<String> = ScenarioArg::value_variants()
            .iter()
            .filter_map(|v| v.to_possible_value().map(|p| p.get_name().to_string()))
            .collect();
        let sim: Vec<String> = Scenario::all_names().map(String::from).collect();
        assert_eq!(cli, sim, "CLI 的场景清单与 scope_sim::Scenario 分家了");

        // 而且每一个都要真的能转过去 —— 防止只加了枚举、忘了写 From 分支
        for v in ScenarioArg::value_variants() {
            let s: Scenario = (*v).into();
            assert_eq!(
                s.name(),
                v.to_possible_value().unwrap().get_name(),
                "枚举名与场景名对不上"
            );
        }
    }
}
