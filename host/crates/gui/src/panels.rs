//! 各个面板的绘制。
//!
//! 每个 `draw_*` 都是自由函数 `(&mut App, &mut Ui)`，不持有状态 ——
//! 状态全在 [`crate::app::App`] 里，绘制只读它、并按需发 `Request`。

use crate::app::App;
use crate::msg::{Request, TransportKind};
use crate::vmodel::{self, SpanKind};
use egui_plot::{HLine, Line, Plot, Points, Polygon, VLine};
use scope_core::{state_name, ChannelScale, State};
use scope_sim::Scenario;

/// 顶部工具条：采集 / 停止 / 复位。
pub fn toolbar(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.heading("F103 Agent Scope");
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
        if ui.button("刷新端口").clicked() {
            app.worker.send(Request::ListPorts);
        }
        let busy = app.is_busy();
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
pub fn config(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("配置");
    let editable = app.connected && !app.is_busy();

    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            ui.label("采样率");
            let choices = vmodel::rate_choices();
            let text = format!("{} Hz", app.want_rate);
            egui::ComboBox::from_id_salt("rate")
                .selected_text(text)
                .show_ui(ui, |ui| {
                    for r in choices {
                        ui.selectable_value(&mut app.want_rate, r, format!("{r} Hz"));
                    }
                });
        });

        // 请求值 vs 设备回显值 —— 差异必须摆在明面上
        if let Some(cfg) = &app.config {
            if cfg.rate_hz != app.want_rate {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("↳ 设备实际生效：{} Hz", cfg.rate_hz),
                );
            }
        }

        ui.horizontal(|ui| {
            ui.label("点数");
            ui.add(egui::DragValue::new(&mut app.want_samples).range(16..=4096));
        });

        ui.separator();
        ui.label("触发");
        egui::ComboBox::from_id_salt("trig_mode")
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
        egui::ComboBox::from_id_salt("trig_edge")
            .selected_text(if app.want_trigger_edge == 0 {
                "上升沿"
            } else {
                "下降沿"
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut app.want_trigger_edge, 0, "上升沿");
                ui.selectable_value(&mut app.want_trigger_edge, 1, "下降沿");
            });
        ui.add(
            egui::Slider::new(&mut app.want_trigger_level, 0..=4095)
                .text("触发电平 (LSB)")
                // 迟滞带宽写死在 core 里，告诉用户免得他以为触发电平是精确的
                .suffix(format!(
                    "  (±{} LSB 迟滞)",
                    scope_core::f103::TRIGGER_HYSTERESIS_LSB
                )),
        );

        ui.separator();
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
        ui.weak("未连接");
    } else if app.is_busy() {
        ui.weak("操作进行中，配置暂不可改");
    }
}

