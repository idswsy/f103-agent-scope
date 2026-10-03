//! 证据包 —— 将「一次采集中已算出的全部事实」渲染为文本，供语言模型分析。
//!
//! # 输出为文本而非 serde 结构体的原因
//!
//! 结构体加各端各自的渲染器，等于**同一份内容存在两种表述** —— 这正是本项目
//! 反复出现的一类缺陷（协议文档与实现不一致、schema 与参数解析不一致）。
//! 因此本模块只产出**一种形式**：人类可读、语言模型可读、`scope-mcp`
//! 后续亦可直接返回的文本。
//!
//! 文本另有一项实际收益：其 token 开销低于等价 JSON。以通道 0 的峰峰值为例，
//! `"pp_lsb":2867` 为 15 个字符，而 `pp=2867` 为 7 个。
//!
//! # 核心约定：仅转述已算出的结果，不作任何重算
//!
//! 测量取 [`crate::measure()`]（全项目唯一实现），波形包络取
//! [`Capture::preview()`]（与 MCP 同源的 minmax 降采样）。
//! **本模块不执行任何信号处理。**
//!
//! # 四项必须维持的约束
//!
//! 1. **告警须为未过滤原文**。`I2cDecode::warnings` 中的 `TruncatedFrame`
//!    必须保留 —— 否则模型会把「共 19 帧」当作 19 笔完整事务作答。
//!    （GUI 面板所用的 `warning_lines()` 会**主动滤除**该条，不可用于此处。）
//! 2. **通道来源须如实标注**。GUI 使用 `scl_channel`/`sda_channel` 的
//!    用户选取值，MCP 才执行自动检测。本文本内不得执行 `detect_channels`，
//!    否则将与用户界面所显示的解码结果不一致。
//! 3. **长度须有硬上限**。帧表与包络均可能过长，超出时须**带标记**截断，
//!    不得静默截断 —— 静默截断会被读作「数据到此为止」。
//! 4. **模拟器数据源须显式声明**。缺少该声明时，模型会把模拟器数据
//!    当作真实总线的结论。

use std::fmt::Write as _;

use crate::capture::{Capture, ChannelScale};
use crate::command::DeviceConfig;
use crate::i2c_decode::{I2cDecode, I2cDecodeConfig};
use crate::measure::measure;

/// 波形包络的**目标**桶数。
///
/// 与 MCP 的 `PREVIEW_POINTS`（256）不同：后者服务于 Agent 的工具返回值，
/// 而此处仅为附带上下文，128 桶足以呈现形状，且不会挤占帧表。
///
/// ⚠ 该值为**目标值**，非硬上限。见 [`preview_section`] 的说明：
/// 采集点数较短时，实际桶数可达 `2 * PREVIEW_POINTS - 1`。
pub const PREVIEW_POINTS: usize = 128;

/// 帧表里最多列几帧。超出的在末尾记一条截断说明。
pub const MAX_FRAMES: usize = 64;

/// 整份证据包的字符上限（约合 2300 token）。
///
/// 这是**兜底**，不是主要控制手段 —— 主要控制是上面两个上限。
/// 真触到这条说明有没预料到的膨胀，那时宁可截断并说明，也不要把它发出去。
pub const MAX_CHARS: usize = 8_000;

/// 构造证据包需要、但 [`Capture`] 自己不携带的上下文。
#[derive(Debug, Clone)]
pub struct EvidenceInput<'a> {
    /// 采集本身。
    pub capture: &'a Capture,
    /// 电压换算（显示层唯一允许做伏特换算的地方）。
    pub scale: &'a ChannelScale,
    /// 链路描述，例如 `sim(i2c_100k)` 或 `COM7 @ 921600`。
    pub link: &'a str,
    /// 数据是不是模拟器产生的。**必须如实填** —— 它决定模型敢不敢下结论。
    pub simulated: bool,
    /// 设备回显的配置。没连上时为 `None`。
    pub config: Option<&'a DeviceConfig>,
    /// I2C 解码结果。没解或通道数不够时为 `None`。
    pub decode: Option<&'a I2cDecode>,
    /// 解码用的配置（不是用户手选通道时用来标注）。
    pub decode_cfg: Option<&'a I2cDecodeConfig>,
    /// 用户额外想问的问题。为空则只给证据。
    pub question: Option<&'a str>,
}

