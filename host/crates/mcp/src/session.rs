//! 会话状态与 14 个工具的实现。
//!
//! # 三层 token 防护（硬性，见 `docs/05-roadmap.md`）
//!
//! 一次 4096 点采集 ≈ 8192 字节 ≈ 4096 个 JSON 数字 ≈ 上万 token。
//! 塞给 LLM 既贵又没用 —— Agent 要的是「这里面发生了什么」，不是第 2713 个样点。
//!
//! 1. [`capture`](Session::capture) / [`watch`](Session::watch) 只回**统计量 + 预览**，
//!    预览点数硬顶 [`PREVIEW_MAX`]
//! 2. [`read_waveform`](Session::read_waveform) 分页、默认 [`READ_DEFAULT`]、
//!    硬顶 [`READ_HARD_CAP`]，超出**拒绝**并提示改用 `scope_save_capture`
//! 3. 全量数据只进 capture store 与磁盘，**永不进上下文**
//!
//! # 为什么它不是线程化的
//!
//! MCP 是一问一答：客户端发一个请求，等一个响应。没有 UI 要刷新，
//! 所以直接同步处理即可 —— GUI 那边需要 worker 线程是因为绘制不能阻塞，
//! 这里没有这个问题。

use scope_core::i2c_decode::{decode_capture, detect_channels, I2cDecodeConfig, Levels};
use scope_core::{
    acquire_cancellable, state_name, AcquireParams, Capture, CaptureStore, ChannelScale, Cmd,
    CommandBus, DeviceInfo, ScopeError,
};
use scope_device::{Transport, TransportKind};
use scope_sim::Scenario;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

/// 预览点数上限 —— 三层防护第一层。
const PREVIEW_MAX: usize = 256;
/// `read_waveform` 的默认点数与硬顶 —— 第二层。
const READ_DEFAULT: usize = 512;
const READ_HARD_CAP: usize = 4096;

/// 一次 MCP 会话。
pub struct Session {
    bus: Option<CommandBus<Transport>>,
    store: CaptureStore,
}

/// 工具失败：说明 + 怎么办。
///
/// 用 `ScopeError::summary()` / `hint()` 那一对，而不是 `Display` ——
/// 有四个变体的 `Display` 已经把 hint 拼进正文了，两者一起显示会出现两遍。
#[derive(Debug)]
pub struct ToolError {
    /// 一句话说明。
    pub message: String,
    /// 「怎么办」。
    pub hint: Option<String>,
}

impl From<ScopeError> for ToolError {
    fn from(e: ScopeError) -> Self {
        ToolError {
            message: e.summary(),
            hint: e.hint(),
        }
    }
}

impl From<scope_core::LinkError> for ToolError {
    fn from(e: scope_core::LinkError) -> Self {
        let scope_err = ScopeError::Link(e);
        ToolError {
            message: scope_err.summary(),
            hint: scope_err.hint(),
        }
    }
}

impl From<anyhow::Error> for ToolError {
    fn from(e: anyhow::Error) -> Self {
        ToolError {
            message: e.to_string(),
            hint: None,
        }
    }
}

type R = std::result::Result<Value, ToolError>;

/// 取 `capture_id`。
fn req_capture_id(p: &Value) -> std::result::Result<u16, ToolError> {
    p.get("capture_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ToolError {
            message: "缺少 capture_id".into(),
            hint: Some("先 scope_capture，或用 scope_list_captures 看现存采集".into()),
        })
        .and_then(|raw| {
            u16::try_from(raw).map_err(|_| ToolError {
                message: format!("capture_id={raw} 超出范围（有效值 0..=65535）"),
                hint: Some("用 scope_list_captures 看现存采集的 id".into()),
            })
        })
}

impl Session {
    /// 建一个空会话。
    pub fn new() -> Session {
        Session {
            bus: None,
            store: CaptureStore::default(),
        }
    }

    /// 从已连接的会话里取 bus，没连就报错并给出自救提示。
    fn bus(
        &mut self,
        action: &'static str,
    ) -> std::result::Result<&mut CommandBus<Transport>, ToolError> {
        self.bus.as_mut().ok_or_else(|| ToolError {
            message: format!("尚未连接设备，无法{action}"),
            hint: Some("先调用 scope_connect（不指定 port 时用模拟器）".into()),
        })
    }

    // ══════════════════════════════════════════════════════════
    // 设备
    // ══════════════════════════════════════════════════════════

    /// `scope_list_devices` —— 枚举串口。
    pub fn list_devices(&self) -> R {
        let ports: Vec<Value> = scope_transport_serial::list_ports()
            .into_iter()
            .map(|(name, desc, likely)| {
                json!({ "port": name, "description": desc, "likely_target": likely })
            })
            .collect();
        Ok(json!({
            "ports": ports,
            "note": "不指定 port 调用 scope_connect 时使用内置模拟器，无需硬件",
        }))
    }

