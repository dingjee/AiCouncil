//! Compact SVG icon set based on Lucide paths (MIT, https://lucide.dev/license).
//! All icons share a 24px viewBox, 1.8px stroke, round caps; stroke color
//! comes from the Soft Sage token (no colors fixed inside the SVG).
use iced::widget::svg::{self, Handle};
use iced::{Length, Theme};

use crate::ui_style::palette;

fn stroke_color() -> String {
    let c = palette::ICON;
    format!(
        "#{:02x}{:02x}{:02x}",
        (c.r * 255.0) as u8,
        (c.g * 255.0) as u8,
        (c.b * 255.0) as u8
    )
}

pub fn icon(name: &'static str) -> svg::Svg<'static, Theme> {
    let paths = match name {
        "panel" => "<rect x='3' y='4' width='18' height='16' rx='2'/><path d='M9 4v16'/>",
        "collapse" => "<path d='m14 18-6-6 6-6'/>",
        "up" => "<path d='m5 12 7-7 7 7M12 19V5'/>",
        "down" => "<path d='m5 12 7 7 7-7M12 5v14'/>",
        "chevron" => "<path d='m6 9 6 6 6-6'/>",
        "check" => "<path d='m5 12 4 4L19 6'/>",
        "export" => "<path d='M12 3v12m0 0 4-4m-4 4-4-4'/><path d='M5 17v4h14v-4'/>",
        "send" => "<path d='m22 2-7 20-4-9-9-4Z'/><path d='M22 2 11 13'/>",
        "stop" => "<rect x='5' y='5' width='14' height='14' rx='2'/>",
        "copy" => "<rect x='8' y='8' width='12' height='12' rx='2'/><path d='M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3'/>",
        "retry" => "<path d='M20 7v5h-5M4 17v-5h5'/><path d='M5.6 9a7 7 0 0 1 11.5-2L20 12M4 12l2.9 5a7 7 0 0 0 11.5-2'/>",
        "continue" => "<path d='M7 7h10l-4-4m4 4-4 4'/><path d='M17 17H7l4 4-4-4'/>",
        "history" => "<path d='M3 12a9 9 0 1 0 3-6.7L3 8'/><path d='M3 3v5h5m4-1v5l3 2'/> ",
        "models" => "<path d='m12 3 9 5-9 5-9-5 9-5Z'/><path d='m3 12 9 5 9-5M3 16l9 5 9-5'/>",
        "settings" => "<circle cx='12' cy='12' r='3'/><path d='m19.4 15 .1.1 1.4 1.1-1.4 2.4-1.7-.6a8 8 0 0 1-1.5.9l-.3 1.8h-2.8l-.3-1.8a8 8 0 0 1-1.5-.9l-1.7.6-1.4-2.4 1.4-1.1a7 7 0 0 1 0-1.8l-1.4-1.1 1.4-2.4 1.7.6a8 8 0 0 1 1.5-.9l.3-1.8h2.8l.3 1.8a8 8 0 0 1 1.5.9l1.7-.6 1.4 2.4-1.4 1.1a7 7 0 0 1-.1 1.7Z'/>",
        "usage" => "<path d='M3 3v18h18'/><path d='m7 14 4-4 4 3 5-7'/>",
        "folder" => "<path d='M3 6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z'/>",
        "attach" => "<path d='m21.44 11.05-9.19 9.19a6 6 0 0 1-8.49-8.49l8.57-8.57A4 4 0 1 1 18 8.84l-8.59 8.57a2 2 0 0 1-2.83-2.83l8.49-8.48'/>",
        "x" => "<path d='M18 6 6 18M6 6l12 12'/>",
        "more" => "<circle cx='5' cy='12' r='1'/><circle cx='12' cy='12' r='1'/><circle cx='19' cy='12' r='1'/>",
        "edit" => "<path d='M17 3a2.85 2.83 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z'/>",
        "star" => "<path d='m12 2 3.09 6.26L22 9.27l-5 4.87 1.18 6.88L12 17.77l-6.18 3.25L7 14.14 2 9.27l6.91-1.01L12 2Z'/>",
        "blank" => "",
        _ => "<circle cx='12' cy='12' r='8'/>",
    };
    let data = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24' fill='none' stroke='{}' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'>{paths}</svg>",
        stroke_color()
    );
    svg::Svg::new(Handle::from_memory(data.into_bytes()))
        .width(Length::Fixed(16.0))
        .height(Length::Fixed(16.0))
}
