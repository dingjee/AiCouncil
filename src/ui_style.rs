//! 全局共享 UI 令牌:Soft Sage 配色 + φ 字级(PLAN 3.1 / 3.1.1 / 3.2)。
//!
//! 独立顶层模块:`render` 与 `app` 都依赖它,但不允许 render → app。
//! 颜色/字号之外的一切控件样式在 `app::design`(iced 样式闭包)。

/// 圆角:控件/菜单/输入框 4px;消息卡/代码容器 6px(禁止胶囊化)。
pub const RADIUS_CONTROL: f32 = 4.0;
pub const RADIUS_CARD: f32 = 6.0;
/// 间距刻度:4 / 8 / 12 / 16 / 24。
pub const SPACING_XS: f32 = 4.0;
pub const SPACING_S: f32 = 8.0;
pub const SPACING_M: f32 = 12.0;
pub const SPACING_L: f32 = 16.0;
pub const SPACING_XL: f32 = 24.0;
/// 图标常规 16px,重要入口 18px;点击区下限 28×28。
pub const ICON_SIZE: f32 = 16.0;
pub const ICON_SIZE_MAJOR: f32 = 18.0;
pub const HIT_AREA: f32 = 28.0;

/// φ = (1+√5)/2。
pub const PHI: f32 = 1.618_034;

/// 正文行高 1.55,标题 1.22(3.2)。
pub const BODY_LINE_HEIGHT: f32 = 1.55;
pub const HEADING_LINE_HEIGHT: f32 = 1.22;
/// 正文字号范围与默认(设置可调,持久化 general settings)。
pub const BODY_SIZE_MIN: f32 = 12.0;
pub const BODY_SIZE_MAX: f32 = 22.0;
pub const BODY_SIZE_DEFAULT: f32 = 15.0;

/// 标题倍率:H1=φ² … H6=1(3.2 表;相邻顶部层级 ≈27% 差异)。
pub fn heading_ratio(level: u8) -> f32 {
    match level {
        1 => PHI * PHI,
        2 => PHI * PHI.sqrt(),
        3 => PHI,
        4 => PHI.sqrt(),
        5 => PHI.sqrt().sqrt(),
        _ => 1.0,
    }
}

/// 标题字重:H1–H4 bold,H5/H6 semibold(字体无半档时回退 bold)。
pub fn heading_weight(level: u8) -> iced::font::Weight {
    if level <= 4 {
        iced::font::Weight::Bold
    } else {
        iced::font::Weight::Semibold
    }
}

/// 语义颜色 token(3.1.1 Soft Sage)。
pub mod palette {
    use iced::{color, Color};
    /// 整窗底色、消息流空白区域。
    pub const CANVAS: Color = color!(0xF4, 0xF3, 0xEE);
    /// assistant 正文、菜单、浮层。
    pub const SURFACE: Color = color!(0xFA, 0xF9, 0xF5);
    /// 历史/模型/设置面板。
    pub const SIDEBAR: Color = color!(0xEC, 0xED, 0xE7);
    /// 输入区表面。
    pub const COMPOSER: Color = color!(0xF8, 0xF8, 0xF2);
    /// user 消息(靠轻微色差分角色)。
    pub const USER_SURFACE: Color = color!(0xEA, 0xEF, 0xE7);
    /// 正文、标题、公式、代码普通文字。
    pub const TEXT: Color = color!(0x2E, 0x34, 0x2F);
    /// 状态、模型信息、placeholder、次级标签。
    pub const TEXT_MUTED: Color = color!(0x62, 0x6B, 0x62);
    /// 常规可用操作图标。
    pub const ICON: Color = color!(0x53, 0x64, 0x51);
    /// 发送、已勾选、主模型标识、激活项。
    pub const ACCENT: Color = color!(0x49, 0x6B, 0x55);
    /// 控件 hover 背景。
    pub const HOVER: Color = color!(0xE3, 0xE9, 0xDF);
    /// 选中项轻背景。
    pub const SELECTED: Color = color!(0xDC, 0xE7, 0xDB);
    /// 正文与原子公式选区(保持深正文色)。
    pub const SELECTION: Color = color!(0xC8, 0xDC, 0xCB);
    /// 表格网格、消息边线、分区。
    pub const LINE_SUBTLE: Color = color!(0xD6, 0xDB, 0xD2);
    /// 输入边界、可操作 scrollbar thumb。
    pub const LINE_INTERACTIVE: Color = color!(0x76, 0x8B, 0x74);
    /// 清晰焦点环(与 hover 区分)。
    pub const FOCUS: Color = color!(0x56, 0x79, 0x5F);
    /// 低饱和蓝青链接(保留下划线)。
    pub const LINK: Color = color!(0x37, 0x68, 0x7B);
    /// 代码块/行内代码底色。
    pub const CODE_SURFACE: Color = color!(0xEC, 0xEF, 0xE8);
    /// 代码注释。
    pub const CODE_COMMENT: Color = color!(0x60, 0x6C, 0x5E);
    /// 简短错误状态(配合图标/文案,不依赖单一颜色)。
    pub const ERROR: Color = color!(0x9B, 0x4D, 0x45);
    pub const WARNING: Color = color!(0x89, 0x66, 0x2D);
    pub const SUCCESS: Color = color!(0x48, 0x67, 0x47);
    /// 实底(accent 按钮等)上的前景。
    pub const ON_ACCENT: Color = color!(0xFA, 0xF9, 0xF5);
}