    /// `scope_connect` —— 连接设备。
    pub fn connect(&mut self, p: &Value) -> R {
        let kind = match p.get("transport").and_then(|v| v.as_str()) {
            Some(s) => TransportKind::parse(s).ok_or_else(|| ToolError {
                message: format!("transport 只接受 \"serial\" 或 \"sim\"，收到 {s:?}"),
                hint: Some("省略 transport 与 port 即使用模拟器".into()),
            })?,
            // 没给 transport：给了 port 就当串口，否则模拟器
            None => {
                if p.get("port").and_then(|v| v.as_str()).is_some() {
                    TransportKind::Serial
                } else {
                    TransportKind::Sim
                }
            }
        };

        let port = p.get("port").and_then(|v| v.as_str()).unwrap_or("");
        let baud = p.get("baud").and_then(|v| v.as_u64()).unwrap_or(921_600) as u32;
        // 拼错了要报错，不能静默换成默认场景 —— 否则 Agent 以为连的是自己
        // 指定的波形，实际是另一回事。
        let scenario = match p.get("sim_scenario").and_then(|v| v.as_str()) {
            Some(name) => Scenario::parse(name).ok_or_else(|| ToolError {
                message: format!("未知场景 {name:?}"),
                hint: Some(
                    "可选：sine_1k_3v3 / square_50k / pulse_glitch / noise / dc / am /                      i2c_100k / i2c_400k"
                        .into(),
                ),
            })?,
            None => Scenario::I2c100k,
        };

        let dev = Transport::open(kind, port, baud, scenario).map_err(ToolError::from)?;
        let mut bus = CommandBus::new(dev);

        let info = bus.connect().map_err(ToolError::from)?;
        // `connect()` 只发 GET_INFO + GET_CONFIG，**不发 GET_STATUS**。
        // 不补这一次的话 `bus.state` 一直是 None，`guard_config_allowed`
        // 就失去判据 —— 已武装时照样能改配置。
        let state = bus.get_status().unwrap_or(scope_core::State::Idle);
        let cfg = bus.config.clone();

        // 换设备后 capture_id 从 1 重新分配，而 store 按 id 取数 —— 不清空的话
        // 旧采集会**遮蔽**同 id 的新采集，measure / decode / read / save
        // 全部返回过期数据（而且是静默的）。历史本就属于单次设备会话，
        // 换连接即失效。
        self.store = CaptureStore::default();

        let out = json!({
            "link": bus.describe(),
            "simulated": bus.is_simulated(),
            "state": state_name(state),
            "device": info_json(&info),
            "config": cfg.map(|c| config_json(&c)),
        });
        self.bus = Some(bus);
        Ok(out)
    }

    /// `scope_disconnect`。
    pub fn disconnect(&mut self) -> R {
        if let Some(bus) = self.bus.as_mut() {
            let _ = bus.stop();
        }
        self.bus = None;
        Ok(json!({ "disconnected": true }))
    }

    /// `scope_status`。
    pub fn status(&mut self) -> R {
        let bus = self.bus("查询状态")?;
        let state = bus.get_status().map_err(ToolError::from)?;
        Ok(json!({
            "state": state_name(state),
            "config": bus.config.as_ref().map(config_json),
        }))
    }

    /// `scope_configure` —— 一个工具替代 N 个 `set_*`。
    pub fn configure(&mut self, p: &Value) -> R {
        let bus = self.bus("配置设备")?;
        let mut applied = serde_json::Map::new();
        let mut warnings: Vec<String> = Vec::new();

        if let Some(hz) = p.get("sample_rate_hz").and_then(|v| v.as_u64()) {
            let actual = bus.set_sample_rate(hz as u32).map_err(ToolError::from)?;
            if actual as u64 != hz {
                // 回显值是唯一真值 —— 定时器只能出整数分频的档位
                warnings.push(format!(
                    "请求采样率 {hz} Hz 被量化为 {actual} Hz，时间轴以 {actual} Hz 为准"
                ));
            }
            applied.insert("sample_rate_hz".into(), json!(actual));
        }

        if let Some(m) = p.get("mode").and_then(|v| v.as_str()) {
            match m {
                "single" => {
                    applied.insert("mode".into(), json!("single"));
                }
                // 流模式固件侧还没实现（P1 之后的事）。明确说清楚，
                // 而不是静默按单次跑 —— Agent 会以为拿到的是流数据。
                "stream" => warnings.push(
                    "流模式尚未实现（固件侧 STREAMING 状态属 P1+），本次仍按单次采集处理".into(),
                ),
                other => {
                    return Err(ToolError {
                        message: format!("未知 mode {other:?}"),
                        hint: Some("可选 single / stream".into()),
                    })
                }
            }
        }

        // 采集三件套（点数 / 格式 / 抽点）一起下发 —— `SET_ACQ` 是一条命令，
        // 分三次调只会互相覆盖。缺省项沿用设备当前配置，不是硬编码的 0/1。
        let cur_samples = bus.config.as_ref().map(|c| c.capture_samples as u64);
        let cur_decim = bus.config.as_ref().map(|c| c.decimation as u64);
        let cur_format = bus.config.as_ref().map(|c| c.format);
        let want_samples = p.get("capture_samples").and_then(|v| v.as_u64());
        let want_decim = p.get("decimation").and_then(|v| v.as_u64());
        let want_format = p.get("format").and_then(|v| v.as_str());
        if want_samples.is_some() || want_decim.is_some() || want_format.is_some() {
            // 先校验再转换 —— `as u16` 会把 65537 变成 1、131072 变成 0，
            // 后者还会让之后每次采集都「成功但空」。
            let max = scope_core::f103::MAX_CAPTURE_SAMPLES as u64;
            let samples = want_samples.or(cur_samples).unwrap_or(4096);
            if samples == 0 || samples > max {
                return Err(ToolError {
                    message: format!("capture_samples={samples} 超出范围（1..={max}）"),
                    hint: Some("8 KB 环按 u16 折半就是 4096 点，这是 F103 的物理上限".into()),
                });
            }
            let decimation = want_decim.or(cur_decim).unwrap_or(1);
            if decimation == 0 || decimation > 256 {
                return Err(ToolError {
                    message: format!("decimation={decimation} 超出范围（1..=256）"),
                    hint: None,
                });
            }
            let (fmt_name, fmt_id) = match want_format {
                None => ("(未改)", cur_format.unwrap_or(0)),
                Some("raw") => ("raw", 0),
                Some("packed12") => ("packed12", 1),
                Some("minmax") => ("minmax", 2),
                Some(other) => {
                    return Err(ToolError {
                        message: format!("未知 format {other:?}"),
                        hint: Some("可选 raw / packed12 / minmax".into()),
                    })
                }
            };
            bus.set_acq(0, samples as u16, fmt_id, decimation as u16)
                .map_err(ToolError::from)?;
            applied.insert("capture_samples".into(), json!(samples));
            applied.insert("decimation".into(), json!(decimation));
            if fmt_name != "(未改)" {
                applied.insert("format".into(), json!(fmt_name));
            }
        }

        if let Some(t) = p.get("trigger") {
            let mode = t.get("mode").and_then(|v| v.as_str()).unwrap_or("normal");
            let mode_id = match mode {
                "auto" => 0u8,
                "single" => 2,
                _ => 1,
            };
            let edge = match t.get("edge").and_then(|v| v.as_str()) {
                Some("falling") => 1u8,
                _ => 0u8,
            };
            // 电平既可以给 LSB，也可以给伏特（上位机换算 —— MCU 不碰浮点）
            let level_lsb = match (
                t.get("level_lsb").and_then(|v| v.as_u64()),
                t.get("level_v").and_then(|v| v.as_f64()),
            ) {
                // 同样先校验再转换：`as u16` 会把 99999 回绕成 34463
                (Some(l), _) if l <= 4095 => l as u16,
                (Some(l), _) => {
                    return Err(ToolError {
                        message: format!("trigger.level_lsb={l} 超出 12-bit 范围（0..=4095）"),
                        hint: Some("也可以改传 level_v（伏特）".into()),
                    })
                }
                (None, Some(v)) => ChannelScale::default().volts_to_lsb(v).clamp(0, 4095) as u16,
                (None, None) => 2048,
            };
            let source = t.get("source").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
            bus.set_trigger(mode_id, source, edge, level_lsb, 1024, 1000)
                .map_err(ToolError::from)?;
            applied.insert(
                "trigger".into(),
                json!({ "mode": mode, "edge": edge, "source": source, "level_lsb": level_lsb }),
            );
        }

        if p.get("channel").is_some() {
            warnings.push("channel 配置暂未接线上层（固件侧 SET_CHANNEL 已就绪）".into());
        }

        Ok(json!({ "applied": applied, "warnings": warnings }))
    }

