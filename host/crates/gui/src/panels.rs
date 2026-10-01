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
                transport: app.transport_kind.clone(),
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

    // ── 布局常量（虚拟 y 坐标，不是电压）──
    const LANE_H: f64 = 3.2; // 一条泳道的高度（信号本身 0~2.8V，留点余量）
    const LANE_GAP: f64 = 0.7; // 泳道间隔
    const DECODE_H: f64 = 1.7; // 底部解码带的高度

    let lane_bottom = |i: usize| -> f64 { -(i as f64) * (LANE_H + LANE_GAP) };
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
        lane_labels.push((span_txt, base + LANE_H * 0.5));

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
pub fn decode_panel(app: &mut App, ui: &mut egui::Ui) {
    // ★ 色标的**唯一**一份在这里。
    // 不要同时在 `plot()` 末尾再画一次：`Plot::show` 会吃掉所有可用高度，
    // 那一份会被压成零高、只漏出一排残影，看起来像界面错位了。
    span_legend(ui);

    ui.horizontal(|ui| {
        ui.heading("I2C 解码");

        let has_cap = app.capture.is_some();
        let ch_count = app.capture.as_ref().map(|c| c.channels.len()).unwrap_or(0);

        // ⚠ 上下限不能用 `ch_count - 1` 直接算：还没有采集时 ch_count = 0，
        // 范围就成了 0..=0，`DragValue` 会**静默把 sda_channel 从 1 夹成 0**
        // ——`add_enabled(false)` 挡不住这个。结果两条线都指向通道 0，解码出 0 帧。
        // 所以没数据时给一个宽松范围，只做显示、不做约束。
        let ch_max = if ch_count > 0 {
            (ch_count - 1) as u32
        } else {
            3
        };

        ui.separator();
        ui.label("SCL");
        ui.add_enabled(
            has_cap,
            egui::DragValue::new(&mut app.scl_channel).range(0..=ch_max as usize),
        );
        ui.label("SDA");
        ui.add_enabled(
            has_cap,
            egui::DragValue::new(&mut app.sda_channel).range(0..=ch_max as usize),
        );
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
