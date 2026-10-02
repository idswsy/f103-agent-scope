//! 各个面板的绘制。
//!
//! 每个 `draw_*` 都是自由函数 `(&mut App, &mut Ui)`，不持有状态 ——
//! 状态全在 [`crate::app::App`] 里，绘制只读它、并按需发 `Request`。

use crate::app::App;
use crate::msg::{Request, TransportKind};
use crate::vmodel::{self, SpanKind};
use egui_plot::{HLine, Line, Plot, Polygon, Text, VLine};
use scope_core::{state_name, ChannelScale, State};
use scope_sim::Scenario;

/// 顶部工具条：采集 / 停止 / 复位。
pub fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.strong("F103 Agent Scope");
        ui.separator();

        let busy = app.is_busy();
        let connected = app.connected;

        if ui
            .add_enabled(connected && !busy, egui::Button::new(acq_label(busy)))
            .clicked()
        {
            app.worker.send(Request::Acquire {
                samples: app.want_samples,
                rate_hz: app.want_rate,
                trigger_level_lsb: app.want_trigger_level,
                timeout_ms: 2000,
            });
        }

        if ui
            .add_enabled(busy, egui::Button::new("取消"))
            .on_hover_text("中断正在等待的触发（最长要等 2 秒）")
            .clicked()
        {
            app.worker.cancel();
        }

        if ui
            .add_enabled(connected && !busy, egui::Button::new("复位"))
            .on_hover_text("RESET。设备进入 Fault 态后这是唯一的出路")
            .clicked()
        {
            app.worker.send(Request::Reset);
        }

        ui.separator();
        ui.checkbox(&mut app.show_volts, "显示电压")
            .on_hover_text("关闭则显示 ADC LSB 整数（控制链路的原生单位）");

        ui.separator();
        if ui
            .selectable_label(app.show_help, "帮助")
            .on_hover_text("这台设备能做什么、做不到什么 —— 建议先看一眼")
            .clicked()
        {
            app.show_help = !app.show_help;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(st) = app.state {
                let color = match st {
                    State::Fault => egui::Color32::RED,
                    State::Armed | State::Streaming => egui::Color32::YELLOW,
                    _ => egui::Color32::GRAY,
                };
                ui.colored_label(color, state_name(st));
            } else {
                ui.colored_label(egui::Color32::GRAY, "未连接");
            }
        });
    });
}

fn acq_label(busy: bool) -> &'static str {
    if busy {
        "采集中…"
    } else {
        "采集一次"
    }
}

