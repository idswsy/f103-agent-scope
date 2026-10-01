//! 中文字体加载。
//!
//! egui 自带的字体**不含 CJK** —— 不挂字体的话整个界面（以及解码表格里的
//! 「帧 / 地址 / 数据 / ACK」）全是豆腐块。
//!
//! # 三条原则
//!
//! 1. **纯 TTF 优先**：`.ttc` 是字体集合，需要额外的 `index` 参数才能取到正确那一款。
//!    `simhei.ttf` 是单体字体，最省事。
//! 2. **绝不 panic**：CI 是 Ubuntu，没有 Windows 字体。找不到就降级并返回 `None`，
//!    由调用方决定是否提示。
//! 3. **不把字体文件提交进仓库**：这些是微软/方正的商业字体，而本项目是 GPL-3.0。
//!    只在运行时从系统加载。将来若要内置，必须换成 OFL 授权的
//!    Noto Sans CJK / 思源黑体。

use std::sync::Arc;

/// 按优先级排列的候选字体路径。
///
/// 顺序有讲究：单体 TTF 在前，字体集合在后。
const CANDIDATES: &[&str] = &[
    // ── Windows ──
    r"C:\Windows\Fonts\simhei.ttf", // 黑体，单体 TTF
    r"C:\Windows\Fonts\msyh.ttc",   // 微软雅黑，字体集合
    r"C:\Windows\Fonts\simsun.ttc", // 宋体，字体集合
    // ── Linux（CI / 服务器）──
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "/usr/share/fonts/truetype/arphic/uming.ttc",
    // ── macOS ──
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
];

/// 挂载结果。
#[derive(Debug, Clone)]
pub enum FontOutcome {
    /// 用上了系统字体。
    Loaded {
        /// 实际加载的路径。
        path: String,
    },
    /// 没找到任何中文字体 —— 界面中文会显示成方框。
    NotFound {
        /// 找过的路径，用于提示用户。
        tried: Vec<String>,
    },
}

impl FontOutcome {
    /// 给用户的一句提示。
    pub fn notice(&self) -> Option<String> {
        match self {
            FontOutcome::Loaded { .. } => None,
            FontOutcome::NotFound { tried } => Some(format!(
                "未找到中文字体，界面中文会显示为方框。候选路径：{}。\
                 可用 `--font <路径>` 手动指定。",
                tried.join(" / ")
            )),
        }
    }
}

/// 往 egui 里装中文字体。`override_path` 非空时只试它。
///
/// 装法是**追加到 family 列表末尾** —— egui 按顺序找字形，拉丁字母仍由默认字体渲染，
/// 只有它缺的字形（汉字）才落到 CJK 字体上。这样不用牺牲默认字体的观感。
///
/// `Proportional` 和 `Monospace` **都要挂** —— 只挂前者的话，
/// 解码表格里的中文（用等宽字体渲染）还是方框。
pub fn install(ctx: &egui::Context, override_path: Option<&str>) -> FontOutcome {
    let tried: Vec<String> = match override_path {
        Some(p) => vec![p.to_string()],
        None => CANDIDATES.iter().map(|s| s.to_string()).collect(),
    };

    for path in &tried {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };

        // .ttc 是字体集合，取第 0 款；单体 .ttf/.otf 的 index 同样是 0
        let mut data = egui::FontData::from_owned(bytes);
        data.index = 0;

        let mut fonts = egui::FontDefinitions::default();
        let key = "cjk".to_string();
        fonts.font_data.insert(key.clone(), Arc::new(data));

        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(key.clone());
        }

        ctx.set_fonts(fonts);
        return FontOutcome::Loaded { path: path.clone() };
    }

    FontOutcome::NotFound { tried }
}