/// 把一次采集渲染成证据包文本。
///
/// 纯函数：无 IO、无全局状态、输入相同则输出逐字节相同。
pub fn build_evidence(input: &EvidenceInput<'_>) -> String {
    let mut s = String::with_capacity(2048);

    link_section(&mut s, input);
    config_section(&mut s, input.config);
    capture_section(&mut s, input.capture);
    channels_section(&mut s, input);
    preview_section(&mut s, input.capture);
    decode_section(&mut s, input);
    footer_section(&mut s, input);

    truncate_to_cap(s)
}

/// 链路与设备。**第一段就声明模拟器**，不给模型先入为主的机会。
fn link_section(s: &mut String, input: &EvidenceInput<'_>) {
    let _ = writeln!(s, "== 链路与设备 ==");
    if input.simulated {
        let _ = writeln!(
            s,
            "链路: {}   ⚠ 这是【模拟器】产生的数据，不是真实硬件上的总线",
            input.link
        );
    } else {
        let _ = writeln!(s, "链路: {}", input.link);
    }
    let _ = writeln!(
        s,
        "通道数: {}   采样率上限: {} Hz   ADC: 12 bit   单次采集上限: {} 点",
        input.capture.channels.len(),
        crate::f103::MAX_SAMPLE_RATE_HZ,
        crate::f103::MAX_CAPTURE_SAMPLES,
    );
}

/// 设备回显的配置 —— 是**设备返回的实际值**，不是主机请求的值。
fn config_section(s: &mut String, config: Option<&DeviceConfig>) {
    let _ = writeln!(s);
    let _ = writeln!(s, "== 配置（设备回显的实际值）==");
    let Some(c) = config else {
        let _ = writeln!(s, "（未知 —— 本次没读到设备配置）");
        return;
    };
    let _ = writeln!(
        s,
        "实际采样率: {} Hz   采集点数: {}   抽点: {}   采集模式: {}",
        c.rate_hz,
        c.capture_samples,
        c.decimation,
        acq_mode_name(c.acq_mode),
    );
    let _ = writeln!(
        s,
        "触发: 模式={} 边沿={} 源=ch{} 电平={} LSB",
        trigger_mode_name(c.trigger_mode),
        trigger_edge_name(c.trigger_edge),
        c.trigger_source,
        c.trigger_level_lsb,
    );
    let _ = writeln!(
        s,
        "CH0: 使能={} 耦合={}",
        if c.ch0_enable != 0 { "是" } else { "否" },
        if c.ch0_coupling != 0 { "AC" } else { "DC" },
    );
}

/// 采集本身的元数据。溢出要显式标出来 —— 有它的数据不能假装完整。
fn capture_section(s: &mut String, cap: &Capture) {
    let _ = writeln!(s);
    let _ = writeln!(s, "== 本次采集 ==");
    let n = cap.channels.first().map(|c| c.len()).unwrap_or(0);
    let dur_us = if cap.rate_hz > 0 {
        n as f64 * 1e6 / cap.rate_hz as f64
    } else {
        0.0
    };
    let _ = writeln!(
        s,
        "采集号: {}   实际点数: {}   时长: {:.1} µs @ {} Hz",
        cap.id, n, dur_us, cap.rate_hz
    );
    match cap.trigger_index {
        Some(i) => {
            let _ = writeln!(s, "触发点: 窗口内第 {} 个样点", i);
        }
        None => {
            let _ = writeln!(s, "触发点: 无（软件触发或整窗没等到触发）");
        }
    }
    if cap.overrun {
        let _ = writeln!(
            s,
            "⚠ 期间发生【溢出】：数据不完整，缺口处的波形与解码结论都不可信"
        );
    }
    if n < cap.expected_samples as usize {
        let _ = writeln!(
            s,
            "⚠ 实际点数 {} 少于期望的 {}，说明传输有缺口",
            n, cap.expected_samples
        );
    }
}