    /// `scope_capture` —— 采一次，**只回统计量 + 预览**。
    pub fn capture(&mut self, p: &Value) -> R {
        let timeout_ms = p.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(2000);
        let max_preview = p
            .get("max_preview_points")
            .and_then(|v| v.as_u64())
            .unwrap_or(PREVIEW_MAX as u64)
            .min(PREVIEW_MAX as u64) as usize;

        let cap = self.acquire_raw(timeout_ms)?;
        let out = capture_summary_json(&cap, max_preview);
        self.store.push(cap);
        Ok(out)
    }

    /// 采一次但**不入库**。
    ///
    /// `scope_watch` 一次能采上百次；全塞进容量 16 的 store 会把用户已有的
    /// 采集全部挤出去 —— 连它自己返回的那个 capture_id 都会立刻失效。
    fn acquire_raw(&mut self, timeout_ms: u64) -> std::result::Result<Capture, ToolError> {
        let bus = self.bus("采集")?;
        let samples = bus
            .config
            .as_ref()
            .map(|c| c.capture_samples)
            .unwrap_or(scope_core::f103::MAX_CAPTURE_SAMPLES as u16);
        let params = AcquireParams {
            samples,
            rate_hz: bus.config.as_ref().map(|c| c.rate_hz).unwrap_or(857_142),
            trigger_level_lsb: bus
                .config
                .as_ref()
                .map(|c| c.trigger_level_lsb)
                .unwrap_or(2048),
            timeout: std::time::Duration::from_millis(timeout_ms),
        };
        // MCP 是同步一问一答，没有「用户按停止」这回事，所以取消标志永远是假
        static NEVER: AtomicBool = AtomicBool::new(false);
        acquire_cancellable(bus, &params, &NEVER).map_err(ToolError::from)
    }

    /// `scope_read_waveform` —— 分页拉样点。**超出硬顶直接拒绝**。
    pub fn read_waveform(&mut self, p: &Value) -> R {
        let id = req_capture_id(p)?;
        let ch = p.get("channel").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let start = p.get("start_sample").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let want = p
            .get("count")
            .or_else(|| p.get("max_points"))
            .and_then(|v| v.as_u64())
            .unwrap_or(READ_DEFAULT as u64) as usize;

        // 硬顶：超过就**拒绝**，不做静默截断 —— 静默截断会让 Agent
        // 以为拿到了全部数据。要全量请走 scope_save_capture。
        if want > READ_HARD_CAP {
            return Err(ToolError {
                message: format!("一次最多读 {READ_HARD_CAP} 点，请求了 {want} 点"),
                hint: Some(
                    "分页读取（改 start_sample 多次调用），或改用 scope_save_capture 落盘 \
                     后用别的工具分析 —— 全量数据不进 LLM 上下文。"
                        .into(),
                ),
            });
        }

        let cap = self.store.get(id).ok_or_else(|| ToolError {
            message: format!("找不到采集 {id}"),
            hint: Some("历史只保留最近 16 次；用 scope_list_captures 看现存采集".into()),
        })?;
        let s = cap.samples(ch).ok_or_else(|| ToolError {
            message: format!("采集 {id} 没有通道 {ch}"),
            hint: Some("用 scope_status 看设备的通道数".into()),
        })?;

        // ⚠ `start` 是从未经校验的 u64 转来的，`start + want` 会溢出 ——
        // debug 构建下直接 panic，**整个 MCP server 进程随之死掉**，
        // 后续请求（包括在途的）全部丢失。
        // 回归：`start_sample: 18446744073709551615` 一次就能远程打挂 server（rc=101）。
        if start >= s.len() {
            return Ok(json!({
                "capture_id": id, "channel": ch, "start_sample": start,
                "count": 0, "has_more": false, "samples": [],
                "note": format!("start_sample 超出采集长度（本次共 {} 点）", s.len()),
            }));
        }
        let end = start.saturating_add(want).min(s.len());
        let slice: Vec<u16> = s[start..end].to_vec();
        Ok(json!({
            "capture_id": id,
            "channel": ch,
            "start_sample": start,
            "count": slice.len(),
            "rate_hz": cap.rate_hz,
            "has_more": end < s.len(),
            "samples": slice,
        }))
    }

