//! 控件样式与 iced Theme(PLAN 3.1)。
//!
//! 语义 token(颜色/字级/间距)在顶层 [`crate::ui_style`](render 与 app
//! 共享,不允许 render → app 依赖);本模块只提供 iced 控件的样式闭包
//! 与 Soft Sage Theme 构造。禁止在 view 层散落新的字面色值。

use iced::widget::{button, container, text_input};
use iced::{Background, Border, Color, Theme};

pub use crate::ui_style::{
    palette, RADIUS_CARD, RADIUS_CONTROL, SPACING_L, SPACING_M, SPACING_S, SPACING_XS,
};

/// Soft Sage 派生 palette(内建控件默认:菜单/滚动条/text_editor 等)。
pub fn theme() -> Theme {
    Theme::custom(
        "Soft Sage",
        iced::theme::Palette {
            background: palette::CANVAS,
            text: palette::TEXT,
            primary: palette::ACCENT,
            success: palette::SUCCESS,
            warning: palette::WARNING,
            danger: palette::ERROR,
        },
    )
}

// ---------------------------------------------------------------------------
// 按钮
// ---------------------------------------------------------------------------

/// 常规安静按钮(图标/次级操作):透明底,hover 轻背景。
pub fn quiet_button(_theme: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        text_color: palette::TEXT,
        border: Border {
            radius: RADIUS_CONTROL.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        ..Default::default()
    };
    match status {
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(palette::HOVER)),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(palette::SELECTED)),
            ..base
        },
        button::Status::Disabled => button::Style {
            text_color: palette::TEXT_MUTED,
            ..base
        },
        _ => base,
    }
}

/// 主操作按钮(发送等):accent 实底,米白前景。
pub fn accent_button(_theme: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: Some(Background::Color(palette::ACCENT)),
        text_color: palette::ON_ACCENT,
        border: Border {
            radius: RADIUS_CONTROL.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        ..Default::default()
    };
    match status {
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(mix(palette::ACCENT, palette::TEXT, 0.12))),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(mix(palette::ACCENT, palette::TEXT, 0.22))),
            ..base
        },
        button::Status::Disabled => button::Style {
            background: Some(Background::Color(palette::LINE_SUBTLE)),
            text_color: palette::TEXT_MUTED,
            ..base
        },
        _ => base,
    }
}

/// 激活态(当前面板/当前会话):选中轻背景 + accent 文字。
pub fn active_button(_theme: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: Some(Background::Color(palette::SELECTED)),
        text_color: palette::ACCENT,
        border: Border {
            radius: RADIUS_CONTROL.into(),
            width: 1.0,
            color: palette::LINE_SUBTLE,
        },
        ..Default::default()
    };
    match status {
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(mix(
                palette::SELECTED,
                palette::ACCENT,
                0.08,
            ))),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(mix(
                palette::SELECTED,
                palette::ACCENT,
                0.16,
            ))),
            ..base
        },
        _ => base,
    }
}

/// 危险操作(删除会话)。
pub fn danger_button(_theme: &Theme, status: button::Status) -> button::Style {
    let base = button::Style {
        background: Some(Background::Color(Color::TRANSPARENT)),
        text_color: palette::ERROR,
        border: Border {
            radius: RADIUS_CONTROL.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        ..Default::default()
    };
    match status {
        button::Status::Hovered => button::Style {
            background: Some(Background::Color(mix(
                palette::ERROR,
                palette::SURFACE,
                0.88,
            ))),
            ..base
        },
        button::Status::Pressed => button::Style {
            background: Some(Background::Color(mix(
                palette::ERROR,
                palette::SURFACE,
                0.78,
            ))),
            ..base
        },
        _ => base,
    }
}

// ---------------------------------------------------------------------------
// 输入框
// ---------------------------------------------------------------------------

/// 输入框:表面 + 可辨边界 + focus 环。
pub fn text_input_style(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let base = text_input::Style {
        background: Background::Color(palette::SURFACE),
        border: Border {
            radius: RADIUS_CONTROL.into(),
            width: 1.0,
            color: palette::LINE_INTERACTIVE,
        },
        icon: palette::TEXT_MUTED,
        placeholder: palette::TEXT_MUTED,
        value: palette::TEXT,
        selection: palette::SELECTION,
    };
    match status {
        text_input::Status::Focused { .. } => text_input::Style {
            border: Border {
                radius: RADIUS_CONTROL.into(),
                width: 2.0,
                color: palette::FOCUS,
            },
            ..base
        },
        text_input::Status::Disabled => text_input::Style {
            background: Background::Color(palette::CANVAS),
            border: Border {
                radius: RADIUS_CONTROL.into(),
                width: 1.0,
                color: palette::LINE_SUBTLE,
            },
            value: palette::TEXT_MUTED,
            ..base
        },
        _ => base,
    }
}

// ---------------------------------------------------------------------------
// 容器
// ---------------------------------------------------------------------------

/// 消息卡片容器:user/assistant 靠轻微色差分角色。
pub fn card_style(user: bool) -> impl Fn(&Theme) -> container::Style {
    move |_t: &Theme| container::Style {
        background: Some(Background::Color(if user {
            palette::USER_SURFACE
        } else {
            palette::SURFACE
        })),
        border: Border {
            radius: RADIUS_CARD.into(),
            width: 1.0,
            color: palette::LINE_SUBTLE,
        },
        ..Default::default()
    }
}

/// 附件 chip。
pub fn chip_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::CODE_SURFACE)),
        border: Border {
            radius: RADIUS_CONTROL.into(),
            width: 1.0,
            color: palette::LINE_SUBTLE,
        },
        ..Default::default()
    }
}

/// 面板容器(侧栏)。
pub fn sidebar_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::SIDEBAR)),
        ..Default::default()
    }
}

/// 输入区表面。
pub fn composer_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::COMPOSER)),
        border: Border {
            radius: 0.0.into(),
            width: 1.0,
            color: palette::LINE_SUBTLE,
        },
        ..Default::default()
    }
}

/// 侧栏分隔条(视觉 1px 线,命中区 6px 由 mouse_area 提供)。
pub fn divider_style(_t: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(palette::LINE_SUBTLE)),
        ..Default::default()
    }
}

/// 两色插值(向 target 靠拢 ratio)。
fn mix(a: Color, b: Color, ratio: f32) -> Color {
    Color {
        r: a.r + (b.r - a.r) * ratio,
        g: a.g + (b.g - a.g) * ratio,
        b: a.b + (b.b - a.b) * ratio,
        a: 1.0,
    }
}