/// 每通道的测量 —— 全部来自 `crate::measure()`，没有一个数是在这里算的。
fn channels_section(s: &mut String, input: &EvidenceInput<'_>) {
    let _ = writeln!(s);
    let _ = writeln!(s, "== 通道测量（机器精确计算的结果，可直接引用）==");

    let mut any = false;
    for ch in 0..input.capture.channels.len() {
        let Some(m) = measure(input.capture, ch, input.scale) else {
            let _ = writeln!(s, "ch{ch}: 测不出（通道无数据）");
            continue;
        };
        any = true;
        let _ = writeln!(
            s,
            "ch{ch}: Vpp={:.4} V  min={:.4} V  max={:.4} V  mean={:.4} V  AC-RMS={:.4} V",
            m.vpp, m.min, m.max, m.mean, m.ac_rms
        );
        // 「测不出」要写成「测不出」，不能写 0 —— 项目纪律：宁可说不知道
        let freq = match m.freq_hz {
            Some(f) => format!("{f:.1} Hz"),
            None => "测不出（无明显周期）".to_string(),
        };
        let duty = match m.duty_pct {
            Some(d) => format!("{d:.1} %"),
            None => "测不出".to_string(),
        };
        let rise = match m.rise_ns {
            Some(r) => format!("{r:.1} ns"),
            None => "测不出（没有干净的边沿）".to_string(),
        };
        let _ = writeln!(s, "     频率={freq}  占空比={duty}  上升时间={rise}");
    }
    if !any {
        let _ = writeln!(s, "（所有通道都测不出 —— 采集可能是空的）");
    }
    let _ = writeln!(
        s,
        "注：频率与占空比已排除空闲段；上升时间受采样周期限制，分辨率约 1 个采样周期。"
    );
}

/// 波形包络。**必须写明「桶内细节不可见」**，否则模型会对着 128 个数编形状。
///
/// ⚠ 桶数是**约** `PREVIEW_POINTS`，不是「最多」：[`Capture::preview`] 里
/// `bucket = (len / target).max(1)`，所以当 `len` 落在 `[target, 2*target)` 时
/// 桶退化到 1，实际桶数能到 `2*target - 1`。这里如实打印**实际**桶数，
/// 不要在表头上写一个会被实际数据推翻的上限。
fn preview_section(s: &mut String, cap: &Capture) {
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "== 波形包络（每桶的 min/max，目标 {PREVIEW_POINTS} 桶）=="
    );
    let _ = writeln!(
        s,
        "⚠ 每个桶里装了多个样点，只有 min/max 被保留 —— 桶内细节不可见，"
    );
    let _ = writeln!(s, "  不要对桶内的毛刺、振铃、边沿形状下结论。");

    let mut wrote = false;
    for ch in 0..cap.channels.len() {
        let Some(p) = cap.preview(ch, PREVIEW_POINTS) else {
            continue;
        };
        wrote = true;
        let _ = writeln!(s, "ch{ch}  桶宽={:.2} µs  桶数={}", p.dt_us, p.y_min.len());
        let _ = writeln!(s, "  min: {}", join_u16(&p.y_min));
        let _ = writeln!(s, "  max: {}", join_u16(&p.y_max));
    }
    if !wrote {
        let _ = writeln!(s, "（没有可用的包络）");
    }
}