    /// `scope_measure` —— 主机侧定点测量（精度最高）。
    pub fn measure(&mut self, p: &Value) -> R {
        let id = req_capture_id(p)?;
        let cap = self.store.get(id).ok_or_else(|| ToolError {
            message: format!("找不到采集 {id}"),
            hint: Some("用 scope_list_captures 看现存采集".into()),
        })?;

        // 只测指定通道（省略则全测）
        let only_ch = p
            .get("channel")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        // 只回指定指标（省略则全回）。**拼错的名字要报错** —— 静默忽略会让
        // Agent 以为测过了，实际那一项根本没算。
        const KINDS: [&str; 7] = ["vpp", "min", "max", "mean", "rms", "freq", "duty"];
        let want: Option<Vec<String>> = match p.get("metrics").and_then(|v| v.as_array()) {
            Some(a) => {
                let names: Vec<String> = a
                    .iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect();
                if let Some(bad) = names.iter().find(|n| !KINDS.contains(&n.as_str())) {
                    return Err(ToolError {
                        message: format!("未知指标 {bad:?}"),
                        hint: Some(format!("可用：{}", KINDS.join(" / "))),
                    });
                }
                Some(names)
            }
            None => None,
        };
        let has = |k: &str| match &want {
            None => true,
            Some(w) => w.iter().any(|x| x == k),
        };

        let scale = ChannelScale::default();
        let mut out = serde_json::Map::new();
        for ch in 0..cap.channels.len() {
            if only_ch.is_some_and(|only| ch != only) {
                continue;
            }
            if let Some(m) = scope_core::measure(cap, ch, &scale) {
                let mut row = serde_json::Map::new();
                if has("vpp") {
                    row.insert("vpp_v".into(), json!(m.vpp));
                }
                if has("min") {
                    row.insert("min_v".into(), json!(m.min));
                }
                if has("max") {
                    row.insert("max_v".into(), json!(m.max));
                }
                if has("mean") {
                    row.insert("mean_v".into(), json!(m.mean));
                }
                if has("rms") {
                    row.insert("ac_rms_v".into(), json!(m.ac_rms));
                }
                if has("freq") {
                    row.insert("freq_hz".into(), json!(m.freq_hz));
                }
                if has("duty") {
                    row.insert("duty_pct".into(), json!(m.duty_pct));
                }
                out.insert(format!("ch{ch}"), Value::Object(row));
            }
        }
        Ok(json!({
            "capture_id": id,
            "measurements": out,
            "note": "电压为未标定换算（占位参数）；频率与占空比已排除事务间空闲；\
                     非周期信号（数据线）报出的是边沿速率",
        }))
    }

    /// `scope_i2c_decode` —— 本项目的招牌能力。
    pub fn i2c_decode(&mut self, p: &Value) -> R {
        let id = req_capture_id(p)?;
        let cap = self.store.get(id).ok_or_else(|| ToolError {
            message: format!("找不到采集 {id}"),
            hint: Some("用 scope_list_captures 看现存采集".into()),
        })?;

        if cap.channels.len() < 2 {
            return Err(ToolError {
                message: format!(
                    "I2C 解码至少需要两条线，这次采集只有 {} 个通道",
                    cap.channels.len()
                ),
                hint: Some("换双通道场景（如 --scenario i2c_100k）或换用双通道硬件配置".into()),
            });
        }

        let levels = match (
            p.get("vih_lsb").and_then(|v| v.as_u64()),
            p.get("vil_lsb").and_then(|v| v.as_u64()),
        ) {
            (Some(h), Some(l)) => Levels {
                vih_lsb: h as u16,
                vil_lsb: l as u16,
            },
            _ => Levels::default_ratio(),
        };

        // 通道没指定就自动判定（SCL 的边沿比 SDA 密）
        let (scl, sda) = match (
            p.get("scl_channel").and_then(|v| v.as_u64()),
            p.get("sda_channel").and_then(|v| v.as_u64()),
        ) {
            (Some(a), Some(b)) => (a as usize, b as usize),
            _ => detect_channels(cap, levels, 50).unwrap_or((0, 1)),
        };

        let cfg = I2cDecodeConfig {
            scl_channel: scl,
            sda_channel: sda,
            levels,
            debounce_ns: p.get("debounce_ns").and_then(|v| v.as_u64()).unwrap_or(50) as u32,
        };
        let d = decode_capture(cap, &cfg).map_err(ToolError::from)?;

        // 帧列表按条给出 —— 这是 Agent 真正要看的东西
        let frames: Vec<Value> = d
            .transactions
            .iter()
            .enumerate()
            .map(|(i, tx)| {
                json!({
                    "index": i + 1,
                    "time_ms": tx.start_time_us as f64 / 1000.0,
                    "repeated_start": tx.repeated,
                    "address": tx.address.as_ref().map(|a| json!({
                        "value": a.value,
                        "read": a.read,
                        "ten_bit": a.ten_bit,
                        "acked": a.acked,
                    })),
                    "bytes": tx.bytes,
                    "nack": tx.has_nack(),
                    "complete": tx.complete,
                })
            })
            .collect();

        Ok(json!({
            "capture_id": id,
            "scl_channel": scl,
            "sda_channel": sda,
            "frame_count": d.frame_count(),
            "frames": frames,
            "all_bytes": d.all_bytes(),
            "signal_quality": {
                "scl_edges": d.quality.scl_edges,
                "scl_freq_hz": d.quality.scl_freq_hz,
                "scl_duty": d.quality.scl_duty,
                "unknown_level_samples": d.quality.unknown_scl + d.quality.unknown_sda,
            },
            "warnings": d.warnings.iter().map(|w| w.text()).collect::<Vec<_>>(),
            "trustworthy": !d.is_untrustworthy(),
        }))
    }

