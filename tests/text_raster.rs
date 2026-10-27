//! 灰度文字覆盖率修正的回归测试。
//!
//! 修补内容:`vendor/iced_tiny_skia/src/text.rs` 在把 swash 的灰度覆盖率
//! 转成预乘像素时,先对覆盖率做一次 gamma 调整(正常字重),再交给 tiny-skia
//! 合成。加粗字重保持原始覆盖率不变。
//!
//! 覆盖:
//! - 变换本身的性质(端点、单调、正常字重变细、加粗不变);
//! - 真实汉字栅格(情 馈 懂 赢)在 15/18/22px、1.0/1.5 DPI 下的墨量变化;
//! - 生产后端(iced headless → tiny-skia)确实使用了修正后的覆盖率;
//! - 彩色 emoji 字形不受影响。

#[path = "../examples/raster/common.rs"]
mod common;

use common::{Coverage, CJK_SAMPLE};
use iced::advanced::{
    layout,
    renderer::{Headless as _, Renderer as _},
    widget::Tree,
    Layout,
};
use iced::{mouse, Font, Pixels, Rectangle, Size};

const SURFACE: iced::Color = iced::Color::from_rgb(0.98, 0.98, 0.96);
const INK: iced::Color = iced::Color::from_rgb(0.15, 0.15, 0.15);

/// 覆盖率变换:端点保持、单调不降、正常字重变细、加粗不变。
#[test]
fn coverage_transform_keeps_endpoints_and_shrinks_normal_weight() {
    let mut previous = 0u8;

    for coverage in 0..=255u8 {
        let corrected = iced_tiny_skia::coverage_to_alpha(coverage, 400);

        assert_eq!(
            iced_tiny_skia::coverage_to_alpha(0, 400),
            0,
            "fully transparent pixels must stay transparent"
        );
        assert_eq!(
            iced_tiny_skia::coverage_to_alpha(255, 400),
            255,
            "fully covered ink must stay fully covered"
        );
        assert!(
            corrected >= previous,
            "coverage mapping must be monotonic ({coverage}: {corrected} < {previous})"
        );
        assert!(
            corrected <= coverage,
            "coverage mapping must never increase coverage ({coverage} -> {corrected})"
        );

        previous = corrected;
    }
    assert!(iced_tiny_skia::coverage_to_alpha(128, 400) < 128);

    // 显式加粗不得被削弱。
    for coverage in 0..=255u8 {
        assert_eq!(
            iced_tiny_skia::coverage_to_alpha(coverage, 700),
            coverage,
            "bold coverage must stay untouched"
        );
        assert_eq!(
            iced_tiny_skia::coverage_to_alpha(coverage, 900),
            coverage,
            "black coverage must stay untouched"
        );
    }
}

/// 真实汉字栅格:正常字重的墨量下降,加粗字重完全不变。
#[test]
fn cjk_normal_weight_loses_ink_while_bold_is_untouched() {
    for size in [15.0f32, 18.0, 22.0] {
        for scale in [1.0f32, 1.5] {
            let raw = common::rasterize(CJK_SAMPLE, size, scale, 400, Coverage::Raw);
            let fixed = common::rasterize(CJK_SAMPLE, size, scale, 400, Coverage::Corrected);

            let raw_metrics = raw.metrics();
            let fixed_metrics = fixed.metrics();

            assert!(
                raw_metrics.ink > 0,
                "fixture produced no ink at {size}px/{scale}x"
            );
            assert!(
                fixed_metrics.ink < raw_metrics.ink,
                "normal CJK weight must lose ink at {size}px/{scale}x: {} -> {}",
                raw_metrics.ink,
                fixed_metrics.ink
            );

            let ratio = fixed_metrics.ink as f64 / raw_metrics.ink as f64;
            assert!(
                (0.5..0.99).contains(&ratio),
                "unexpected ink ratio {ratio:.3} at {size}px/{scale}x"
            );

            // Fixed-size, aligned masks: no pixel may become darker. Enclosed
            // hole area is not monotonic when an opening connects to the outside.
            assert_eq!((raw.width, raw.height), (fixed.width, fixed.height));
            assert!(fixed
                .coverage
                .iter()
                .zip(&raw.coverage)
                .all(|(fixed, raw)| fixed <= raw));

            // 加粗:逐字节一致。
            let bold_raw = common::rasterize(CJK_SAMPLE, size, scale, 700, Coverage::Raw);
            let bold_fixed = common::rasterize(CJK_SAMPLE, size, scale, 700, Coverage::Corrected);

            assert_eq!(
                bold_raw.coverage, bold_fixed.coverage,
                "bold coverage must be identical at {size}px/{scale}x"
            );
        }
    }
}

