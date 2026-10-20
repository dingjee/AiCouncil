//! Headless layout and clipping regressions for the live Iced composer and picker.
use iced::advanced::{
    layout,
    renderer::{Headless, Renderer as _},
    widget::Tree,
    Layout,
};
use iced::{
    mouse,
    widget::{container, scrollable, stack, text_editor},
    Background, Border, Color, Element, Font, Length, Padding, Pixels, Rectangle, Size,
};

const SURFACE: Color = Color::from_rgb(0.98, 0.98, 0.96);
const INK: Color = Color::from_rgb(0.15, 0.15, 0.15);

/// 无边框样式:底部区域的墨迹只可能来自字形(不含控件自身的边框),
/// 这样“字形逃出文本视口”的断言才有意义。
fn plain_editor_style(
    _theme: &iced::Theme,
    _status: iced::widget::text_editor::Status,
) -> iced::widget::text_editor::Style {
    iced::widget::text_editor::Style {
        background: Background::Color(SURFACE),
        border: Border::default(),
        placeholder: INK,
        value: INK,
        selection: Color::from_rgb(0.78, 0.86, 0.8),
    }
}

fn editor_content() -> String {
    (0..26)
        .map(|i| match i % 5 {
            0 => format!("段落 {i}：这是一段较长的中文内容，用于验证软换行和滚动之后的每一行都保留正确的行距。"),
            1 => "".to_owned(),
            _ => format!("继续描述第 {i} 行，末尾使用中文标点，确保编辑器处理多个段落而不是单个长字符串。"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn scrolled_editor_lines_keep_vertical_positions_and_clip_to_editor() {
    let body = editor_content();
    let mut content = text_editor::Content::with_text(&body);
    let mut editor: Element<'_, ()> = text_editor(&content)
        .height(Length::Shrink)
        .min_height(76.0)
        .max_height(77.0)
        .padding(8)
        .line_height(iced::advanced::text::LineHeight::Relative(1.55))
        .wrapping(iced::advanced::text::Wrapping::Word)
        .into();
    let mut renderer = iced::Renderer::new(Font::DEFAULT, Pixels(16.0));
    let mut tree = Tree::new(&editor);
    let limits = layout::Limits::new(Size::ZERO, Size::new(520.0, 93.0));
    let node = editor.as_widget_mut().layout(&mut tree, &renderer, &limits);
    {
        let layout = Layout::new(&node);
        assert!(layout.bounds().height <= 93.1);
    }
    drop(node);
    drop(editor);

    // Drive the same public scroll action the text editor publishes for wheel input.
    content.perform(text_editor::Action::Scroll { lines: 8 });
    let mut editor: Element<'_, ()> = text_editor(&content)
        .height(Length::Shrink)
        .min_height(76.0)
        .max_height(77.0)
        .padding(8)
        .line_height(iced::advanced::text::LineHeight::Relative(1.55))
        .wrapping(iced::advanced::text::Wrapping::Word)
        .into();
    tree.diff(&editor);
    let node = editor.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let layout = Layout::new(&node);
    let viewport = Rectangle::with_size(layout.bounds().size());
    renderer.reset(viewport);
    editor.as_widget().draw(
        &tree,
        &mut renderer,
        &iced::Theme::Light,
        &iced::advanced::renderer::Style { text_color: INK },
        layout,
        mouse::Cursor::Unavailable,
        &viewport,
    );
    let physical = Size::new(
        layout.bounds().width.ceil() as u32,
        layout.bounds().height.ceil() as u32,
    );
    let pixels = renderer.screenshot(physical, 1.0, SURFACE);
    save_png("editor-scrolled.png", physical, &pixels);

    let occupied_rows = ink_rows(&pixels, physical, 0, physical.width as usize);
    let groups = row_groups(&occupied_rows);
    assert!(
        groups.len() >= 2,
        "expected multiple visible CJK text rows, got {groups:?}"
    );
    let gaps: Vec<usize> = groups
        .windows(2)
        .map(|pair| pair[1].0 - pair[0].1 - 1)
        .collect();
    assert!(
        gaps.iter().all(|gap| (4..40).contains(gap)),
        "text lines overlap or have abnormal spacing: {groups:?}"
    );
    assert!(
        groups.last().unwrap().1 < physical.height as usize - 8,
        "glyphs escaped the editor's bottom clip: {groups:?}"
    );
}

#[test]
fn retained_pixmap_scroll_matches_full_frame_at_high_dpi() {
    for inner_height in [44.0f32, 45.0, 61.0, 62.0, 77.0, 97.0, 135.0, 188.0, 204.0] {
        let body = format!(
            "{}\n\n{}\n\n{}",
            "第一段中文内容用于测试文本编辑器在滚动后是否残留旧字形。".repeat(24),
            "第二段包含软换行和长中文字符，保证内部视口持续更新。".repeat(26),
            "最后一段应保持正常行距并完整裁剪，不得堆叠在编辑框底部。".repeat(24),
        );
        let mut content = text_editor::Content::with_text(&body);
        let logical = Size::new(520.0, inner_height + 16.0);
        let physical = Size::new(780u32, (logical.height * 1.5).round() as u32);
        let viewport = Rectangle::with_size(logical);
        let graphics_viewport =
            iced_tiny_skia::graphics::Viewport::with_physical_size(physical, 1.5);
        let limits = layout::Limits::new(Size::ZERO, logical);
        let mut renderer = iced::Renderer::new(Font::DEFAULT, Pixels(16.0));
        let min_height = if inner_height >= 76.0 { 76.0 } else { 23.0 };
        let mut editor: Element<'_, ()> = text_editor(&content)
            .height(Length::Shrink)
            .min_height(min_height)
            .max_height(inner_height)
            .padding(8)
            .style(plain_editor_style)
            .line_height(iced::advanced::text::LineHeight::Relative(1.55))
            .wrapping(iced::advanced::text::Wrapping::Word)
            .into();
        let mut tree = Tree::new(&editor);
        let node = editor.as_widget_mut().layout(&mut tree, &renderer, &limits);
        let layout = Layout::new(&node);
        renderer.reset(viewport);
        editor.as_widget().draw(
            &tree,
            &mut renderer,
            &iced::Theme::Light,
            &iced::advanced::renderer::Style { text_color: INK },
            layout,
            mouse::Cursor::Unavailable,
            &viewport,
        );
        let mut previous_layers = renderer.layers().to_vec();
        let mut pixmap = tiny_skia::Pixmap::new(physical.width, physical.height).unwrap();
        let mut mask = tiny_skia::Mask::new(physical.width, physical.height).unwrap();
        mask.data_mut().fill(255);
        renderer.draw(
            &mut pixmap.as_mut(),
            &mut mask,
            &graphics_viewport,
            &[viewport],
            SURFACE,
        );

        for step in 0..5 {
            drop(editor);
            content.perform(text_editor::Action::Scroll { lines: 1 });
            editor = text_editor(&content)
                .height(Length::Shrink)
                .min_height(min_height)
                .max_height(inner_height)
                .padding(8)
                .style(plain_editor_style)
                .line_height(iced::advanced::text::LineHeight::Relative(1.55))
                .wrapping(iced::advanced::text::Wrapping::Word)
                .into();
            tree.diff(&editor);
            let node = editor.as_widget_mut().layout(&mut tree, &renderer, &limits);
            let layout = Layout::new(&node);
            renderer.reset(viewport);
            editor.as_widget().draw(
                &tree,
                &mut renderer,
                &iced::Theme::Light,
                &iced::advanced::renderer::Style { text_color: INK },
                layout,
                mouse::Cursor::Unavailable,
                &viewport,
            );
            let current_layers = renderer.layers().to_vec();
            assert_eq!(previous_layers.len(), current_layers.len());
            let damage: Vec<_> = previous_layers
                .iter()
                .zip(&current_layers)
                .flat_map(|(previous, current)| iced_tiny_skia::Layer::damage(previous, current))
                .collect();
            assert!(!damage.is_empty(), "scroll step {step} produced no damage");
            mask.data_mut().fill(255);
            renderer.draw(
                &mut pixmap.as_mut(),
                &mut mask,
                &graphics_viewport,
                &damage,
                SURFACE,
            );

            let mut full = tiny_skia::Pixmap::new(physical.width, physical.height).unwrap();
            let mut full_mask = tiny_skia::Mask::new(physical.width, physical.height).unwrap();
            full_mask.data_mut().fill(255);
            renderer.draw(
                &mut full.as_mut(),
                &mut full_mask,
                &graphics_viewport,
                &[viewport],
                SURFACE,
            );
            let mismatch = pixmap
                .pixels()
                .iter()
                .zip(full.pixels())
                .filter(|(incremental, complete)| incremental != complete)
                .count();
            assert_eq!(
                mismatch, 0,
                "retained/full frame mismatch at max editor height {inner_height}, scroll step {step}: {mismatch} pixels"
            );
            assert_eq!(
                dark_pixel_count(
                    &pixmap,
                    (8.0 * 1.5) as u32,
                    ((logical.height - 8.0) * 1.5) as u32,
                    physical.height,
                ),
                0,
                "glyphs escaped the editor text viewport into bottom padding at height {inner_height}, scroll step {step}"
            );
            previous_layers = current_layers;
        }
        save_pixmap(&format!("editor-retained-{inner_height:.0}.png"), &pixmap);
    }
}

fn dark_pixel_count(pixmap: &tiny_skia::Pixmap, x0: u32, y0: u32, y1: u32) -> usize {
    pixmap
        .pixels()
        .iter()
        .enumerate()
        .filter(|(index, pixel)| {
            let x = *index as u32 % pixmap.width();
            let y = *index as u32 / pixmap.width();
            x >= x0 && y >= y0 && y < y1 && pixel.red() < 220
        })
        .count()
}

#[test]
fn picker_surface_covers_message_pixels_in_its_bounds() {
    let message = std::iter::repeat_n(
        "Message text under the model picker should remain behind its surface.",
        36,
    )
    .collect::<Vec<_>>()
    .join("\n\n");
    let mut math_cache = aicouncil::render::math::MathCache::new();
    let rendered = aicouncil::render::render_message(
        &message,
        &aicouncil::render::RenderStyle::soft_sage(18.0),
        aicouncil::render::md::ParsePolicy::LlmCompatible,
        &mut math_cache,
    );
    let make_base = || {
        let hooks = aicouncil::render::flow::SelectionHooks::<()> {
            on_start: std::sync::Arc::new(|_, _| ()),
            on_extend: std::sync::Arc::new(|_, _| ()),
            on_finish: std::sync::Arc::new(|| ()),
            on_range: std::sync::Arc::new(|_, _, _| ()),
        };
        scrollable(aicouncil::render::view::view_message(
            &rendered,
            aicouncil::render::RenderStyle::soft_sage(18.0),
            None,
            hooks,
            std::sync::Arc::new(|_| ()),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
    };
    let backdrop = container(
        iced::widget::Space::new()
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill);
    let panel = container(iced::widget::Space::new())
        .width(Length::Fixed(360.0))
        .height(Length::Fixed(260.0))
        .style(|_| iced::widget::container::Style {
            background: Some(Background::Color(SURFACE)),
            border: Border {
                color: INK,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        });
    let placed = container(iced::widget::opaque(panel))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::Alignment::Start)
        .align_y(iced::Alignment::End)
        .padding(Padding {
            top: 0.0,
            bottom: 92.0,
            left: 16.0,
            right: 0.0,
        });
    let mut renderer = iced::Renderer::new(Font::DEFAULT, Pixels(16.0));
    let limits = layout::Limits::new(Size::ZERO, Size::new(900.0, 600.0));
    let physical = Size::new(900, 600);
    let underlay = draw_screenshot(&mut renderer, make_base().into(), &limits, physical);
    let overlay: Element<'_, ()> = stack![make_base(), backdrop, placed].into();
    let pixels = draw_screenshot(&mut renderer, overlay, &limits, physical);
    save_png("picker-overlay.png", physical, &pixels);

    let underlay_ink = count_dark_pixels(&underlay, physical, (16, 248, 376, 508));
    assert!(
        underlay_ink > 1000,
        "fixture does not place MathFlow message text under the picker: {underlay_ink}"
    );
    // The repeated message really occupies this rectangle; the opaque panel
    // must replace it with its surface in the gap below the panel controls.
    for (x, y) in [(120, 360), (240, 375), (350, 450), (120, 490)] {
        let pixel = rgba(&pixels, physical, x, y);
        assert!(
            near(pixel, SURFACE),
            "underlay leaked through picker at ({x},{y}): {pixel:?}"
        );
    }
}

fn draw_screenshot(
    renderer: &mut iced::Renderer,
    mut view: Element<'_, ()>,
    limits: &layout::Limits,
    physical: Size<u32>,
) -> Vec<u8> {
    let mut tree = Tree::new(&view);
    let node = view.as_widget_mut().layout(&mut tree, renderer, limits);
    let root = Layout::new(&node);
    let viewport = Rectangle::with_size(root.bounds().size());
    renderer.reset(viewport);
    view.as_widget().draw(
        &tree,
        renderer,
        &iced::Theme::Light,
        &iced::advanced::renderer::Style { text_color: INK },
        root,
        mouse::Cursor::Unavailable,
        &viewport,
    );
    renderer.screenshot(physical, 1.0, SURFACE)
}

fn count_dark_pixels(pixels: &[u8], size: Size<u32>, bounds: (u32, u32, u32, u32)) -> usize {
    let (x0, y0, x1, y1) = bounds;
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| ((y * size.width + x) * 4) as usize))
        .filter(|index| {
            let [r, g, b, _] = pixels[*index..*index + 4].try_into().unwrap();
            r < 180 && g < 180 && b < 180
        })
        .count()
}

fn ink_rows(pixels: &[u8], size: Size<u32>, start_x: usize, end_x: usize) -> Vec<bool> {
    (0..size.height as usize)
        .map(|y| {
            (start_x..end_x).any(|x| {
                let [r, g, b, _] = rgba(pixels, size, x as u32, y as u32);
                r < 180 && g < 180 && b < 180
            })
        })
        .collect()
}

fn row_groups(rows: &[bool]) -> Vec<(usize, usize)> {
    let mut groups = Vec::new();
    let mut start = None;
    for (i, occupied) in rows.iter().copied().enumerate() {
        match (start, occupied) {
            (None, true) => start = Some(i),
            (Some(begin), false) => {
                groups.push((begin, i - 1));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        groups.push((begin, rows.len() - 1));
    }
    groups
}

fn rgba(pixels: &[u8], size: Size<u32>, x: u32, y: u32) -> [u8; 4] {
    let index = ((y * size.width + x) * 4) as usize;
    pixels[index..index + 4].try_into().unwrap()
}

fn near(actual: [u8; 4], expected: Color) -> bool {
    let expected = [expected.r, expected.g, expected.b].map(|c| (c * 255.0).round() as i16);
    actual[..3]
        .iter()
        .zip(expected)
        .all(|(a, e)| (*a as i16 - e).abs() <= 2)
}

fn save_png(name: &str, size: Size<u32>, pixels: &[u8]) {
    if let Some(dir) = std::env::var_os("AICOUNCIL_EDITOR_CAPTURE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(std::path::PathBuf::from(dir).join(name)).unwrap();
        let mut encoder = png::Encoder::new(file, size.width, size.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(pixels)
            .unwrap();
    }
}

fn save_pixmap(name: &str, pixmap: &tiny_skia::Pixmap) {
    if let Some(dir) = std::env::var_os("AICOUNCIL_EDITOR_CAPTURE_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        pixmap
            .save_png(std::path::PathBuf::from(dir).join(name))
            .unwrap();
    }
}