/// 左侧：设备与连接。
pub fn device(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("设备");

    ui.horizontal(|ui| {
        ui.selectable_value(&mut app.transport_kind, TransportKind::Sim, "模拟器");
        ui.selectable_value(&mut app.transport_kind, TransportKind::Serial, "串口");
    });

    match app.transport_kind {
        TransportKind::Sim => {
            ui.horizontal(|ui| {
                ui.label("场景");
                egui::ComboBox::from_id_salt("scenario")
                    .selected_text(app.scenario.name())
                    .show_ui(ui, |ui| {
                        for s in [
                            Scenario::I2c100k,
                            Scenario::I2c400k,
                            Scenario::Sine1k3v3,
                            Scenario::Square50k,
                            Scenario::PulseGlitch,
                            Scenario::Noise,
                            Scenario::Dc,
                            Scenario::Am,
                        ] {
                            ui.selectable_value(&mut app.scenario, s, s.name());
                        }
                    });
            });
            if app.connected && ui.button("切换场景").clicked() {
                app.worker.send(Request::SetScenario(app.scenario));
            }
        }
        TransportKind::Serial => {
            ui.horizontal(|ui| {
                ui.label("端口");
                let text = if app.port.is_empty() {
                    "（先点刷新）".to_string()
                } else {
                    app.port.clone()
                };
                egui::ComboBox::from_id_salt("port")
                    .selected_text(text)
                    .show_ui(ui, |ui| {
                        let ports = app.ports.clone();
                        for (name, desc, likely) in ports {
                            let label = if likely {
                                format!("{name}  ← 疑似目标")
                            } else {
                                format!("{name}  {desc}")
                            };
                            ui.selectable_value(&mut app.port, name, label);
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("波特率");
                ui.add(egui::DragValue::new(&mut app.baud).speed(100));
            });
        }
    }

    ui.horizontal(|ui| {
        let busy = app.is_busy();
        // 「刷新端口」只对串口有意义，模拟器模式下不该占位置
        if app.transport_kind == TransportKind::Serial && ui.button("刷新端口").clicked() {
            app.worker.send(Request::ListPorts);
        }
        if app.connected {
            if ui.add_enabled(!busy, egui::Button::new("断开")).clicked() {
                app.worker.send(Request::Disconnect);
            }
        } else if ui.add_enabled(!busy, egui::Button::new("连接")).clicked() {
            app.worker.send(Request::Connect {
                transport: app.transport_kind,
                port: app.port.clone(),
                baud: app.baud,
                scenario: app.scenario,
            });
        }
    });

    if let Some(info) = &app.info {
        ui.separator();
        egui::Grid::new("info").num_columns(2).show(ui, |ui| {
            ui.label("固件");
            ui.label(format!(
                "{}.{}.{}",
                (info.fw_ver >> 16) & 0xFF,
                (info.fw_ver >> 8) & 0xFF,
                info.fw_ver & 0xFF
            ));
            ui.end_row();
            ui.label("通道数");
            ui.label(info.ch_count.to_string());
            ui.end_row();
            ui.label("采样率上限");
            // 857142，不是 1000000 —— docs/04-performance.md:35 写死了这条
            ui.label(format!("{} Hz", info.rate_max_hz));
            ui.end_row();
            ui.label("单次上限");
            ui.label(format!("{} 点", info.capture_max_samples));
            ui.end_row();
        });
    }
}

/// 左侧：采集与触发配置。
///
/// 用 `Grid` 而不是一排 `ui.horizontal`：两列对齐后左标签、右控件各成一条线。
/// 控件宽度统一成 [`CTRL_W`] —— combo / 数字框 / 滑块三种控件的默认宽度各不相同，
/// 并排时右边缘参差不齐。
pub fn config(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("配置");
    let editable = app.connected && !app.is_busy();

    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("cfg_grid")
            .num_columns(2)
            .spacing([10.0, 7.0])
            .show(ui, |ui| {
                ui.label("采样率");
                egui::ComboBox::from_id_salt("rate")
                    .width(CTRL_W)
                    .selected_text(format!("{} Hz", app.want_rate))
                    .show_ui(ui, |ui| {
                        for r in vmodel::rate_choices() {
                            ui.selectable_value(&mut app.want_rate, r, format!("{r} Hz"));
                        }
                    });
                ui.end_row();

                ui.label("点数");
                ui.add_sized(
                    [CTRL_W, 20.0],
                    egui::DragValue::new(&mut app.want_samples).range(16..=4096),
                );
                ui.end_row();

                ui.label("模式");
                egui::ComboBox::from_id_salt("trig_mode")
                    .width(CTRL_W)
                    .selected_text(match app.want_trigger_mode {
                        0 => "auto",
                        2 => "single",
                        _ => "normal",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut app.want_trigger_mode, 0, "auto");
                        ui.selectable_value(&mut app.want_trigger_mode, 1, "normal");
                        ui.selectable_value(&mut app.want_trigger_mode, 2, "single");
                    });
                ui.end_row();

                ui.label("边沿");
                egui::ComboBox::from_id_salt("trig_edge")
                    .width(CTRL_W)
                    .selected_text(if app.want_trigger_edge == 0 {
                        "上升沿"
                    } else {
                        "下降沿"
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut app.want_trigger_edge, 0, "上升沿");
                        ui.selectable_value(&mut app.want_trigger_edge, 1, "下降沿");
                    });
                ui.end_row();

                ui.label("触发电平");
                ui.spacing_mut().slider_width = CTRL_W - 58.0;
                ui.add(egui::Slider::new(&mut app.want_trigger_level, 0..=4095).suffix(" LSB"));
                ui.end_row();
            });

        ui.add_space(2.0);
        ui.small(format!(
            "触发比较带 ±{} LSB 迟滞",
            scope_core::f103::TRIGGER_HYSTERESIS_LSB
        ));

        // 请求值 vs 设备回显值 —— 采样率会被定时器分频量化，差异必须摆在明面上
        if let Some(cfg) = &app.config {
            if cfg.rate_hz != app.want_rate {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 190, 90),
                    format!("↳ 设备实际生效 {} Hz", cfg.rate_hz),
                );
            }
        }

        ui.add_space(4.0);
        if ui.button("应用到设备").clicked() {
            app.worker.send(Request::ApplyConfig {
                rate_hz: app.want_rate,
                samples: app.want_samples,
                trigger_mode: app.want_trigger_mode,
                trigger_source: 0,
                trigger_edge: app.want_trigger_edge,
                trigger_level_lsb: app.want_trigger_level,
                pre_samples: (app.want_samples / 2).min(2048),
                holdoff_us: 1000,
            });
        }
    });

    if !app.connected {
        ui.small("未连接");
    } else if app.is_busy() {
        ui.small("操作进行中，配置暂不可改");
    }
}

/// 配置面板里控件的统一宽度。
const CTRL_W: f32 = 150.0;

/// 左侧：模拟器故障注入。**只在真的连着模拟器时出现。**
///
/// 这些开关的价值在于：错误路径（丢帧、CRC 错、不触发、溢出）在真实硬件上
/// 很难复现，而它们恰恰是最容易写错的地方。
pub fn faults(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("故障注入");
    ui.weak("模拟器专用。用来验证错误路径不会把界面卡死。");

    let f = &mut app.faults;
    egui::Grid::new("fault_grid")
        .num_columns(2)
        .spacing([10.0, 5.0])
        .show(ui, |ui| {
            ui.label("丢帧");
            ui.add(
                egui::DragValue::new(&mut f.drop_every_n_frames)
                    .range(0..=100u32)
                    .suffix(" 帧"),
            );
            ui.end_row();

            ui.label("CRC 错");
            ui.add(
                egui::DragValue::new(&mut f.crc_err_every_n_frames)
                    .range(0..=100u32)
                    .suffix(" 帧"),
            );
            ui.end_row();

            ui.label("延迟尖峰概率");
            ui.add(egui::DragValue::new(&mut f.latency_spike_probability).range(0.0..=1.0));
            ui.end_row();

            ui.label("尖峰时长");
            ui.add(
                egui::DragValue::new(&mut f.latency_spike_ms)
                    .range(0..=5000u64)
                    .suffix(" ms"),
            );
            ui.end_row();
        });

    ui.checkbox(&mut f.no_trigger, "永不触发")
        .on_hover_text("验证「等触发超时」这条路不会卡死界面");
    ui.checkbox(&mut f.force_overrun, "强制溢出")
        .on_hover_text("验证界面会如实标记数据不完整，而不是假装正常");

    ui.horizontal(|ui| {
        if ui.button("注入").clicked() {
            app.worker
                .send(Request::SetFaults(Box::new(app.faults.clone())));
        }
        if ui.button("清除全部").clicked() {
            app.faults = scope_sim::FaultInjection::default();
            app.worker
                .send(Request::SetFaults(Box::new(app.faults.clone())));
        }
    });
}