/// 生产后端(iced headless → tiny-skia)必须使用修正后的覆盖率。
#[test]
fn production_backend_uses_the_corrected_coverage() {
    let size = 18.0f32;
    let scale = 1.5f32;

    let raw = common::rasterize(CJK_SAMPLE, size, scale, 400, Coverage::Raw);
    let fixed = common::rasterize(CJK_SAMPLE, size, scale, 400, Coverage::Corrected);

    // 排版位置必须一致,否则墨量不可比。
    let raw_bounds = raw.ink_bounds().expect("raw ink bounds");
    let fixed_bounds = fixed.ink_bounds().expect("fixed ink bounds");
    assert_eq!(
        raw_bounds, fixed_bounds,
        "coverage correction must not move glyphs"
    );

    let pixels = render_text(CJK_SAMPLE, size, scale);
    let measured = measured_ink(&pixels, pixels_size(size, scale));

    let raw_ink = raw.metrics().ink as f64;
    let fixed_ink = fixed.metrics().ink as f64;

    // 渲染结果的墨量应当贴近“修正后”的期望值,而不是“修补前”的期望值。
    let measured_ratio = measured / raw_ink;
    let expected_ratio = fixed_ink / raw_ink;

    assert!(
        (measured_ratio - expected_ratio).abs() < 0.05,
        "production ink ratio {measured_ratio:.3} does not match the corrected expectation {expected_ratio:.3} \
         (raw {raw_ink:.0}, corrected {fixed_ink:.0}, measured {measured:.0})"
    );
    assert!(
        measured < raw_ink * 0.98,
        "production backend still composites the uncorrected coverage ({measured:.0} vs raw {raw_ink:.0})"
    );
}

/// 彩色 emoji 字形不走覆盖率路径,必须完全不受修正影响。
#[test]
fn color_emoji_glyphs_are_not_touched() {
    let emoji = "😀";

    let raw = common::rasterize(emoji, 18.0, 1.0, 400, Coverage::Raw);
    let fixed = common::rasterize(emoji, 18.0, 1.0, 400, Coverage::Corrected);
    #[cfg(windows)]
    assert!(
        raw.color_glyphs > 0,
        "the Windows fixture must exercise a color glyph"
    );
    if raw.color_glyphs == 0 {
        eprintln!("System fallback selected no color glyph; color-only comparison unavailable");
        return;
    }

    assert_eq!(
        raw.color_glyphs, fixed.color_glyphs,
        "emoji must be rasterized as a color glyph"
    );
    assert_eq!(
        raw.rgba, fixed.rgba,
        "color glyphs must be bit-identical before and after the coverage correction"
    );
}

/// 渲染一行文本,返回原始 RGBA 像素。
fn render_text(text: &str, size: f32, scale: f32) -> Vec<u8> {
    let mut widget: iced::Element<'_, ()> = iced::widget::text(text).size(size).into();
    let mut renderer = iced::Renderer::new(Font::DEFAULT, Pixels(size));

    let logical = Size::new(320.0, size * 2.0);
    let physical = pixels_size(size, scale);

    let limits = layout::Limits::new(Size::ZERO, logical);
    let mut tree = Tree::new(&widget);
    let node = widget.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let root = Layout::new(&node);
    let viewport = Rectangle::with_size(root.bounds().size());

    renderer.reset(viewport);
    widget.as_widget().draw(
        &tree,
        &mut renderer,
        &iced::Theme::Light,
        &iced::advanced::renderer::Style { text_color: INK },
        root,
        mouse::Cursor::Unavailable,
        &viewport,
    );

    renderer.screenshot(physical, scale, SURFACE)
}

/// headless 截图的物理尺寸。
fn pixels_size(size: f32, scale: f32) -> Size<u32> {
    Size::new(
        (320.0 * scale).round() as u32,
        (size * 2.0 * scale).round() as u32,
    )
}

/// 从渲染结果反推覆盖率并求和(与覆盖率 → 颜色的合成互为逆运算)。
fn measured_ink(pixels: &[u8], size: Size<u32>) -> f64 {
    assert_eq!(pixels.len(), size.width as usize * size.height as usize * 4);
    let [r, g, b, _] = SURFACE.into_rgba8();
    let background = [r, g, b].map(f64::from);
    let [r, g, b, _] = INK.into_rgba8();
    let ink = [r, g, b].map(f64::from);

    let mut total = 0.0;

    for pixel in pixels.as_chunks::<4>().0 {
        let mut coverage = 0.0;

        for channel in 0..3 {
            let denominator = background[channel] - ink[channel];
            let value = pixel[channel] as f64;

            coverage += ((background[channel] - value) / denominator).clamp(0.0, 1.0);
        }

        total += coverage / 3.0 * 255.0;
    }

    total
}
