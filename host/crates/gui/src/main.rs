//! # scope-gui —— 桌面上位机
//!
//! 把 `scope-core` 的命令层做成给人用的界面：连接 → 配置 → 采集 → 看波形 → 解码 I2C。
//!
//! 与 CLI / MCP 同级 —— 三者都只依赖 [`scope_core::CommandBus`] 与
//! [`scope_core::DevicePort`]，不关心背后是真硬件还是模拟器（见 `docs/01-architecture.md`）。
//!
//! ```text
//! scope-gui  ┃  scope-cli  ┃  scope-mcp      ← 三个消费方，同一套命令层
//!            ↓
//!        CommandBus
//!            ↓  DevicePort（唯一的解耦点）
//!     UART │ 模拟器
//! ```
//!
//! ## 三条纪律
//!
//! 1. **构建必须走 `./host/run.sh`** —— 中文路径会让 MinGW `ld.exe` 链接失败
//!    （见 `docs/07-dev-env.md` 坑 #1）。
//! 2. **阻塞 IO 一律在 worker 线程**（[`worker`]），UI 线程只做绘制。
//!    逻辑与绘制分家正好对应 `eframe::App` 在 0.36 起的 `logic()` / `ui()` 两段。
//! 3. **没有硬件也能跑**：默认走模拟器，`i2c_100k` 场景开箱即见波形与解码。
//!
//! ## 用法
//!
//! ```text
//! ./host/run.sh run -p scope-gui
//! ./host/run.sh run -p scope-gui -- --font "D:/fonts/思源黑体.ttf"
//! ```

#![deny(clippy::all)]
#![warn(missing_docs)]

mod app;
mod font;
mod msg;
mod panels;
mod transport;
mod vmodel;
mod worker;

fn main() -> eframe::Result<()> {
    // 只认一个可选参数：中文字体路径。
    // 系统字体列表覆盖不到时（比如精简版 Windows、CI 容器）用它兜底。
    let args: Vec<String> = std::env::args().collect();
    let font_override: Option<String> = args
        .windows(2)
        .find(|w| w[0] == "--font")
        .map(|w| w[1].clone());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([880.0, 560.0])
            .with_title("F103 Agent Scope"),
        ..Default::default()
    };

    eframe::run_native(
        "F103 Agent Scope",
        options,
        Box::new(move |cc| {
            // 字体必须在建 App 之前装好 —— 界面全是中文，缺字体就是满屏方框
            let outcome = font::install(&cc.egui_ctx, font_override.as_deref());
            Ok(Box::new(app::App::new(&cc.egui_ctx, outcome)))
        }),
    )
}
