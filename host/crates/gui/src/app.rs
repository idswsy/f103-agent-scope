//! 应用状态与布局。
//!
//! # 状态放哪
//!
//! | 状态 | 位置 | 理由 |
//! |---|---|---|
//! | `CommandBus` / 设备真值快照 | worker | 阻塞 IO 只能有一个所有者 |
//! | 采集结果 `CaptureStore` | **本线程** | `Capture: Clone` 很便宜，UI 每帧要随机访问；放 worker 侧就要加锁，而锁一旦落在 worker 手里，2 s 等待期间 UI 会被卡住 |
//! | 解码结果 | 本线程，脏标记缓存 | 4096 点解码很快，但没必要每帧重算 |
//! | 「期望值」与「回显值」 | **分开存** | 采样率会被量化，界面必须能画出「请求 vs 实际」的差异 |

use crate::font::FontOutcome;
use crate::msg::{Request, TransportKind, Update};
use crate::panels;
use crate::worker::Worker;
use scope_core::{
    Capture, CaptureStore, DeviceConfig, DeviceInfo, I2cDecode, I2cDecodeConfig, State,
};
use scope_sim::Scenario;
use std::sync::atomic::{AtomicBool, Ordering};

/// 应用主状态。
pub struct App {
    /// worker 句柄（所有阻塞 IO 的去处）。
    pub(crate) worker: Worker,

    // ── 连接意图 ──
    /// 传输选择。
    pub(crate) transport_kind: TransportKind,
    /// 串口名。
    pub(crate) port: String,
    /// 波特率。
    pub(crate) baud: u32,
    /// 模拟器场景。
    pub(crate) scenario: Scenario,
    /// 已枚举到的串口。
    pub(crate) ports: Vec<(String, String, bool)>,

    // ── 设备真值（全部来自设备回显）──
    /// 设备能力。
    pub(crate) info: Option<DeviceInfo>,
    /// 设备当前配置（**回显值**）。
    pub(crate) config: Option<DeviceConfig>,
    /// 设备当前状态。
    pub(crate) state: Option<State>,
    /// 是否已连接。
    pub(crate) connected: bool,

    // ── 期望配置（UI 意图，与真值分开）──
    /// 请求的采样率。
    pub(crate) want_rate: u32,
    /// 请求的点数。
    pub(crate) want_samples: u16,
    /// 触发模式 `0=auto / 1=normal / 2=single`。
    pub(crate) want_trigger_mode: u8,
    /// 触发边沿 `0=上升 / 1=下降`。
    pub(crate) want_trigger_edge: u8,
    /// 触发电平（ADC LSB）。
    pub(crate) want_trigger_level: u16,

    // ── 采集与解码 ──
    /// 当前显示的采集。
    pub(crate) capture: Option<Capture>,
    /// 历史采集（最近 16 条，容量由 core 决定）。
    pub(crate) store: CaptureStore,
    /// 解码结果。
    pub(crate) decode: Option<I2cDecode>,
    /// 解码参数。
    pub(crate) decode_cfg: I2cDecodeConfig,
    /// 解码是否需要重算。
    decode_dirty: AtomicBool,
    /// 当前 SCL/SDA 通道选择。
    pub(crate) scl_channel: usize,
    pub(crate) sda_channel: usize,

    // ── UI 态 ──
    /// 正在进行的操作名。
    pub(crate) busy: Option<&'static str>,
    /// 最后一次错误：`(说明, 怎么办)`。
    pub(crate) last_error: Option<(String, Option<String>)>,
    /// 滚动日志。
    pub(crate) log: Vec<String>,
    /// 字体加载结果（没找到时要在界面上说一声）。
    pub(crate) font_notice: Option<String>,
    /// 下次绘制时把图重置到全览。
    pub(crate) fit_pending: bool,
    /// 显示电压还是 LSB。
    pub(crate) show_volts: bool,
    /// `--demo` 的状态机：0=未开始 1=已发连接 2=已发采集。
    demo_stage: u8,
}