/// sRGB 相对亮度对比度(WCAG 2.2 公式;验收基线见 docs/ACCEPTANCE.md)。
pub fn contrast(fg: iced::Color, bg: iced::Color) -> f32 {
    fn channel(c: f32) -> f32 {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    let lum =
        |c: iced::Color| 0.2126 * channel(c.r) + 0.7152 * channel(c.g) + 0.0722 * channel(c.b);
    let (l1, l2) = (lum(fg), lum(bg));
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

// ---------------------------------------------------------------------------
// 渲染层容器样式(render::view 使用;iced 类型,无 app 依赖)
// ---------------------------------------------------------------------------

use iced::widget::container;

/// 代码容器。
pub fn code_container_style(_t: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(palette::CODE_SURFACE)),
        border: iced::Border {
            radius: RADIUS_CARD.into(),
            width: 1.0,
            color: palette::LINE_SUBTLE,
        },
        ..Default::default()
    }
}

/// 表头单元格:浅底强调,不靠大 padding。
pub fn table_head_style(_t: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(palette::CODE_SURFACE)),
        border: iced::Border::default(),
        ..Default::default()
    }
}

/// 数据单元格:surface 底(遮住网格缝隙底色,构成 1px 线)。
pub fn table_cell_style(_t: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(palette::SURFACE)),
        border: iced::Border::default(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use palette as p;

    #[test]
    fn golden_heading_ratios() {
        let (r1, r2, r3) = (heading_ratio(1), heading_ratio(2), heading_ratio(3));
        assert!((r1 - 2.618).abs() < 1e-3);
        assert!((r2 - 2.058).abs() < 1e-3);
        assert!((r3 - 1.618).abs() < 1e-3);
        assert!((r1 / r2 - 1.0).abs() > 0.25, "H1/H2 差异应明显大于旧版 18%");
        assert_eq!(heading_ratio(6), 1.0);
        assert_eq!(heading_ratio(0), heading_ratio(6));
    }

    #[test]
    fn body_text_contrast_baseline() {
        // 未舍入对比度 ≥ 4.5:1(文字)/ ≥ 3:1(必要边界)
        assert!(contrast(p::TEXT, p::SURFACE) >= 4.5, "text/surface");
        assert!(contrast(p::TEXT_MUTED, p::SURFACE) >= 4.5, "muted/surface");
        assert!(contrast(p::TEXT_MUTED, p::SIDEBAR) >= 4.5, "muted/sidebar");
        assert!(contrast(p::ACCENT, p::SURFACE) >= 4.5, "accent/surface");
        assert!(contrast(p::LINK, p::SURFACE) >= 4.5, "link/surface");
        assert!(contrast(p::TEXT, p::SELECTION) >= 4.5, "text/selection");
        assert!(
            contrast(p::CODE_COMMENT, p::CODE_SURFACE) >= 4.5,
            "code_comment/code_surface"
        );
        assert!(
            contrast(p::LINE_INTERACTIVE, p::COMPOSER) >= 3.0,
            "line_interactive/composer"
        );
        assert!(contrast(p::ERROR, p::SURFACE) >= 4.5, "error/surface");
    }

    #[test]
    fn body_size_range_sane() {
        const {
            assert!(BODY_SIZE_MIN < BODY_SIZE_DEFAULT && BODY_SIZE_DEFAULT < BODY_SIZE_MAX);
        }
        assert_eq!(
            heading_ratio(3) * BODY_SIZE_DEFAULT,
            PHI * BODY_SIZE_DEFAULT
        );
    }
}