/// 中央区：能力边界。
///
/// 写成**规格文档**的样子，不是"给你讲解" —— 客观陈述、表格化、不用
/// emoji、不用第二人称。文档要求这些限制「必须写进 UI 与预期」，
/// 目的是让人查得到，不是让人读得感动。
pub fn help_page(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.heading("能力边界");
        if ui.button("← 返回波形").clicked() {
            app.show_help = false;
        }
    });
    ui.separator();

    egui::ScrollArea::vertical()
        .id_salt("help_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("caps")
                .num_columns(2)
                .spacing([18.0, 4.0])
                .show(ui, |ui| {
                    ui.strong("测量能力");
                    ui.label("");
                    ui.end_row();
                    for (k, v) in [
                        ("I2C 协议解码", "100 kHz / 400 kHz / 1 MHz"),
                        ("模拟带宽", "≤ 100 kHz"),
                        ("采样率", "双通道同步 857 kSPS / 通道"),
                        ("单次采集深度", "4096 点"),
                        ("触发电平迟滞", "±16 LSB"),
                    ] {
                        ui.label(format!("    {k}"));
                        ui.monospace(v);
                        ui.end_row();
                    }
                });

            ui.add_space(10.0);
            ui.strong("不支持");
            ui.add_space(2.0);
            // 用普通列表而不是 Grid —— 这两列里第一列是空的，Grid 会把
            // 各行的缩进按最宽单元格对齐，首行反而跟别的行对不齐。
            for v in [
                "1 MSPS 与 USB 并存",
                "深存储 / 外扩 SRAM",
                "带宽 > 0.5 MHz",
                "I2C 时序合规性验证（tSU;DAT / tr 的 ns 级判定）",
                "计量级测量、THD / SFDR、12-bit 绝对精度",
                "程控量程（量程切换为机械开关）",
            ] {
                ui.label(format!("    {v}"));
            }

            ui.add_space(10.0);
            ui.strong("注意事项");
            ui.add_space(2.0);
            // ⚠ 这些字符串**不要用 `\` 续行**：续行后源码里的缩进会原样
            // 进到字符串里，渲染出来每句中间凭空多一大段空白。
            let notes = [
                "采样率上限为 857 142 Hz。设备按 72 MHz 定时器整数分频得到档位，请求值会被量化；时间轴一律以设备回显的实际值为准。",
                "电压换算使用未标定的占位参数（3.3 V / 4096、零点 2048）。接入实机后需按设备 UID 标定。",
                "上升时间的分辨率下限为 1 个采样周期，更快的边沿无法分辨。",
                "频率与占空比已排除事务之间的空闲区间；非周期信号（如数据线）报出的是边沿速率。",
                "协议解码走数字通路（LM393 比较器 + 定时器输入捕获，13.9 ns 分辨率），与 ADC 采样率无关；ADC 通路负责信号质量评估。两条通路互补：前者回答协议是否正确，后者回答信号是否良好。",
            ];
            for t in notes {
                ui.label(format!("    · {t}"));
            }

            ui.add_space(14.0);
            ui.separator();
            ui.weak(format!(
                "scope-gui {}  ·  协议 v1  ·  设计与性能数据见 docs/",
                scope_core::VERSION
            ));
        });
}

/// 左侧：历史采集（最近 16 次，容量由 core 的 CaptureStore 决定）。
pub fn history(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("历史采集");
    if app.store.is_empty() {
        ui.weak("还没有采集。采集一次后会留在这里，点一下就能回看。");
        return;
    }

    let current = app.capture.as_ref().map(|c| c.id);
    let entries: Vec<(u16, u32, u64, bool)> = app
        .store
        .iter()
        .map(|c| (c.id, c.rate_hz, c.wall_time, c.overrun))
        .collect();

    egui::ScrollArea::vertical()
        .id_salt("hist")
        .max_height(120.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (id, rate, _t, overrun) in entries {
                let label = format!("#{id}  {} Hz{}", rate, if overrun { "  ⚠溢出" } else { "" });
                if ui.selectable_label(current == Some(id), label).clicked() {
                    // 回看历史：换掉当前采集并重算解码，**不重抓**
                    if let Some(c) = app.store.get(id) {
                        app.capture = Some(c.clone());
                        app.mark_decode_dirty();
                        app.fit_pending = true;
                    }
                }
            }
        });
}