/// I2C 解码结果 —— 帧表 + **未过滤的**告警原文。
fn decode_section(s: &mut String, input: &EvidenceInput<'_>) {
    let _ = writeln!(s);
    let _ = writeln!(s, "== I2C 解码 ==");

    let Some(d) = input.decode else {
        let _ = writeln!(s, "（没有解码结果 —— 通道数不足 2，或还没解码）");
        return;
    };

    if let Some(cfg) = input.decode_cfg {
        // 说明通道是**用户选的**，不是自动检测的 —— 这一点影响模型对结论的信任度
        let _ = writeln!(
            s,
            "SCL=ch{}  SDA=ch{}   （通道是用户在界面上选的，不是自动检测的结果）",
            cfg.scl_channel, cfg.sda_channel
        );
        let _ = writeln!(
            s,
            "判决门限: VIH={} LSB  VIL={} LSB   去抖: {} ns",
            cfg.levels.vih_lsb, cfg.levels.vil_lsb, cfg.debounce_ns
        );
    }

    let _ = writeln!(
        s,
        "帧数: {}   解码可信: {}",
        d.transactions.len(),
        if d.is_untrustworthy() { "否" } else { "是" }
    );

    let truncated = count_truncated(d);
    if d.transactions.is_empty() {
        let _ = writeln!(s, "（一帧都没解出来）");
    } else {
        let _ = writeln!(s, "帧表:");
        for (i, t) in d.transactions.iter().take(MAX_FRAMES).enumerate() {
            let bytes: Vec<String> = t.bytes.iter().map(|b| format!("{b:02X}")).collect();
            let _ = writeln!(
                s,
                "  #{:<3} t={:>9.3} ms  {}{}{}{}",
                i + 1,
                t.start_time_us as f64 / 1000.0,
                if t.repeated { "Sr " } else { "" },
                t.address_str(),
                if bytes.is_empty() {
                    String::new()
                } else {
                    format!("  数据: {}", bytes.join(" "))
                },
                if t.complete {
                    ""
                } else {
                    "  ⚠被窗口截断"
                },
            );
        }
        let extra = d.transactions.len().saturating_sub(MAX_FRAMES);
        if extra > 0 {
            // 不静默截断：说清楚还剩多少没列
            let _ = writeln!(s, "  …… 另有 {extra} 帧未列出（超出 {MAX_FRAMES} 帧上限）");
        }
    }

    if truncated > 0 {
        let _ = writeln!(
            s,
            "⚠ 这 {truncated} 帧没有正常 STOP 收尾 —— I2C 总线一直在跑，采集窗口切在哪是随机的，"
        );
        let _ = writeln!(
            s,
            "  所以最后一帧被切断是正常的。**不要把「帧数」当成「完整事务笔数」。**"
        );
    }

    if !d.warnings.is_empty() {
        let _ = writeln!(s, "告警（原文，未过滤）:");
        for w in &d.warnings {
            let _ = writeln!(s, "  - {}", w.text());
        }
    }
}

/// 结尾：把「你该怎么用这些数字」说死，防止模型自由发挥。
fn footer_section(s: &mut String, input: &EvidenceInput<'_>) {
    let _ = writeln!(s);
    let _ = writeln!(s, "== 说明 ==");
    if input.simulated {
        let _ = writeln!(
            s,
            "- 以上是【模拟器】数据。可以分析信号与协议本身，但**不能**据此判断真实硬件好坏。"
        );
    }
    // 满量程 = 每 LSB 的伏特数 × 4096。别写死 3.3 —— 标定一旦变了这里就错了。
    let full_scale = input.scale.volts_per_lsb * 4096.0;
    let _ = writeln!(
        s,
        "- 电压是按 {full_scale:.2} V 满量程做的**未标定**换算，只看相对关系，不要当成校准值。"
    );
    let _ = writeln!(
        s,
        "- 上面所有数字都来自精确计算。**直接引用它们，不要自己估算或重新推导。**"
    );
    let _ = writeln!(
        s,
        "- 看不出来的就说看不出来。需要更多信息时，明确指出需要什么（例如「需要换个场景重采」）。"
    );

    if let Some(q) = input.question {
        let q = q.trim();
        if !q.is_empty() {
            let _ = writeln!(s);
            let _ = writeln!(s, "== 用户的问题 ==");
            let _ = writeln!(s, "{q}");
        }
    }
}