/// 左侧：滚动日志。
pub fn log(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("日志");
    egui::ScrollArea::vertical()
        .id_salt("log_scroll")
        .max_height(180.0)
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
pub fn plot(app: &mut App, ui: &mut egui::Ui) {
    let Some(cap) = app.capture.clone() else {
        ui.centered_and_justified(|ui| {
            ui.weak("还没有数据。左侧选「模拟器」+ 场景 i2c_100k，点「连接」，再点「采集一次」。");
        });
        return;
    };

    let ch_count = cap.channels.len();
    let scale = ChannelScale::default();

    // y 轴的显示范围：从全量数据里取，留 10% 余量
    let (mut y_lo, mut y_hi) = (f64::MAX, f64::MIN);
    let mut series: Vec<(String, Vec<[f64; 2]>, egui::Color32)> = Vec::new();
    let palette = [
        egui::Color32::from_rgb(120, 200, 255),
        egui::Color32::from_rgb(255, 190, 120),
        egui::Color32::from_rgb(160, 255, 160),
        egui::Color32::from_rgb(230, 160, 255),
    ];

    for ch in 0..ch_count {
        let pts = vmodel::waveform_points(&cap, ch, &scale);
        let plot_pts: Vec<[f64; 2]> = pts
            .iter()
            .map(|p| {
                let y = if app.show_volts {
                    p[1]
                } else {
                    p[1] / scale.volts_per_lsb
                };
                y_lo = y_lo.min(y);
                y_hi = y_hi.max(y);
                [p[0], y]
            })
            .collect();
        series.push((
            format!("CH{}", ch + 1),
            plot_pts,
            palette[ch % palette.len()],
        ));
    }
    if !y_lo.is_finite() || !y_hi.is_finite() || y_hi <= y_lo {
        y_lo = 0.0;
        y_hi = 1.0;
    }
    let pad = (y_hi - y_lo) * 0.1;
    let (band_lo, band_hi) = (y_lo - pad, y_hi + pad);

    // 解码色块：整高半透明竖带，颜色区分事件类型
    let spans = app
        .decode
        .as_ref()
        .map(|d| vmodel::event_spans(d, vmodel::duration_us(&cap)))
        .unwrap_or_default();
    let span_polys: Vec<(Vec<[f64; 2]>, egui::Color32)> = spans
        .iter()
        .map(|s| {
            let color = span_color(s.kind, s.nack);
            let pts = vec![
                [s.t0_us, band_lo],
                [s.t1_us, band_lo],
                [s.t1_us, band_hi],
                [s.t0_us, band_hi],
            ];
            (pts, color)
        })
        .collect();

    // 判决带：VIH / VIL 两条参考线，告诉用户解码判定的依据在哪
    let (vih, vil) = {
        let l = app.decode_cfg.levels;
        let c = |v: u16| -> f64 {
            if app.show_volts {
                scale.lsb_to_volts(v)
            } else {
                v as f64
            }
        };
        (c(l.vih_lsb), c(l.vil_lsb))
    };

    let trig_x = cap
        .trigger_index
        .map(|i| i as f64 * 1_000_000.0 / cap.rate_hz.max(1) as f64);

    let fit = app.fit_pending;
    app.fit_pending = false;
    // 闭包是 `move` 的，会把 `&mut App` 整个搬进去 —— 所以进闭包前先把
    // 要用的值取出来（闭包之后还要用 app 画下面的解码视图）。
    let show_volts = app.show_volts;

    let plot = Plot::new("scope")
        .x_axis_label("时间 (µs)")
        .y_axis_label(if app.show_volts {
            "电压 (V)"
        } else {
            "ADC LSB"
        })
        .legend(egui_plot::Legend::default())
        .allow_scroll(false);

    // `fit` 时重置视口。注意 Plot 的视口存在 egui memory 里跨帧保留，
    // 光靠 auto_bounds 不会在"新采集到来"时重新贴合。
    let plot = if fit { plot.reset() } else { plot };

    plot.show(ui, move |pui| {
        for (pts, color) in span_polys {
            pui.polygon(Polygon::new("", pts).fill_color(color));
        }
        for (name, pts, color) in series {
            pui.line(Line::new(name, pts).color(color));
        }
        if show_volts {
            pui.hline(HLine::new("VIH", vih).color(egui::Color32::from_gray(90)));
            pui.hline(HLine::new("VIL", vil).color(egui::Color32::from_gray(90)));
        }
        if let Some(x) = trig_x {
            pui.vline(VLine::new("触发", x).color(egui::Color32::from_rgb(255, 80, 80)));
        }
        // 放一个不可见的点集，避免空图时坐标轴退化
        pui.points(Points::new("", vec![[0.0, band_lo]]).radius(0.0));
    });

    ui.separator();
    decode_view(app, ui);
}

fn span_color(kind: SpanKind, nack: bool) -> egui::Color32 {
    if nack {
        return egui::Color32::from_rgba_unmultiplied(255, 60, 60, 40);
    }
    match kind {
        SpanKind::Start => egui::Color32::from_rgba_unmultiplied(255, 200, 60, 45),
        SpanKind::Address => egui::Color32::from_rgba_unmultiplied(80, 160, 255, 40),
        SpanKind::Data => egui::Color32::from_rgba_unmultiplied(80, 220, 120, 35),
        SpanKind::Stop => egui::Color32::from_rgba_unmultiplied(200, 120, 255, 45),
        SpanKind::Truncated => egui::Color32::from_rgba_unmultiplied(160, 160, 160, 45),
    }
}

/// 波形下方：解码结果。
fn decode_view(app: &mut App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.heading("I2C 解码");

        let has_cap = app.capture.is_some();
        let ch_count = app.capture.as_ref().map(|c| c.channels.len()).unwrap_or(0);

        ui.separator();
        ui.label("SCL");
        ui.add_enabled(
            has_cap,
            egui::DragValue::new(&mut app.scl_channel).range(0..=ch_count.saturating_sub(1)),
        );
        ui.label("SDA");
        ui.add_enabled(
            has_cap,
            egui::DragValue::new(&mut app.sda_channel).range(0..=ch_count.saturating_sub(1)),
        );
        ui.label("去抖 ns");
        ui.add(
            egui::DragValue::new(&mut app.decode_cfg.debounce_ns)
                .range(0..=10_000u32)
                .speed(10),
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
        .max_height(200.0)
        .show(ui, |ui| {
            egui::Grid::new("tx_grid")
                .num_columns(2)
                .striped(true)
                .show(ui, |ui| {
                    for line in vmodel::transaction_summary(&d) {
                        ui.monospace(&line);
                        ui.end_row();
                    }
                });
        });
}
