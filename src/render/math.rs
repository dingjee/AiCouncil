//! LaTeX 公式渲染与缓存。
//!
//! iced_math 把 LaTeX 排版为 SVG(纯 Rust,无 JS 执行)。SVG 的
//! width/height 直接取自输出头部,垂直中心即数学轴(TeX 惯例),
//! 供 [`crate::render::flow`] 做基线对齐。
//!
//! 缓存键 = (公式源文, 字号, 显示模式, 颜色),见 PLAN P0.5。

use std::collections::HashMap;
use std::sync::Arc;

use iced::advanced::svg;
use iced::Size;

/// 渲染一条公式所需的一切。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MathKey {
    /// 公式 LaTeX 源文(规范化后)。
    pub tex: Arc<str>,
    pub font_size_bits: u32,
    pub display: bool,
    pub color_bits: u32,
}

/// 单条公式的缓存产物。
pub struct CachedMath {
    pub handle: svg::Handle,
    /// SVG 固有尺寸(逻辑像素)。
    pub size: Size,
    /// 解析失败时的回退:显示红色源文(由视图层处理)。
    pub error: Option<String>,
}

/// 全局公式缓存。公式内容在流式输出稳定后不再变化,命中率接近 1。
#[derive(Default)]
pub struct MathCache {
    entries: HashMap<MathKey, CachedMath>,
}

fn color_bits(c: iced::Color) -> u32 {
    let (r, g, b, a) = (c.r, c.g, c.b, c.a);
    ((r * 255.0) as u32) << 24
        | ((g * 255.0) as u32) << 16
        | ((b * 255.0) as u32) << 8
        | ((a * 255.0) as u32)
}

impl MathCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取或渲染一条公式。解析失败不 panic,返回带 error 的回退项。
    pub fn get_or_render(
        &mut self,
        tex: &str,
        font_size: f32,
        display: bool,
        color: iced::Color,
    ) -> &CachedMath {
        let key = MathKey {
            tex: Arc::from(normalize_tex(tex).as_str()),
            font_size_bits: font_size.to_bits(),
            display,
            color_bits: color_bits(color),
        };
        self.entries.entry(key).or_insert_with_key(|k| {
            let renderer = iced_math::MathRenderer::new()
                .font_size(f32::from_bits(k.font_size_bits))
                .display_style(k.display)
                .color(iced_math::Color::rgb(
                    ((k.color_bits >> 24) & 0xFF) as u8,
                    ((k.color_bits >> 16) & 0xFF) as u8,
                    ((k.color_bits >> 8) & 0xFF) as u8,
                ));
            match renderer.to_svg(&k.tex) {
                Ok(bytes) => {
                    let size = parse_svg_size(&bytes).unwrap_or(Size::new(16.0, 16.0));
                    CachedMath {
                        handle: svg::Handle::from_memory(bytes),
                        size,
                        error: None,
                    }
                }
                Err(_) => CachedMath {
                    handle: svg::Handle::from_memory(Vec::new()),
                    size: Size::ZERO,
                    error: Some(k.tex.to_string()),
                },
            }
        })
    }
}

/// 环境名映射:iced_math 不支持带编号的 `align`/`gather` 等,
/// 映射到等价的无编号环境(渲染语义一致,编号本就不在首版范围)。
pub fn normalize_tex(tex: &str) -> String {
    tex.trim()
        .replace("\\begin{align}", "\\begin{aligned}")
        .replace("\\begin{align*}", "\\begin{aligned}")
        .replace("\\end{align}", "\\end{aligned}")
        .replace("\\end{align*}", "\\end{aligned}")
        .replace("\\begin{gather}", "\\begin{gathered}")
        .replace("\\begin{gather*}", "\\begin{gathered}")
        .replace("\\end{gather}", "\\end{gathered}")
        .replace("\\end{gather*}", "\\end{gathered}")
}

/// 从 iced_math 输出的 SVG 头部解析固有尺寸。
/// 输出形如 `<svg xmlns="..." width="{w}" height="{h}" viewBox="0 0 {w} {h}">`。
fn parse_svg_size(bytes: &[u8]) -> Option<Size> {
    let s = std::str::from_utf8(bytes).ok()?;
    let start = s.find('<')?;
    let header_end = s[start..].find('>')? + start;
    let header = &s[start..header_end];

    let mut width = None;
    let mut height = None;
    for attr in header.split_whitespace() {
        if let Some(v) = attr.strip_prefix("width=\"") {
            width = v.strip_suffix('"').and_then(|v| v.parse::<f32>().ok());
        } else if let Some(v) = attr.strip_prefix("height=\"") {
            height = v.strip_suffix('"').and_then(|v| v.parse::<f32>().ok());
        }
    }
    Some(Size::new(width?, height?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_fraction_to_sized_svg() {
        let mut cache = MathCache::new();
        let m = cache.get_or_render(r"\frac{1}{2}", 16.0, false, iced::Color::BLACK);
        assert!(m.error.is_none());
        assert!(m.size.width > 8.0 && m.size.height > 8.0, "{:?}", m.size);
    }

    #[test]
    fn parse_error_returns_fallback() {
        let mut cache = MathCache::new();
        // 未知宏:要么 iced_math 容忍(渲染出结果),要么给出错误回退;两种都不 panic。
        let _ = cache
            .get_or_render(r"\unknownmacro{x}", 16.0, false, iced::Color::BLACK)
            .error;
    }

    #[test]
    fn normalize_aligns() {
        assert_eq!(
            normalize_tex("\\begin{align} x &= 1 \\\\ \\end{align}"),
            "\\begin{aligned} x &= 1 \\\\ \\end{aligned}"
        );
    }

    #[test]
    fn svg_size_parser() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="12.5" height="4.25" viewBox="0 0 12.5 4.25">"#;
        let s = parse_svg_size(svg).unwrap();
        assert_eq!((s.width, s.height), (12.5, 4.25));
    }

    #[test]
    fn cache_hit_same_key() {
        let mut cache = MathCache::new();
        let ptr1 = cache.get_or_render("x^2", 16.0, false, iced::Color::BLACK) as *const _;
        let ptr2 = cache.get_or_render("x^2", 16.0, false, iced::Color::BLACK) as *const _;
        assert_eq!(ptr1, ptr2, "same key must reuse cached entry");
    }
}