    /// `scope_list_captures`。
    pub fn list_captures(&self) -> R {
        let list: Vec<Value> = self
            .store
            .iter()
            .map(|c| {
                json!({
                    "capture_id": c.id,
                    "rate_hz": c.rate_hz,
                    "sample_count": c.len(),
                    "channels": c.channels.len(),
                    "duration_ms": c.duration_us() as f64 / 1000.0,
                    "trigger_index": c.trigger_index,
                    "overrun": c.overrun,
                })
            })
            .collect();
        Ok(json!({ "captures": list, "capacity": scope_core::DEFAULT_HISTORY }))
    }

    /// `scope_save_capture` —— 全量落盘。**数据不进 LLM 上下文。**
    pub fn save_capture(&self, p: &Value) -> R {
        let id = req_capture_id(p)?;
        let path = p
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                message: "缺少 path".into(),
                hint: Some("要写到哪？例如 ./captures/cap1.csv".into()),
            })?;

        let cap = self.store.get(id).ok_or_else(|| ToolError {
            message: format!("找不到采集 {id}"),
            hint: Some("用 scope_list_captures 看现存采集".into()),
        })?;

        // schema 里曾经宣称支持 npy / bin，实际一律写 CSV —— 静默落错格式
        // 比报错危险得多：Agent 会拿一个 CSV 当二进制去解析。
        if let Some(f) = p.get("format").and_then(|v| v.as_str()) {
            if f != "csv" {
                return Err(ToolError {
                    message: format!("暂不支持导出 {f} 格式"),
                    hint: Some("目前只能写 csv；npy / bin 尚未实现".into()),
                });
            }
        }

        let scales: Vec<ChannelScale> = (0..cap.channels.len())
            .map(|_| ChannelScale::default())
            .collect();
        std::fs::write(path, cap.to_csv(&scales)).map_err(|e| ToolError {
            message: format!("写 {path} 失败：{e}"),
            hint: Some("确认目录存在、有写权限".into()),
        })?;

        Ok(json!({
            "saved": path,
            "bytes": std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            "note": "全量样点已落盘。不要在对话里读它 —— 4096 点 ≈ 上万 token",
        }))
    }

    /// `scope_watch` —— 在一段时间里连续单次采集，返回滚动计数与缺口统计。
    ///
    /// **不是设备侧流模式**：固件与模拟器都还没实现 `STREAMING` 状态
    /// （那是 P1 之后的事）。用「重复单次采集」达成同样的目的 ——
    /// Agent 要的是「这一秒里总线上发生过什么」，不是真流。
    ///
    /// 两处与初版不同：
    /// - **未连接当场报错**。初版把一切错误都计进 `gaps`，于是「还没连设备」
    ///   会伪装成「1 秒里 100 万次没触发」，还返回 `isError=false`。
    /// - **中间采集不入库**。初版全部都 push，500 ms 就能把用户已有的采集
    ///   全挤出容量 16 的历史。改为只保留第一次，供 Agent 后续 measure/decode。
    pub fn watch(&mut self, p: &Value) -> R {
        let duration_ms = p
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(1000);
        let max_preview = p
            .get("max_points")
            .and_then(|v| v.as_u64())
            .unwrap_or(128)
            .min(PREVIEW_MAX as u64) as usize;

        if self.bus.is_none() {
            return Err(ToolError {
                message: "尚未连接设备，无法观察".into(),
                hint: Some("先调用 scope_connect".into()),
            });
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(duration_ms);
        let mut captured = 0u32;
        let mut failed = 0u32;
        let mut overruns = 0u32;
        let mut first: Option<Capture> = None;

        while std::time::Instant::now() < deadline {
            match self.acquire_raw(300) {
                Ok(cap) => {
                    captured += 1;
                    if cap.overrun {
                        overruns += 1;
                    }
                    if first.is_none() {
                        first = Some(cap);
                    }
                }
                // 这次没等到触发 —— 总线可能正闲，计入缺口而不是报错
                Err(_) => failed += 1,
            }
        }

        // 只把第一次采集入库，且是**最后**才 push —— 保证它一定还在 store 里
        let sample = match first {
            Some(cap) => {
                let s = capture_summary_json(&cap, max_preview);
                self.store.push(cap);
                Some(s)
            }
            None => None,
        };

        Ok(json!({
            "requested_ms": duration_ms,
            "captures": captured,
            "gaps": failed,
            "overruns": overruns,
            "sample": sample,
            "note": "用重复单次采集实现（设备侧流模式尚未实现），两次采集之间必有间隙；                     gaps 是其中没等到触发的次数。只有第一次采集进了历史，\"sample\" 的                      capture_id 可直接用于 scope_measure / scope_read_waveform。",
        }))
    }

    /// `scope_sim_set_scenario` —— 仅模拟器。
    pub fn sim_set_scenario(&mut self, p: &Value) -> R {
        let name = p
            .get("scenario")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                message: "缺少 scenario".into(),
                hint: Some(
                    "可选：sine_1k_3v3 / square_50k / pulse_glitch / noise / dc / am / \
                     i2c_100k / i2c_400k"
                        .into(),
                ),
            })?;
        let sc = Scenario::parse(name).ok_or_else(|| ToolError {
            message: format!("未知场景 {name:?}"),
            hint: Some("见 scope_debug_raw 之外的场景清单".into()),
        })?;

        let bus = self.bus("切换场景")?;
        let _ = bus.stop();
        match bus.port_mut().sim_mut() {
            Some(sim) => {
                sim.set_scenario(sc);
                Ok(json!({ "scenario": sc.name() }))
            }
            None => Err(ToolError {
                message: "当前连接的不是模拟器".into(),
                hint: Some("这个工具只在 transport=\"sim\" 时注册".into()),
            }),
        }
    }

    /// `scope_debug_raw` —— 开发者逃生门。
    pub fn debug_raw(&mut self, p: &Value) -> R {
        let code = p
            .get("cmd")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError {
                message: "缺少 cmd（命令码）".into(),
                hint: None,
            })? as u16;
        let cmd = Cmd::try_from_u16(code).ok_or_else(|| ToolError {
            message: format!("未知命令码 0x{code:04X}"),
            hint: Some("见 proto/protocol.h 的命令表".into()),
        })?;

        let payload: Vec<u8> = p
            .get("payload_hex")
            .and_then(|v| v.as_str())
            .map(|h| {
                h.split_whitespace()
                    .filter_map(|b| u8::from_str_radix(b, 16).ok())
                    .collect()
            })
            .unwrap_or_default();

        let bus = self.bus("发原始命令")?;
        let resp = bus
            .transaction(cmd, payload, std::time::Duration::from_millis(500))
            .map_err(ToolError::from)?;
        Ok(json!({
            "cmd": format!("{cmd:?}"),
            "seq": resp.seq,
            "flags": resp.flags,
            "payload_hex": resp
                .payload
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" "),
        }))
    }
}