/// 左侧：滚动日志。
pub fn log(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("日志");
    egui::ScrollArea::vertical()
        .id_salt("log_scroll")
        .max_height(150.0)
        .auto_shrink([false, true])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in &app.log {
                ui.small(line);
            }
        });
    if ui.small_button("清空").clicked() {
        app.log.clear();
    }
}

/// 底部状态栏。
pub fn status_bar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        if let Some((text, hint)) = &app.last_error {
            ui.colored_label(egui::Color32::RED, format!("✗ {text}"));
            if let Some(h) = hint {
                ui.separator();
                ui.colored_label(egui::Color32::LIGHT_RED, h);
            }
        } else if let Some(cap) = &app.capture {
            ui.label(format!(
                "{} 点 · {} Hz · {:.3} ms",
                cap.len(),
                cap.rate_hz,
                cap.duration_us() as f64 / 1000.0
            ));
            // 电压是未标定的换算（ChannelScale::default 是占位值）。
            // 这句必须放在**一定看得见**的地方 —— y 轴标签会被
            // show_axes([true,false]) 一起藏掉，放那儿等于没写。
            if app.show_volts {
                ui.separator();
                ui.colored_label(egui::Color32::from_rgb(180, 150, 90), "⚠ 电压未标定")
                    .on_hover_text(
                        "电压用的是 ChannelScale::default() 这个占位换算                          （3.3 V / 4096、零点 2048），不是标定值。                         模拟器的 I2C 电平是 0.15/0.85·VDD，按这个换算是 ±1.15 V；                         真机上要按设备 uid 存标定表才对得上。",
                    );
            }
            if cap.overrun {
                ui.colored_label(
                    egui::Color32::RED,
                    "⚠ 溢出：这份数据不完整，不能当完整波形用",
                );
            }
            if cap.trigger_index.is_none() {
                ui.colored_label(egui::Color32::YELLOW, "· 无触发点（软触发）");
            }
        } else if let Some(n) = &app.font_notice {
            ui.colored_label(egui::Color32::YELLOW, format!("⚠ {n}"));
        } else {
            ui.weak("就绪 —— 选模拟器 + i2c_100k，点「连接」再「采集一次」");
        }
    });
}