impl App {
    /// 建一个 App 并挂上 worker。
    pub fn new(
        ctx: &egui::Context,
        font: FontOutcome,
        demo: bool,
        scenario: Option<Scenario>,
    ) -> App {
        let worker = Worker::spawn(ctx.clone());
        // 首启预选模拟器 + I2C 场景 —— 零硬件的人双击就能看到东西
        worker.send(Request::ListPorts);

        let mut app = App {
            worker,
            transport_kind: TransportKind::Sim,
            port: String::new(),
            baud: 921_600,
            scenario: scenario.unwrap_or(Scenario::I2c100k),
            ports: Vec::new(),

            info: None,
            config: None,
            state: None,
            connected: false,

            want_rate: scope_core::f103::MAX_SAMPLE_RATE_HZ,
            want_samples: 4096,
            want_trigger_mode: 1, // normal
            want_trigger_edge: 0, // 上升
            want_trigger_level: 2048,

            capture: None,
            store: CaptureStore::default(),
            decode: None,
            decode_cfg: I2cDecodeConfig::default(),
            decode_dirty: AtomicBool::new(false),
            scl_channel: 0,
            sda_channel: 1,

            busy: None,
            last_error: None,
            log: Vec::new(),
            font_notice: font.notice(),
            fit_pending: false,
            show_volts: true,
            demo_stage: if demo { 0 } else { 3 },
        };

        // 字体去向写进日志 —— 加载成功也留一条，方便排查「为什么中文是方框」
        match &font {
            FontOutcome::Loaded { path } => app.note(format!("界面字体：{path}")),
            FontOutcome::NotFound { .. } => app.note("⚠ 未找到中文字体，中文会显示为方框"),
        }
        app
    }

    /// 记一行日志。
    pub(crate) fn note(&mut self, line: impl Into<String>) {
        self.log.push(line.into());
        // 只留最近 200 行 —— 日志面板是给人扫的，不是归档
        if self.log.len() > 200 {
            self.log.drain(..self.log.len() - 200);
        }
    }

    /// 是否有长操作在进行。
    pub(crate) fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// 处理一条 worker 回执。
    fn apply(&mut self, u: Update) {
        match u {
            Update::Ports(list) => {
                if self.port.is_empty() {
                    // 预选第一个疑似目标
                    if let Some((name, _, true)) = list.iter().find(|p| p.2) {
                        self.port = name.clone();
                    }
                }
                self.ports = list;
            }

            Update::Connected {
                info,
                config,
                state,
            } => {
                self.note(format!(
                    "已连接 {} 通道 / 上限 {} Hz",
                    info.ch_count, info.rate_max_hz
                ));
                self.info = Some(info);
                self.config = config;
                self.state = Some(state);
                self.connected = true;
                self.last_error = None;
            }

            Update::Disconnected => {
                self.note("已断开");
                self.connected = false;
                self.info = None;
                self.config = None;
                self.state = None;
            }

            Update::ConfigApplied(cfg) => {
                // 回显值才是真值。采样率被量化时，这里就是用户能看到差异的地方。
                if cfg.rate_hz != self.want_rate {
                    self.note(format!(
                        "采样率被量化：请求 {} Hz → 实际 {} Hz",
                        self.want_rate, cfg.rate_hz
                    ));
                }
                self.config = Some(*cfg);
            }

            Update::Acquired(cap) => {
                let cap = *cap;
                let mut msg = format!(
                    "采集完成：{} 点 @ {} Hz（{:.3} ms）",
                    cap.len(),
                    cap.rate_hz,
                    cap.duration_us() as f64 / 1000.0
                );
                if cap.overrun {
                    msg.push_str("  ⚠ 发生溢出，数据不完整");
                }
                if cap.trigger_index.is_none() {
                    msg.push_str("  · 无触发点（软触发）");
                }
                self.note(msg);
                // 这次采集成功了，上一次的错误就不再适用 —— 不清的话，
                // 之前那条红字会一直挂在底栏
                self.last_error = None;
                self.fit_pending = true;
                self.store.push(cap.clone());
                self.capture = Some(cap);
                self.decode_dirty.store(true, Ordering::Relaxed);
            }

            Update::StateChanged(s) => {
                self.state = Some(s);
            }

            Update::Busy { op, active } => {
                self.busy = if active { Some(op) } else { None };
            }

            Update::Failed { text, hint } => {
                self.note(format!("✗ {text}"));
                self.last_error = Some((text, hint));
            }
        }
    }