// ══════════════════════════════════════════════════════════════
// JSON 构造辅助
// ══════════════════════════════════════════════════════════════

fn info_json(i: &DeviceInfo) -> Value {
    json!({
        "proto_ver": i.proto_ver,
        "fw_ver": format!("{}.{}.{}", (i.fw_ver >> 16) & 0xFF, (i.fw_ver >> 8) & 0xFF, i.fw_ver & 0xFF),
        "model": format!("0x{:04X}", i.model),
        "channels": i.ch_count,
        "adc_bits": i.adc_bits,
        "rate_max_hz": i.rate_max_hz,
        "capture_max_samples": i.capture_max_samples,
        "preferred_chunk_samples": i.preferred_chunk_samples,
        "max_rx_payload": i.max_rx_payload,
        "max_tx_payload": i.max_tx_payload,
    })
}

fn config_json(c: &scope_core::DeviceConfig) -> Value {
    json!({
        "rate_hz": c.rate_hz,
        "acq_mode": c.acq_mode,
        "capture_samples": c.capture_samples,
        "format": c.format,
        "decimation": c.decimation,
        "trigger_mode": c.trigger_mode,
        "trigger_source": c.trigger_source,
        "trigger_edge": c.trigger_edge,
        "trigger_level_lsb": c.trigger_level_lsb,
    })
}