/// 中央：波形 + 解码泳道。
///
/// # 为什么每个通道占独立泳道
///
/// I2C 的 SCL 与 SDA 都是 0→3.3V 的数字信号，画在同一个坐标系里**完全重合**，
/// 根本分不出哪条是哪条。所以按通道纵向错开成"泳道"（示波器的 stacked 模式），
/// 每条泳道内部仍然是真实的电压刻度，VIH/VIL 参考线画在各自泳道里。
///
/// # 为什么默认不显示全部 4096 点
///
/// 4.778 ms 的窗口里塞了约 478 个 SCL 周期，摊在 770 px 上每周期只有 1.6 像素 ——
/// 全览时就是一团竖条纹，看不出任何形状。所以默认只显示十几个周期，
/// 鼠标滚轮可以自由缩放。
pub fn plot(app: &mut App, ui: &mut egui::Ui) {
    let Some(cap) = app.capture.clone() else {
        ui.centered_and_justified(|ui| {
            ui.weak("还没有数据。左侧选「模拟器」+ 场景 i2c_100k，点「连接」，再点「采集一次」。");
        });
        return;
    };

    let ch_count = cap.channels.len().max(1);
    let scale = ChannelScale::default();

    // 操作提示与重置按钮。egui_plot 的缩放/平移是内置的，但**用户不知道** ——
    // 不给提示的话没人会去滚轮。另外缩进去之后没有出口，得给个按钮。
    ui.horizontal(|ui| {
        if ui.small_button("重置缩放").clicked() {
            app.fit_pending = true;
        }
        ui.checkbox(&mut app.lanes_overlap, "泳道重叠")
            .on_hover_text("打开后两条泳道同基线，看时序错开更直观");
        ui.weak("· 滚轮缩放 · 拖拽平移 · 双击重置");
    });

    // ── 布局常量（虚拟 y 坐标，不是电压）──
    const LANE_H: f64 = 3.2; // 一条泳道的高度（信号本身 0~2.8V，留点余量）
    const LANE_GAP: f64 = 0.7; // 泳道间隔
    const DECODE_H: f64 = 1.7; // 底部解码带的高度

    // 重叠模式：所有泳道同基线 —— 看两路信号**时序上的错开**更直观
    // （比如 SDA 在 SCL 高电平期间变化 = START/STOP）。
    // 分开模式：纵向错开，看各自的波形形状更清楚。
    let lane_step = if app.lanes_overlap {
        0.0
    } else {
        LANE_H + LANE_GAP
    };
    let lane_bottom = |i: usize| -> f64 { -(i as f64) * lane_step };
    // 解码带要贴在**最后一条泳道的下方**。
    // 回归：这里曾经写成 `lane_bottom(ch_count)`，那是"再往下一条泳道"的位置，
    // 于是 2 通道时中间凭空多出 4.9 个单位的空白。
    // 只有多于一条泳道时才需要解码带；单通道时它纯属占地方
    let show_band = ch_count >= 2;
    let decode_top = lane_bottom(ch_count - 1) - LANE_GAP;
    let decode_bottom = decode_top - DECODE_H;

    let to_v = |lsb: u16| scale.lsb_to_volts(lsb);
    let show_volts = app.show_volts;

    // ── 各通道折线 ──
    let palette = [
        egui::Color32::from_rgb(120, 200, 255),
        egui::Color32::from_rgb(255, 190, 120),
        egui::Color32::from_rgb(160, 255, 160),
        egui::Color32::from_rgb(230, 160, 255),
    ];
    let mut series: Vec<(String, Vec<[f64; 2]>, egui::Color32)> = Vec::new();
    let mut lane_labels: Vec<(String, f64)> = Vec::new();
    for ch in 0..ch_count {
        let base = lane_bottom(ch);
        // 几何一律用电压 —— 泳道里保持真实电压刻度。
        // 「显示电压」开关只换**标签单位**，不换形状：
        // 之前关掉时把 y 算成 volts/3.3*LANE_H，那既不是伏特也不是 LSB，
        // 是个没有物理意义的缩放。
        let pts: Vec<[f64; 2]> = vmodel::waveform_points(&cap, ch, &scale)
            .into_iter()
            .map(|p| [p[0], base + p[1]])
            .collect();

        // 泳道左侧的标签：给出这条泳道的**真实**电压（或 LSB）范围。
        // 纵向错开之后 y 刻度不再等于电压，所以这个标签是唯一的读数来源。
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for p in &pts {
            lo = lo.min(p[1]);
            hi = hi.max(p[1]);
        }
        let span_txt = if lo.is_finite() && hi > lo {
            let (lo_v, hi_v) = (lo - base, hi - base);
            if show_volts {
                format!("CH{}  {lo_v:.2}–{hi_v:.2} V", ch + 1)
            } else {
                format!(
                    "CH{}  {}–{} LSB",
                    ch + 1,
                    scale.volts_to_lsb(lo_v),
                    scale.volts_to_lsb(hi_v)
                )
            }
        } else if lo.is_finite() {
            // 直流：范围零宽，直接报那一个电平
            let v = lo - base;
            if show_volts {
                format!("CH{}  {v:.2} V", ch + 1)
            } else {
                format!("CH{}  {} LSB", ch + 1, scale.volts_to_lsb(v))
            }
        } else {
            format!("CH{}", ch + 1)
        };
        // 重叠模式下所有泳道同基线，标签会叠在同一个位置互相盖住 ——
        // 按通道号纵向错开。分开模式下各泳道本来就不在一处，居中即可。
        let label_y = if app.lanes_overlap {
            LANE_H * (0.42 - ch as f64 * 0.20)
        } else {
            base + LANE_H * 0.5
        };
        lane_labels.push((span_txt, label_y));

        series.push((format!("CH{}", ch + 1), pts, palette[ch % palette.len()]));
    }

    // ── 每条泳道内的 VIH / VIL 参考线 ──
    let (vih_v, vil_v) = {
        let l = app.decode_cfg.levels;
        (to_v(l.vih_lsb), to_v(l.vil_lsb))
    };
    let mut thresholds: Vec<([f64; 2], egui::Color32)> = Vec::new();
    for ch in 0..ch_count {
        let base = lane_bottom(ch);
        let c = egui::Color32::from_gray(80);
        thresholds.push(([base + vih_v, base + vil_v], c));
    }

    // ── 解码色块，只画在底部专用带里，不覆盖波形 ──
    let total_us = vmodel::duration_us(&cap);
    let spans = app
        .decode
        .as_ref()
        .map(|d| vmodel::event_spans(d, total_us))
        .unwrap_or_default();
    let span_polys: Vec<(Vec<[f64; 2]>, egui::Color32)> = spans
        .iter()
        .map(|s| {
            let color = span_color(s.kind, s.nack);
            (
                vec![
                    [s.t0_us, decode_bottom],
                    [s.t1_us, decode_bottom],
                    [s.t1_us, decode_top],
                    [s.t0_us, decode_top],
                ],
                color,
            )
        })
        .collect();

    // ── 默认时间窗口：正好装下**第一笔完整事务** ──
    //
    // 一笔 I2C 事务（START + 3 字节 + STOP）要 29 个 SCL 周期 ≈ 290 µs @100 kHz。
    // 按"周期数"取窗口会只能看到 1.5 个事件，色块颜色都来不及变；
    // 按"一笔事务"取，才是 I2C 调试真正要看的最小单位。
    let span_us = app
        .decode
        .as_ref()
        .and_then(|d| d.transactions.first())
        .map(|tx| {
            let last = tx.events.last().map(|e| e.time_us).unwrap_or(0) as f64;
            (last - tx.start_time_us as f64) * 1.15 + 20.0
        })
        .or_else(|| {
            // 还没解出事务时，退回"约 15 个 SCL 周期"
            app.decode
                .as_ref()
                .and_then(|d| d.quality.scl_freq_hz)
                .map(|f| 15.0 * 1_000_000.0 / f as f64)
        })
        // 没有解码结果（非 I2C 信号）：按上升沿反推周期，显示约 4 个周期。
        // 回归：这里曾经硬取 total_us * 0.03，把 1 kHz 正弦显示成了一条直线。
        .unwrap_or_else(|| vmodel::suggest_span_us(&cap))
        .min(total_us.max(1.0))
        .max(1.0);

    let trig_x = cap
        .trigger_index
        .map(|i| i as f64 * 1_000_000.0 / cap.rate_hz.max(1) as f64);

    let y_top = lane_bottom(0) + LANE_H + 0.4;
    let y_bot = if show_band {
        decode_bottom - 0.3
    } else {
        lane_bottom(ch_count - 1) - 0.3
    };

    let fit = app.fit_pending;
    app.fit_pending = false;

    let mut p = Plot::new("scope")
        .x_axis_label("时间 (µs)")
        // 泳道纵向错开之后，y 刻度对第 2 条以后的泳道**不再是电压**
        // （CH1 恰好是，CH2 整体下移）。留着刻度会被误读，所以关掉 y 刻度，
        // 改由每条泳道左侧的标签给出真实电压范围。
        .show_axes([true, false])
        // 十字准线的 y 读数在泳道模式下不是电压（CH2 起整体下移了），
        // 留着会被当成电压读。时间信息由交易表和横轴给。
        .show_crosshair(false)
        // 电压是**未标定**的换算：`ChannelScale::default()` 是个占位值
        // （3.3V/4096、零点 2048），真机上要按 uid 存标定表才对得上。
        // 不标出来的话，等于把一个猜测当成测量值展示 —— 违反项目那条
        // 「宁可说不知道，也不给看似精确的垃圾数」。
        .y_axis_label("泳道（电压未标定）")
        .legend(egui_plot::Legend::default())
        .allow_scroll(false)
        .default_y_bounds(y_bot, y_top);

    // 新采集时贴合到默认窗口；鼠标滚轮可自由缩放
    p = if fit {
        p.reset().default_x_bounds(0.0, span_us)
    } else {
        p
    };

    p.show(ui, move |pui| {
        for (pts, color) in span_polys {
            pui.polygon(Polygon::new("", pts).fill_color(color));
        }
        for ([hi, lo], color) in thresholds {
            pui.hline(HLine::new("VIH", hi).color(color));
            pui.hline(HLine::new("VIL", lo).color(color));
        }
        if show_band {
            for y in [decode_top, decode_bottom] {
                pui.hline(HLine::new("", y).color(egui::Color32::from_gray(55)));
            }
        }
        for (name, pts, color) in series {
            pui.line(Line::new(name, pts).color(color));
        }
        for (text, y) in lane_labels {
            // 锚点必须显式写成左对齐：egui_plot 的 `Text` 默认是
            // `Align2::CENTER_CENTER`，文本以给定点为中心，左半截会伸到
            // 绘图区外被裁掉 —— 「CH1」那三个字就是这么凭空消失的。
            pui.text(
                Text::new("", egui_plot::PlotPoint::new(span_us * 0.02, y), text)
                    .anchor(egui::Align2::LEFT_CENTER),
            );
        }
        if let Some(x) = trig_x {
            pui.vline(VLine::new("触发", x).color(egui::Color32::from_rgb(255, 80, 80)));
        }
    });
    // 色标**不在这里**画 —— `Plot::show` 会吃掉所有可用高度，
    // 跟在它后面的行会被压成零高，只漏出一排残影。
    // 唯一的那份在下面 `decode_panel` 里。
}