    /// 按需重算解码。
    fn refresh_decode(&mut self) {
        if !self.decode_dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let Some(cap) = self.capture.as_ref() else {
            self.decode = None;
            return;
        };

        // I2C 解码至少要两条线。单通道采集（正弦 / 方波 / 直流…）**不是错误**，
        // 只是这个场景没有 I2C 可解 —— 静默跳过。
        // 回归：之前这里会一路走到 decode_capture 报 InvalidParam，
        // 于是用户抓个正弦波都会看到一条红字错误。
        if cap.channels.len() < 2 {
            self.decode = None;
            return;
        }

        let cfg = I2cDecodeConfig {
            scl_channel: self.scl_channel,
            sda_channel: self.sda_channel,
            levels: self.decode_cfg.levels,
            debounce_ns: self.decode_cfg.debounce_ns,
        };
        match scope_core::decode_capture(cap, &cfg) {
            Ok(d) => {
                self.note(format!("解码完成：共 {} 帧", d.frame_count()));
                self.decode = Some(d);
            }
            Err(e) => {
                let (text, hint) = crate::msg::describe_error(&e);
                self.note(format!("✗ 解码失败：{text}"));
                self.last_error = Some((text, hint));
                self.decode = None;
            }
        }
    }

    /// 请求解码重算（参数变了就调它）。
    pub(crate) fn mark_decode_dirty(&mut self) {
        self.decode_dirty.store(true, Ordering::Relaxed);
    }
}

impl eframe::App for App {
    /// 非绘制逻辑。**worker 消息只在这里排空。**
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut pending: Vec<Update> = Vec::new();
        self.worker.drain(|u| pending.push(u));
        for u in pending {
            self.apply(u);
        }

        if self.worker.is_dead() {
            // worker 悄悄死了的话，UI 会永远转圈 —— 这是最坏的用户体验
            self.busy = None;
            if self.last_error.is_none() {
                self.last_error =
                    Some(("后台工作线程已退出".into(), Some("重启应用即可恢复".into())));
            }
        }

        // --demo：先连模拟器，连上了再采一次。
        // 用级联的 stage 而不是一个 bool —— 连接是异步的，得等回执。
        match self.demo_stage {
            0 if !self.is_busy() => {
                self.demo_stage = 1;
                self.worker.send(Request::Connect {
                    transport: TransportKind::Sim,
                    port: String::new(),
                    baud: 921_600,
                    scenario: self.scenario,
                });
            }
            // 连接失败就一直停在 1 —— 不重试，免得每帧刷屏
            1 if self.connected && !self.is_busy() => {
                self.demo_stage = 2;
                self.worker.send(Request::Acquire {
                    samples: self.want_samples,
                    rate_hz: self.want_rate,
                    trigger_level_lsb: self.want_trigger_level,
                    timeout_ms: 2000,
                });
            }
            _ => {}
        }

        self.refresh_decode();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("topbar").show(ui, |ui| panels::toolbar(self, ui));
        egui::Panel::bottom("statusbar").show(ui, |ui| panels::status_bar(self, ui));
        egui::Panel::left("side")
            .resizable(true)
            .default_size(288.0)
            .size_range(240.0..=460.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    panels::device(self, ui);
                    ui.separator();
                    panels::config(self, ui);
                    ui.separator();
                    panels::log(self, ui);
                });
            });
        // 解码区必须自己占一个面板。`Plot::show` 会**吃掉所有可用高度**，
        // 直接跟在它后面的东西会被整个挤出屏幕 —— 交易表和色标就是这么消失的。
        // 解码面板只在真有 I2C 可解的时候出现（≥2 通道）。单通道采集时它
        // 占着 250px 却什么都做不了，白挤波形的高度。
        let ch_count = self.capture.as_ref().map(|c| c.channels.len()).unwrap_or(0);
        if ch_count >= 2 {
            egui::Panel::bottom("decode")
                .resizable(true)
                .default_size(250.0)
                .size_range(110.0..=520.0)
                .show(ui, |ui| panels::decode_panel(self, ui));
        }

        egui::CentralPanel::default().show(ui, |ui| panels::plot(self, ui));
    }

    /// 收尾。注意签名**不带 `glow::Context`** —— eframe 0.36 的默认渲染后端是
    /// wgpu，`glow` 是可选特性，没开时 trait 用的是这个无参版本。
    fn on_exit(&mut self) {
        self.worker.shutdown();
    }
}
