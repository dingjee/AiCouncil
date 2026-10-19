//! Regression for wrapped text escaping its line boxes and table grid gaps.
use std::sync::Arc;

use aicouncil::render::{
    self, flow::SelectionHooks, math::MathCache, md::ParsePolicy, view, RenderStyle,
};
use iced::advanced::{
    layout,
    renderer::{Headless, Renderer as _},
    widget::Tree,
    Layout,
};
use iced::{mouse, Font, Pixels, Rectangle, Size};

#[test]
fn wrapped_table_backgrounds_fill_rows_and_rules_follow_content() {
    for body in [12.0, 15.0, 22.0] {
        let style = RenderStyle::soft_sage(body);
        let mut cache = MathCache::new();
        let rendered = render::render_message(
            include_str!("fixtures/render_spacing.md"),
            &style,
            ParsePolicy::LlmCompatible,
            &mut cache,
        );
        let mut renderer = iced::Renderer::new(Font::DEFAULT, Pixels(body));
        for width in [360.0, 760.0, 1100.0] {
            let hooks = SelectionHooks::<()> {
                on_start: Arc::new(|_, _| ()),
                on_extend: Arc::new(|_, _| ()),
                on_finish: Arc::new(|| ()),
                on_range: Arc::new(|_, _, _| ()),
            };
            let mut view = view::view_message(&rendered, style, None, hooks, Arc::new(|_| ()));
            let mut tree = Tree::new(&view);
            let node = view.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY)),
            );
            let root = Layout::new(&node);
            let blocks: Vec<_> = root.children().collect();
            let mut bottom = 0.0;
            for block in &blocks {
                assert!(block.bounds().y >= bottom - 0.01);
                bottom = block.bounds().y + block.bounds().height;
            }
            // Paragraph, gap, table -> outer frame -> padding -> row column.
            let table = blocks[2];
            let grid = table.child(0).child(0);
            for row in grid.children() {
                let backgrounds = row.child(0);
                let content = row.child(1);
                assert!((backgrounds.bounds().height - content.bounds().height).abs() < 0.01);
                for (bg, cell) in backgrounds.children().zip(content.children()) {
                    assert!((bg.bounds().height - row.bounds().height).abs() < 0.01);
                    assert!((bg.bounds().x - cell.bounds().x).abs() < 0.01);
                    assert!((bg.bounds().width - cell.bounds().width).abs() < 0.01);
                    assert!(cell.bounds().height <= bg.bounds().height + 0.01);
                    assert!(cell.bounds().height >= body * style.line_height + 12.0 - 0.01);
                    assert_drawn_text_contained(cell.child(0));
                }
            }
            // Full document traversal also checks headings and paragraphs next
            // to horizontal rules, at every tested width/font size.
            assert_flow_nodes_contained(root);
            if let Some(capture_dir) = std::env::var_os("AICOUNCIL_RENDER_CAPTURE_DIR") {
                let scale = 1.5;
                let logical_size = Size::new(width, node.size().height.ceil());
                let viewport = Rectangle::with_size(logical_size);
                renderer.reset(viewport);
                view.as_widget().draw(
                    &tree,
                    &mut renderer,
                    &iced::Theme::Light,
                    &iced::advanced::renderer::Style {
                        text_color: style.color,
                    },
                    root,
                    mouse::Cursor::Unavailable,
                    &viewport,
                );
                let physical_size = Size::new(
                    (width * scale).ceil() as u32,
                    (logical_size.height * scale).ceil() as u32,
                );
                let pixels = renderer.screenshot(
                    physical_size,
                    scale,
                    aicouncil::ui_style::palette::SURFACE,
                );
                std::fs::create_dir_all(&capture_dir).unwrap();
                let path = std::path::PathBuf::from(capture_dir)
                    .join(format!("markdown-{body:.0}px-{width:.0}w.png"));
                let file = std::fs::File::create(path).unwrap();
                let mut encoder =
                    png::Encoder::new(file, physical_size.width, physical_size.height);
                encoder.set_color(png::ColorType::Rgba);
                encoder.set_depth(png::BitDepth::Eight);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&pixels)
                    .unwrap();
            }
        }
    }
}

fn assert_drawn_text_contained(flow: Layout<'_>) {
    let bounds = flow.bounds();
    for line in flow.children() {
        for atom in line.children() {
            assert!(atom.bounds().y >= bounds.y - 0.01);
            assert!(
                atom.bounds().y + atom.bounds().height <= bounds.y + bounds.height + 0.01,
                "text escapes its block and can overlap the next block"
            );
        }
    }
}

fn assert_flow_nodes_contained(parent: Layout<'_>) {
    for child in parent.children() {
        assert!(child.bounds().y >= parent.bounds().y - 0.01);
        assert!(
            child.bounds().y + child.bounds().height
                <= parent.bounds().y + parent.bounds().height + 0.1,
            "child escapes parent: {:?}, parent {:?}",
            child.bounds(),
            parent.bounds()
        );
        assert_flow_nodes_contained(child);
    }
}