/// 色块不透明版 —— 图例里用。
///
/// 数据带上的色块是半透明的（压在黑色绘图区上），直接拿来做图例会发灰，
/// 跟用户在图上看到的对不上。所以图例取同样的色相、拉满不透明度。
fn span_color_solid(kind: SpanKind, nack: bool) -> egui::Color32 {
    let c = span_color(kind, nack);
    egui::Color32::from_rgb(c.r(), c.g(), c.b())
}

/// 画一个「色块 + 文字」的图例项。
///
/// 不用 `allocate_exact_size` 配 `ui.small` 的组合 —— egui 的横向布局会把
/// 两者各自按当前行高对齐，结果是**色块浮在文字上沿**（实测偏 3~4 px）。
/// 这里把两者的位置**都从同一个 `rect` 的中心线算出来**，从根上杜绝错位。
///
/// `swatch = None` 时只画文字（用于行首的「色标」两个字）。
fn legend_item(ui: &mut egui::Ui, swatch: Option<egui::Color32>, text: &str) {
    const SW: f32 = 14.0; // 色块宽
    const SH: f32 = 11.0; // 色块高
    const GAP: f32 = 5.0; // 色块与文字的间距
    const ROW: f32 = 17.0; // 单项占的行高，固定值才可预期

    let color = ui.visuals().text_color();
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_owned(), egui::FontId::proportional(11.5), color);

    let lead = if swatch.is_some() { SW + GAP } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(lead + galley.size().x, ROW),
        egui::Sense::hover(),
    );
    let cy = rect.center().y;

    if let Some(c) = swatch {
        ui.painter().rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + SW / 2.0, cy),
                egui::vec2(SW, SH),
            ),
            2.0,
            c,
        );
    }
    ui.painter().galley(
        egui::pos2(rect.left() + lead, cy - galley.size().y / 2.0),
        galley,
        color,
    );
}