/// 数一数有多少帧没正常收尾。
fn count_truncated(d: &I2cDecode) -> usize {
    // `complete: false` 才是真正的「被窗口切断」。注意这里**不**用告警条数代替 ——
    // `warnings` 里那些是别的种类，两者不是一回事。
    d.transactions.iter().filter(|t| !t.complete).count()
}

fn join_u16(v: &[u16]) -> String {
    let mut s = String::with_capacity(v.len() * 5);
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{x}");
    }
    s
}

fn acq_mode_name(m: u8) -> &'static str {
    match m {
        0 => "single",
        1 => "stream",
        _ => "?",
    }
}

fn trigger_mode_name(m: u8) -> &'static str {
    match m {
        0 => "auto",
        1 => "normal",
        2 => "single",
        _ => "?",
    }
}

fn trigger_edge_name(e: u8) -> &'static str {
    match e {
        0 => "rising",
        1 => "falling",
        _ => "?",
    }
}

/// 兜底截断。**带标记**，不静默。
fn truncate_to_cap(s: String) -> String {
    if s.chars().count() <= MAX_CHARS {
        return s;
    }
    let mut out: String = s.chars().take(MAX_CHARS).collect();
    out.push_str(&format!(
        "\n\n⚠ 证据包超过 {MAX_CHARS} 字符上限，**上面是截断后的内容** —— 缺失的部分请当作未知。"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::ChannelScale;
    use crate::i2c_decode::{decode_capture, I2cDecodeConfig};

    const HIGH: u16 = 4095;
    const LOW: u16 = 0;

    /// 造一份两通道采集：SCL=ch0、SDA=ch1，解得出 `0x88 (W)` + 一个数据字节。
    ///
    /// `with_stop = false` 时**砍掉收尾的 STOP** —— 这样最后一帧没有正常收尾，
    /// 用来验证「截断帧没有被吞掉」。
    fn i2c_capture(with_stop: bool) -> Capture {
        let rate = 800_000u32;
        let mut scl: Vec<u16> = Vec::new();
        let mut sda: Vec<u16> = Vec::new();

        macro_rules! hold {
            ($sv:expr, $dv:expr, $n:expr) => {
                for _ in 0..$n {
                    scl.push($sv);
                    sda.push($dv);
                }
            };
        }
        macro_rules! bit {
            ($sv:expr) => {{
                let d = if $sv { HIGH } else { LOW };
                for _ in 0..4 {
                    scl.push(LOW);
                    sda.push(d);
                }
                for _ in 0..4 {
                    scl.push(HIGH);
                    sda.push(d);
                }
            }};
        }

        // START：SCL 高时 SDA 由高变低
        hold!(HIGH, HIGH, 4);
        hold!(HIGH, LOW, 4);
        hold!(LOW, LOW, 4);
        for b in [0x88u8, 0x00] {
            for i in (0..8).rev() {
                bit!((b >> i) & 1 == 1);
            }
            bit!(false); // ACK
        }
        if with_stop {
            hold!(LOW, LOW, 4);
            hold!(HIGH, LOW, 4);
            hold!(HIGH, HIGH, 4);
        }

        let mut cap = Capture::new(1, rate, 2, scl.len() as u32);
        cap.channels = vec![scl, sda];
        cap
    }

    /// 造一份直流采集 —— 没有边沿，所以频率/占空比/上升时间都测不出。
    fn flat_capture() -> Capture {
        let mut cap = Capture::new(2, 800_000, 2, 512);
        cap.channels = vec![vec![2048u16; 512], vec![2048u16; 512]];
        cap
    }

    fn input<'a>(cap: &'a Capture, scale: &'a ChannelScale) -> EvidenceInput<'a> {
        EvidenceInput {
            capture: cap,
            scale,
            link: "sim(i2c_100k)",
            simulated: true,
            config: None,
            decode: None,
            decode_cfg: None,
            question: None,
        }
    }

    // ── 坑 1 的回归：告警必须用未过滤的原文 ──────────────────────────

    /// **这是本文件最重要的一条测试。**
    ///
    /// `I2cDecode::warnings` 里那条 `TruncatedFrame` 必须活到文本里。
    /// GUI 面板上那层 `vmodel::warning_lines()` 是**故意滤掉它**的
    /// （滤掉的理由写在 vmodel.rs:163-166：截断每采必有，标黄等于天天报警），
    /// 所以这里用错层的话，模型会把「共 N 帧」当成 N 笔完整事务来回答。
    ///
    /// 断的是**行为**不是措辞：只要「截断帧」这件事被说出来了就算过。
    #[test]
    fn truncated_frame_survives_into_the_text() {
        let cap = i2c_capture(false); // 砍掉 STOP → 最后一帧没收尾
        let scale = ChannelScale::default();
        let d = decode_capture(&cap, &I2cDecodeConfig::default()).expect("应能解码");

        // 前提：这份数据里确实有截断帧和对应告警，否则这条测试是假通过的
        assert!(
            d.transactions.iter().any(|t| !t.complete),
            "测试数据本身就没有截断帧 —— 这条测试失去意义"
        );
        assert!(
            d.warnings
                .iter()
                .any(|w| matches!(w, crate::i2c_decode::I2cWarning::TruncatedFrame)),
            "解码器没有给出 TruncatedFrame 告警 —— 测试数据不对"
        );

        let mut i = input(&cap, &scale);
        i.decode = Some(&d);
        let cfg = I2cDecodeConfig::default();
        i.decode_cfg = Some(&cfg);
        let text = build_evidence(&i);

        assert!(
            text.contains("截断") || text.contains("没有正常 STOP"),
            "截断帧在证据包里消失了 —— 模型会把帧数当成完整事务数\n{text}"
        );
        assert!(
            text.contains("不要把「帧数」当成「完整事务笔数」"),
            "缺失了那条防止误读的说明"
        );

        // ⚠ 上面两条**不足以**证明告警没被过滤：帧表的「⚠被窗口截断」和
        // `count_truncated()` 走的是 `t.complete`，是**独立于 warnings 的另一条路径**。
        // 真把 `d.warnings` 换成 `vmodel::warning_lines()`（那个会滤掉
        // TruncatedFrame 的版本）上面两条照样过。
        //
        // 所以再钉一句**只有未过滤的告警才会产生**的原文 —— 这条才真正守住坑 1。
        assert!(
            text.contains("采集窗口在帧中间结束，最后一帧不完整"),
            "告警原文被过滤掉了。这就是把 `I2cDecode::warnings` 错换成 \
             `vmodel::warning_lines()` 的症状\n{text}"
        );
    }

    #[test]
    fn complete_frames_do_not_get_a_truncation_note() {
        let cap = i2c_capture(true);
        let scale = ChannelScale::default();
        let d = decode_capture(&cap, &I2cDecodeConfig::default()).expect("应能解码");
        assert!(
            d.transactions.iter().all(|t| t.complete),
            "测试数据里还有没收尾的帧 —— 这条测试失去意义"
        );

        let mut i = input(&cap, &scale);
        i.decode = Some(&d);
        let cfg = I2cDecodeConfig::default();
        i.decode_cfg = Some(&cfg);
        let text = build_evidence(&i);

        assert!(
            !text.contains("不要把「帧数」当成「完整事务笔数」"),
            "全收尾的数据不该出现截断说明（否则是天天响的假警报）\n{text}"
        );
    }

    // ── 坑 2 的回归：通道是用户选的就要说 ────────────────────────────

    #[test]
    fn user_selected_channels_are_disclosed() {
        let cap = i2c_capture(true);
        let scale = ChannelScale::default();
        let d = decode_capture(&cap, &I2cDecodeConfig::default()).unwrap();
        let cfg = I2cDecodeConfig::new(1, 0); // 故意反接，验证打印的是配置值
        let mut i = input(&cap, &scale);
        i.decode = Some(&d);
        i.decode_cfg = Some(&cfg);
        let text = build_evidence(&i);

        assert!(text.contains("SCL=ch1"), "应打印配置里的通道：{text}");
        assert!(text.contains("SDA=ch0"), "应打印配置里的通道：{text}");
        assert!(
            text.contains("用户在界面上选"),
            "必须说明通道是用户选的而不是自动检测的"
        );
    }

    // ── 坑 3：长度上限 ───────────────────────────────────────────────

    #[test]
    fn oversize_output_is_truncated_with_a_marker() {
        let huge = "x".repeat(MAX_CHARS + 500);
        let out = truncate_to_cap(huge);
        assert!(out.contains("截断后的内容"), "超限必须带标记，不能静默截断");
        assert!(
            out.chars().count() <= MAX_CHARS + 200,
            "截断后还是太长：{}",
            out.chars().count()
        );
    }

    #[test]
    fn under_cap_output_is_left_alone() {
        let small = "x".repeat(MAX_CHARS - 1);
        assert_eq!(
            truncate_to_cap(small).chars().count(),
            MAX_CHARS - 1,
            "没超限就不该动它"
        );
    }

    /// 真实采集渲染出来必须远低于上限 —— 否则「省略号」会天天出现。
    #[test]
    fn a_real_capture_fits_comfortably() {
        let cap = i2c_capture(true);
        let scale = ChannelScale::default();
        let d = decode_capture(&cap, &I2cDecodeConfig::default()).unwrap();
        let cfg = I2cDecodeConfig::default();
        let mut i = input(&cap, &scale);
        i.decode = Some(&d);
        i.decode_cfg = Some(&cfg);
        let text = build_evidence(&i);

        assert!(
            text.chars().count() < MAX_CHARS,
            "一份普通采集就超了上限（{} 字符）—— 上限定错了或哪里膨胀了",
            text.chars().count()
        );
    }

    // ── 桶数的**真实**上界 ───────────────────────────────────────────

    /// 桶数是「约」PREVIEW_POINTS，不是「最多」。
    ///
    /// `Capture::preview` 里 `bucket = (len / target).max(1)`，所以
    /// `len` 落在 `[target, 2*target)` 时 bucket 退化成 1，桶数能到 `2*target - 1`。
    /// 这条测试把这个**实际行为**钉住 —— 免得以后有人在文档里
    /// 写一个会被真实数据推翻的上限（我一开始就写错了）。
    #[test]
    fn preview_bucket_count_is_at_most_twice_the_target() {
        let scale = ChannelScale::default();

        for len in [64usize, 127, 128, 129, 255, 256, 300, 512, 1024, 4096] {
            let mut cap = Capture::new(1, 800_000, 1, len as u32);
            cap.channels = vec![(0..len).map(|i| (i % 4096) as u16).collect()];
            let i = input(&cap, &scale);
            let text = build_evidence(&i);

            // 从文本里把实际桶数读回来
            let buckets = cap.preview(0, PREVIEW_POINTS).unwrap().y_min.len();
            assert!(
                buckets <= 2 * PREVIEW_POINTS,
                "len={len} 时桶数 {buckets} 超过 2×目标（{})",
                2 * PREVIEW_POINTS
            );
            assert!(text.contains("波形包络"), "len={len} 时没有包络段");
        }
    }

    /// 4096 点（硬件上限）时桶数应当**正好**是目标值 —— 这是最常见的情形。
    #[test]
    fn full_capture_hits_the_target_bucket_count_exactly() {
        let mut cap = Capture::new(1, 857_142, 1, 4096);
        cap.channels = vec![(0..4096).map(|i| (i % 4096) as u16).collect()];
        assert_eq!(
            cap.preview(0, PREVIEW_POINTS).unwrap().y_min.len(),
            PREVIEW_POINTS,
            "4096 点应当整除出正好 {PREVIEW_POINTS} 桶"
        );
    }

    // ── 模拟器声明 ───────────────────────────────────────────────────

    #[test]
    fn simulator_data_is_declared_loudly() {
        let cap = flat_capture();
        let scale = ChannelScale::default();

        let mut sim = input(&cap, &scale);
        sim.simulated = true;
        let sim_text = build_evidence(&sim);
        assert!(sim_text.contains("【模拟器】"), "模拟器标志必须出现");
        assert!(
            sim_text.contains("不能") && sim_text.contains("真实硬件"),
            "必须说清模拟器数据的结论不能推及真机"
        );

        let mut real = input(&cap, &scale);
        real.simulated = false;
        real.link = "COM7 @ 921600";
        let real_text = build_evidence(&real);
        assert!(
            !real_text.contains("【模拟器】"),
            "真机数据不该被标成模拟器"
        );
    }

    // ── 测不出就写测不出，不写 0 ─────────────────────────────────────

    /// 项目纪律：宁可说不知道，也不给看似精确的垃圾数。
    /// 直流信号没有周期 —— 频率必须写「测不出」，不能写 `0 Hz`。
    #[test]
    fn unmeasurable_metrics_say_so_rather_than_zero() {
        let cap = flat_capture();
        let scale = ChannelScale::default();
        let text = build_evidence(&input(&cap, &scale));

        assert!(text.contains("测不出"), "直流信号应报「测不出」：{text}");
        assert!(
            !text.contains("频率=0.0 Hz"),
            "测不出不能写成 0 Hz —— 那读起来像「测到了，是 0」"
        );
    }

    // ── 溢出必须显式标出 ─────────────────────────────────────────────

    #[test]
    fn overrun_is_flagged() {
        let mut cap = flat_capture();
        let scale = ChannelScale::default();
        assert!(
            !build_evidence(&input(&cap, &scale)).contains("溢出"),
            "没溢出就不该提溢出"
        );

        cap.overrun = true;
        let text = build_evidence(&input(&cap, &scale));
        assert!(text.contains("溢出"), "溢出了必须标出来");
        assert!(
            text.contains("不完整"),
            "还要说清后果：数据不完整，结论不可信"
        );
    }

    // ── 确定性与纯函数性 ─────────────────────────────────────────────

    /// 同输入必须逐字节同输出。这条是**反例不存在**型的断言 ——
    /// 它抓的是「输出里混进了时间戳/随机数/地址」这类不稳定。
    #[test]
    fn same_input_gives_byte_identical_output() {
        let cap = i2c_capture(true);
        let scale = ChannelScale::default();
        let d = decode_capture(&cap, &I2cDecodeConfig::default()).unwrap();
        let cfg = I2cDecodeConfig::default();

        let render = || {
            let mut i = input(&cap, &scale);
            i.decode = Some(&d);
            i.decode_cfg = Some(&cfg);
            build_evidence(&i)
        };
        assert_eq!(render(), render(), "同样的输入渲染出了不同的文本");
    }

    // ── 用户的问题进得去 ─────────────────────────────────────────────

    #[test]
    fn the_question_is_included_but_blank_is_ignored() {
        let cap = flat_capture();
        let scale = ChannelScale::default();

        let mut with_q = input(&cap, &scale);
        with_q.question = Some("这段总线有什么问题？");
        assert!(build_evidence(&with_q).contains("这段总线有什么问题？"));

        let mut blank = input(&cap, &scale);
        blank.question = Some("   \n  ");
        assert!(
            !build_evidence(&blank).contains("用户的问题"),
            "空白问题不该产生一个空的提问段"
        );
    }

    // ── 没解码时也要说得清 ───────────────────────────────────────────

    #[test]
    fn missing_decode_explains_itself() {
        let cap = flat_capture(); // 只有 1 个真通道有数据，且没解
        let scale = ChannelScale::default();
        let text = build_evidence(&input(&cap, &scale));
        assert!(
            text.contains("没有解码结果"),
            "没解码就要说没解码，不能整段消失"
        );
    }
}