/// 采集摘要 —— **三层 token 防护的第一层**。
///
/// 只给统计量 + ≤`max_preview` 点的 minmax 预览，绝不回全量样点。
fn capture_summary_json(cap: &Capture, max_preview: usize) -> Value {
    let scale = ChannelScale::default();
    let mut channels = Vec::new();
    for ch in 0..cap.channels.len() {
        let Some(s) = cap.summary(ch) else { continue };
        let m = scope_core::measure(cap, ch, &scale);
        channels.push(json!({
            "channel": ch,
            // 叫 sample_count 而不是 samples：后者读起来像「样点数组」，
            // 而这里装的是数量。名字含糊在 LLM 那边会变成误判。
            "sample_count": s.n_samples,
            "min_lsb": s.min_lsb,
            "max_lsb": s.max_lsb,
            "pp_lsb": s.pp_lsb,
            "mean_lsb": s.mean_lsb,
            "rms_lsb": s.rms_lsb,
            "ac_rms_lsb": s.ac_rms_lsb,
            "rising_edges": s.rising_edges,
            "vpp_v": m.as_ref().map(|m| m.vpp),
            "freq_hz": m.as_ref().and_then(|m| m.freq_hz),
            "duty_pct": m.as_ref().and_then(|m| m.duty_pct),
            "rise_ns": m.as_ref().and_then(|m| m.rise_ns),
        }));
    }

    // 预览只给一条通道 —— minmax 的意义是保住尖峰不被平均掉
    let preview = cap.preview(0, max_preview).map(|p| {
        json!({
            "channel": p.channel,
            "bucket": p.bucket,
            "points": p.y_min.len(),
            "t0_us": p.t0_us,
            "dt_us": p.dt_us,
            "y_min": p.y_min,
            "y_max": p.y_max,
        })
    });

    json!({
        "capture_id": cap.id,
        "rate_hz": cap.rate_hz,
        "sample_count": cap.len(),
        "duration_ms": cap.duration_us() as f64 / 1000.0,
        "trigger_index": cap.trigger_index,
        "overrun": cap.overrun,
        "channels": channels,
        "preview": preview,
        "note": "只给统计量与 minmax 预览。全量样点走 scope_read_waveform（分页）\
                 或 scope_save_capture（落盘）—— 4096 点 ≈ 上万 token，不进上下文。",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 连模拟器 + 采一次，返回会话。
    fn connected() -> Session {
        let mut s = Session::new();
        s.connect(&json!({ "transport": "sim", "sim_scenario": "i2c_100k" }))
            .expect("连模拟器不该失败");
        s
    }

    #[test]
    fn refuses_to_work_before_connecting() {
        let mut s = Session::new();
        let e = s.capture(&json!({})).unwrap_err();
        assert!(e.message.contains("尚未连接"), "实测 {}", e.message);
        assert!(e.hint.is_some(), "错误必须给出「怎么办」");
    }

    #[test]
    fn capture_returns_stats_and_preview_but_not_the_raw_waveform() {
        // 三层 token 防护的第一层：统计量 + ≤256 点预览，绝不回全量样点。
        let mut s = connected();
        let v = s.capture(&json!({})).unwrap();
        assert!(v["sample_count"].as_u64().unwrap() > 1000);
        assert!(!v["channels"].as_array().unwrap().is_empty());
        let pts = v["preview"]["points"].as_u64().unwrap();
        assert!(pts <= PREVIEW_MAX as u64, "预览桶数 {pts} 超过上限");
        // 关键：**响应体积有上界**。
        //
        // 直接按「有没有样点数组」查会误伤吗？不会 —— 但要区分清楚：
        // `preview` 里那两串 256 个数字是**设计要求**（minmax 预览），
        // 真正不能出现的是 4096 点的全量波形。全量波形一进来，响应
        // 至少 2 万字符，所以卡体积比数数组更贴近要保证的性质。
        let size = serde_json::to_string(&v).unwrap().len();
        assert!(
            size < 8000,
            "摘要响应 {size} 字符，太大了 —— 全量波形漏进来了？"
        );
    }

    #[test]
    fn read_waveform_rejects_requests_over_the_hard_cap() {
        // 超硬顶必须**拒绝**，不能静默截断 —— 静默截断会让 Agent
        // 以为拿到了全部数据。要全量请走 save_capture。
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();
        let e = s
            .read_waveform(&json!({ "capture_id": 1, "count": 99999 }))
            .unwrap_err();
        assert!(e.message.contains("4096"), "实测 {}", e.message);
        assert!(
            e.hint.as_deref().unwrap_or("").contains("save_capture"),
            "提示应该指向落盘那条路"
        );
    }

    #[test]
    fn read_waveform_pages() {
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();
        let a = s
            .read_waveform(&json!({ "capture_id": 1, "count": 16 }))
            .unwrap();
        assert_eq!(a["count"], 16);
        assert_eq!(a["has_more"], true);
        let b = s
            .read_waveform(&json!({ "capture_id": 1, "start_sample": 16, "count": 16 }))
            .unwrap();
        assert_ne!(a["samples"], b["samples"], "分页两段不该相同");
    }

    #[test]
    fn i2c_decode_returns_frames() {
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();
        let v = s.i2c_decode(&json!({ "capture_id": 1 })).unwrap();
        assert!(v["frame_count"].as_u64().unwrap() > 0, "应解出帧");
        let f = &v["frames"][0];
        assert_eq!(f["address"]["value"], 0x44, "模拟器的默认事务是 0x44");
        assert_eq!(f["bytes"], json!([0x00, 0x1A]));
        assert_eq!(v["trustworthy"], true);
    }

    #[test]
    fn i2c_decode_on_a_single_channel_capture_says_why() {
        let mut s = Session::new();
        s.connect(&json!({ "transport": "sim", "sim_scenario": "sine_1k_3v3" }))
            .unwrap();
        let _ = s.capture(&json!({})).unwrap();
        let e = s.i2c_decode(&json!({ "capture_id": 1 })).unwrap_err();
        assert!(e.message.contains("两条线"), "实测 {}", e.message);
        assert!(e.hint.is_some());
    }

    #[test]
    fn configure_reports_the_quantized_rate() {
        // SET_* 必须回显实际生效值 —— 定时器只能出整数分频的档位。
        let mut s = connected();
        let v = s.configure(&json!({ "sample_rate_hz": 857_143 })).unwrap();
        assert_eq!(v["applied"]["sample_rate_hz"], 857_142);
        let w = v["warnings"].as_array().unwrap();
        assert!(!w.is_empty(), "量化了就要说");
        assert!(w[0].as_str().unwrap().contains("857142"));
    }

    #[test]
    fn measure_reports_honest_units() {
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();
        let v = s.measure(&json!({ "capture_id": 1 })).unwrap();
        let ch0 = &v["measurements"]["ch0"];
        // 幅值必须是正的 —— 回归：曾经把幅值当电平换算，得到负数
        assert!(ch0["vpp_v"].as_f64().unwrap() > 0.0);
        assert!(ch0["ac_rms_v"].as_f64().unwrap() >= 0.0);
        // SCL 是 100 kHz
        let f = ch0["freq_hz"].as_f64().unwrap();
        assert!((f - 100_000.0).abs() < 500.0, "实测 {f} Hz");
    }

    #[test]
    fn reject_bad_transport_with_a_hint() {
        let mut s = Session::new();
        let e = s.connect(&json!({ "transport": "bogus" })).unwrap_err();
        assert!(e.message.contains("serial"), "实测 {}", e.message);
        assert!(e.hint.is_some());
    }

    #[test]
    fn read_waveform_survives_an_absurd_start_sample() {
        // 回归：start + want 曾溢出 → debug 下 panic → **整个 server 进程死掉**，
        // 后续请求（含在途的）全部丢失。一个合法 JSON 整数就能远程打挂它。
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();
        let v = s
            .read_waveform(&json!({
                "capture_id": 1,
                "start_sample": u64::MAX,
                "count": 1
            }))
            .expect("越界的 start_sample 应返回空页，而不是崩掉");
        assert_eq!(v["count"], 0);
        assert_eq!(v["has_more"], false);
    }

    #[test]
    fn reconnecting_clears_the_capture_store() {
        // 回归：换设备后 capture_id 从 1 重新分配，而 store 按 id 取数 ——
        // 旧采集会**遮蔽**同 id 的新采集，measure/decode/read/save 全部
        // 静默返回过期数据。
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();
        assert_eq!(
            s.list_captures().unwrap()["captures"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        s.connect(&json!({ "transport": "sim", "sim_scenario": "sine_1k_3v3" }))
            .unwrap();
        assert_eq!(
            s.list_captures().unwrap()["captures"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "换连接后历史应清空，否则旧采集会遮蔽同 id 的新采集"
        );
        let _ = s.capture(&json!({})).unwrap();
        let caps = s.list_captures().unwrap();
        let arr = caps["captures"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["channels"], 1, "留下的应是新场景（单通道）的采集");
    }

    #[test]
    fn unknown_sim_scenario_is_rejected_not_silently_replaced() {
        // 静默换成默认场景会让 Agent 以为连的是自己指定的波形
        let mut s = Session::new();
        let e = s
            .connect(&json!({ "transport": "sim", "sim_scenario": "i2c_999k" }))
            .unwrap_err();
        assert!(e.message.contains("未知场景"), "实测 {}", e.message);
        assert!(e.hint.unwrap().contains("i2c_100k"));
    }

    #[test]
    fn capture_samples_is_validated_not_truncated() {
        let mut s = connected();
        // 65537 as u16 == 1：静默回绕会让「采 65537 点」变成「采 1 点」
        let e = s
            .configure(&json!({ "capture_samples": 65537 }))
            .unwrap_err();
        assert!(e.message.contains("65537"), "实测 {}", e.message);
        // 0 点会让每次采集都「成功但空」，状态还会持续污染历史
        assert!(s.configure(&json!({ "capture_samples": 0 })).is_err());
    }

    #[test]
    fn save_capture_refuses_formats_it_cannot_write() {
        // schema 曾宣称支持 npy / bin，实际一律写 CSV
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();
        let p = std::env::temp_dir().join("x.npy");
        let e = s
            .save_capture(&json!({ "capture_id": 1, "path": p.to_string_lossy(), "format": "npy" }))
            .unwrap_err();
        assert!(e.message.contains("npy"), "实测 {}", e.message);
    }

    #[test]
    fn measure_metrics_filter_and_reject_typos() {
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();

        let v = s
            .measure(&json!({ "capture_id": 1, "metrics": ["vpp", "freq"], "channel": 0 }))
            .unwrap();
        let m = &v["measurements"];
        assert!(m.get("ch1").is_none(), "指定 channel=0 时不该回 ch1");
        let ch0 = &m["ch0"];
        assert!(ch0.get("vpp_v").is_some() && ch0.get("freq_hz").is_some());
        assert!(ch0.get("duty_pct").is_none(), "没要的指标不该回");

        // 拼错的名字必须报错 —— 静默忽略会让 Agent 以为测过了
        let e = s
            .measure(&json!({ "capture_id": 1, "metrics": ["freqency"] }))
            .unwrap_err();
        assert!(e.message.contains("freqency"), "实测 {}", e.message);
    }

    #[test]
    fn watch_refuses_to_run_without_a_connection() {
        // 回归：初版把一切错误计进 gaps，于是「还没连设备」伪装成
        // 「1 秒里 100 万次没触发」，还返回 isError=false。
        let mut s = Session::new();
        let e = s.watch(&json!({ "duration_ms": 50 })).unwrap_err();
        assert!(e.message.contains("尚未连接"), "实测 {}", e.message);
    }

    #[test]
    fn watch_does_not_flood_the_capture_store() {
        let mut s = connected();
        let before = s.list_captures().unwrap()["captures"]
            .as_array()
            .unwrap()
            .len();
        let v = s.watch(&json!({ "duration_ms": 400 })).unwrap();
        let after = s.list_captures().unwrap()["captures"]
            .as_array()
            .unwrap()
            .len();
        assert!(
            v["captures"].as_u64().unwrap() > 1,
            "这段时间应采到不止一次"
        );
        assert_eq!(
            after,
            before + 1,
            "watch 只该入库一次（供后续 measure/read），而不是每次都 push"
        );
    }

    #[test]
    fn save_capture_writes_the_file() {
        let mut s = connected();
        let _ = s.capture(&json!({})).unwrap();
        let path = std::env::temp_dir().join("scope-mcp-test.csv");
        let p = path.to_string_lossy().into_owned();
        let v = s
            .save_capture(&json!({ "capture_id": 1, "path": p }))
            .unwrap();
        assert!(v["bytes"].as_u64().unwrap() > 0, "文件应有内容");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("Time(s)"), "CSV 表头不对");
        let _ = std::fs::remove_file(&path);
    }
}