/// 解码色标 —— 告诉人「黄蓝绿紫分别是什么」。
///
/// 不放进 egui_plot 的图例里：那样会和 CH1/CH2/VIH/VIL/触发 挤成一长条，
/// 而且数据带上的色块是半透明的，图例里的色块看起来对不上色。
fn span_legend(ui: &mut egui::Ui) {
    let items: [(SpanKind, bool, &str); 6] = [
        (SpanKind::Start, false, "起始位"),
        (SpanKind::Address, false, "地址"),
        (SpanKind::Data, false, "数据"),
        (SpanKind::Stop, false, "停止位"),
        (SpanKind::Data, true, "NACK"),
        (SpanKind::Truncated, false, "截断"),
    ];

    ui.horizontal_wrapped(|ui| {
        legend_item(ui, None, "色标");
        ui.add_space(8.0);
        for (kind, nack, label) in items {
            legend_item(ui, Some(span_color_solid(kind, nack)), label);
            ui.add_space(10.0);
        }
    });
}

fn span_color(kind: SpanKind, nack: bool) -> egui::Color32 {
    if nack {
        return egui::Color32::from_rgba_unmultiplied(255, 70, 70, 150);
    }
    // 事件跨度是首尾相接的，色块连成一片。不透明度压得太低会糊成一色，
    // 所以这里给到 130~150 —— 既压得住深色背景，又能分出相邻事件。
    match kind {
        SpanKind::Start => egui::Color32::from_rgba_unmultiplied(255, 200, 60, 150),
        SpanKind::Address => egui::Color32::from_rgba_unmultiplied(70, 150, 255, 140),
        SpanKind::Data => egui::Color32::from_rgba_unmultiplied(70, 210, 110, 120),
        SpanKind::Stop => egui::Color32::from_rgba_unmultiplied(200, 120, 255, 150),
        SpanKind::Truncated => egui::Color32::from_rgba_unmultiplied(200, 200, 200, 110),
    }
}

/// 底部面板：解码结果（参数、质量、告警、交易表）。
pub fn detail_panel(app: &mut App, ui: &mut egui::Ui) {
    measure_section(app, ui);
    ui.separator();
    decode_panel(app, ui);
}

/// 测量结果 —— 每个通道一行。
fn measure_section(app: &mut App, ui: &mut egui::Ui) {
    let Some(cap) = app.capture.clone() else {
        return;
    };
    let scale = ChannelScale::default();

    ui.horizontal(|ui| {
        ui.strong("测量");
        if ui
            .button("导出 CSV")
            .on_hover_text("全量样点落盘（不进界面、不占内存）")
            .clicked()
        {
            let path = capture_path(&cap, "csv");
            let scales: Vec<ChannelScale> = (0..cap.channels.len()).map(|_| scale).collect();
            match std::fs::write(&path, cap.to_csv(&scales)) {
                Ok(()) => app.note(format!("已写出 {path}")),
                Err(e) => app.note(format!("✗ 写 CSV 失败：{e}")),
            }
        }
        if app.decode.is_some() && ui.button("导出解码结果").clicked() {
            let text = app.decode.as_ref().map(|d| d.to_text()).unwrap_or_default();
            let path = capture_path(&cap, "i2c.txt");
            match std::fs::write(&path, text) {
                Ok(()) => app.note(format!("已写出 {path}")),
                Err(e) => app.note(format!("✗ 写解码结果失败：{e}")),
            }
        }
    });

    for ch in 0..cap.channels.len() {
        let Some(m) = vmodel::measure(&cap, ch, &scale) else {
            continue;
        };
        let freq = m
            .freq_hz
            .map(|f| format!("{:.1} kHz", f / 1000.0))
            .unwrap_or_else(|| "—".into());
        let duty = m
            .duty_pct
            .map(|d| format!("{d:.1}%"))
            .unwrap_or_else(|| "—".into());
        let rise = m
            .rise_ns
            .map(|r| format!("{r:.0} ns"))
            .unwrap_or_else(|| "—".into());
        ui.monospace(format!(
            "CH{:<2}  Vpp {:>6.2} V   均值 {:>6.2} V   有效值 {:>6.2} V   频率 {:>10}   占空比 {:>6}   上升 {:>8}",
            ch + 1,
            m.vpp,
            m.mean,
            m.ac_rms,
            freq,
            duty,
            rise
        ));
    }
    // 这几个边界必须写出来，否则数字会被过度解读：
    // 「有效值」是扣除直流后的 RMS（交流信号看这个）；频率/占空比排除了
    // 事务间的空闲（否则 100 kHz 会被算成 95 kHz）；上升时间受采样率限制；
    // 数据线的「频率」只是跳变速率，不是时钟。
    let dt_ns = if cap.rate_hz > 0 {
        1e9 / cap.rate_hz as f64
    } else {
        0.0
    };
    ui.weak(format!(
        "有效值 = 扣除直流后的 RMS · 占空比按 min/max 中值判定（与解码门限无关）\
         · 频率与占空比已排除事务间空闲 · 上升时间分辨率下限 1 个采样周期（{dt_ns:.0} ns）\
         · 非周期信号（数据线）的「频率」只是边沿速率，参考意义有限"
    ));
}

/// 生成导出路径：`<当前目录>/captures/<时间>_<id>.<后缀>`。
///
/// 不用文件对话框（`rfd` 在 Linux 上要拖 GTK 进 CI）；固定目录 + 把完整路径
/// 写进日志，用户知道文件去哪了。
fn capture_path(cap: &scope_core::Capture, ext: &str) -> String {
    let dir = std::path::PathBuf::from("captures");
    let _ = std::fs::create_dir_all(&dir);
    // 用采集编号 + 采样率命名，不用墙钟（避免依赖时间函数，也便于复现）
    let name = format!("cap{}_{}Hz.{}", cap.id, cap.rate_hz, ext);
    dir.join(name).to_string_lossy().into_owned()
}

fn decode_panel(app: &mut App, ui: &mut egui::Ui) {
    // ★ 色标的**唯一**一份在这里。
    // 不要同时在 `plot()` 末尾再画一次：`Plot::show` 会吃掉所有可用高度，
    // 那一份会被压成零高、只漏出一排残影，看起来像界面错位了。
    span_legend(ui);

    ui.horizontal(|ui| {
        ui.heading("I2C 解码");

        let has_cap = app.capture.is_some();
        let ch_count = app.capture.as_ref().map(|c| c.channels.len()).unwrap_or(0);

        ui.separator();
        // 用下拉框而不是 `DragValue`。后者有个坑：它的 range 在**渲染时就会夹值**，
        // 而 `add_enabled(false)` 挡不住 —— 没采集时 ch_count=0，范围算成 0..=0，
        // 会把 sda_channel 从 1 悄悄夹成 0，两条线都指向同一条通道，
        // 界面看着正常却永远解不出东西。下拉框只可能设成合法值，从根上没有这个问题。
        let names: Vec<String> = (0..ch_count.max(1))
            .map(|i| format!("CH{}", i + 1))
            .collect();
        let label_of =
            |idx: usize| -> String { names.get(idx).cloned().unwrap_or_else(|| "—".into()) };

        ui.label("SCL");
        ui.add_enabled_ui(has_cap, |ui| {
            egui::ComboBox::from_id_salt("scl_ch")
                .width(64.0)
                .selected_text(label_of(app.scl_channel))
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut app.scl_channel, i, n);
                    }
                });
        });
        ui.label("SDA");
        ui.add_enabled_ui(has_cap, |ui| {
            egui::ComboBox::from_id_salt("sda_ch")
                .width(64.0)
                .selected_text(label_of(app.sda_channel))
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        ui.selectable_value(&mut app.sda_channel, i, n);
                    }
                });
        });

        if ui
            .add_enabled(ch_count >= 2, egui::Button::new("自动检测"))
            .on_hover_text("按边沿密度判断哪条是 SCL —— 通道接反时用")
            .clicked()
        {
            let picked = app.capture.as_ref().and_then(|cap| {
                scope_core::detect_channels(cap, app.decode_cfg.levels, app.decode_cfg.debounce_ns)
            });
            match picked {
                Some((a, b)) => {
                    app.scl_channel = a;
                    app.sda_channel = b;
                    app.mark_decode_dirty();
                    app.note(format!("自动检测：SCL=CH{} SDA=CH{}", a + 1, b + 1));
                }
                None => app.note("自动检测失败：两条线都没有边沿"),
            }
        }
        ui.label("去抖");
        ui.add(
            egui::DragValue::new(&mut app.decode_cfg.debounce_ns)
                .range(0..=10_000u32)
                .speed(10)
                .suffix(" ns"),
        );

        if ui.button("重新解码").clicked() {
            app.mark_decode_dirty();
        }

        if let Some(d) = &app.decode {
            ui.separator();
            if d.is_untrustworthy() {
                ui.colored_label(egui::Color32::RED, "结果不可信");
            }
            ui.label(format!("共 {} 帧", d.frame_count()));
        }
    });

    let Some(d) = app.decode.clone() else {
        ui.weak("尚无解码结果。采集一次后会自动解码。");
        return;
    };

    // 信号质量
    ui.horizontal_wrapped(|ui| {
        let q = &d.quality;
        ui.label(format!("SCL 边沿 {}", q.scl_edges));
        if let Some(f) = q.scl_freq_hz {
            ui.separator();
            ui.label(format!("频率 {:.1} kHz", f as f64 / 1000.0));
        }
        if let Some(duty) = q.scl_duty {
            ui.separator();
            ui.label(format!("占空比 {:.1}%", duty * 100.0));
        }
        ui.separator();
        ui.label(format!("判决带样点 {}", q.unknown_scl + q.unknown_sda));
    });

    for w in vmodel::warning_lines(&d) {
        ui.colored_label(egui::Color32::YELLOW, format!("⚠ {w}"));
    }

    ui.separator();
    egui::ScrollArea::vertical()
        .id_salt("tx_table")
        // 横向不收缩 —— 否则 ScrollArea 只占内容那么宽，
        // 滚动条会站在面板正中间（看起来像一根乱入的竖线）
        .auto_shrink([false, true])
        .show(ui, |ui| {
            // num_columns 必须与「每行实际放几个单元格」一致。
            // 写成 2 但每行只放一个，斑马纹会按两列铺，行首行尾各冒出一块空底色。
            // 不用 Grid —— 列对齐已经由 `transaction_summary` 的 format! 做好了，
            // 而 Grid 的斑马纹按单元格宽度铺，行长不一时会只盖住右半截。
            for line in vmodel::transaction_summary(&d) {
                ui.monospace(line);
            }
        });
}
