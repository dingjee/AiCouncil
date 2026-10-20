//! CodeFlow inside the same horizontal Scrollable used for Markdown code blocks.
use aicouncil::render::{code_flow::CodeFlow, RenderStyle};
use iced::advanced::{
    layout,
    renderer::{Headless, Renderer as _},
    widget::Tree,
    Layout, Shell,
};
use iced::widget::{container, scrollable};
use iced::{mouse, Element, Event, Font, Length, Pixels, Rectangle, Size};
use std::cell::Cell;
use std::rc::Rc;

#[test]
fn code_flow_measures_finite_intrinsic_width_for_horizontal_scroll() {
    let lines = vec![format!(
        "这是一个很长的代码块内容{}",
        "变量名称很长 ".repeat(80)
    )];
    let mut flow: iced::Element<'_, ()> =
        CodeFlow::new(lines, vec![vec![]], RenderStyle::soft_sage(15.0)).into();
    let mut tree = Tree::new(&flow);
    let renderer = iced::Renderer::new(Font::DEFAULT, Pixels(15.0));
    // A horizontal Scrollable gives its child an infinite width limit.
    let limits = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, 1000.0));
    let node = flow.as_widget_mut().layout(&mut tree, &renderer, &limits);
    assert!(
        node.size().width.is_finite(),
        "CodeFlow returned an infinite width: {:?}",
        node.size()
    );
    assert!(
        node.size().width > 400.0,
        "long code line must retain its intrinsic width: {:?}",
        node.size()
    );
}

#[test]
fn horizontally_scrolled_code_block_keeps_text_visible_and_finite() {
    let source = format!("    {} END_TOKEN", "变量名称很长 ".repeat(80));
    let lines = vec![source];
    let offset = Rc::new(Cell::new(0.0_f32));
    let record_offset = offset.clone();
    let mut view: Element<'_, ()> = scrollable(
        container(CodeFlow::new(
            lines.clone(),
            vec![vec![]],
            RenderStyle::soft_sage(15.0),
        ))
        .padding(8.0),
    )
    .direction(scrollable::Direction::Horizontal(
        scrollable::Scrollbar::default().width(6).scroller_width(6),
    ))
    .width(Length::Fill)
    .height(Length::Fixed(88.0))
    .on_scroll(move |viewport| {
        record_offset.set(viewport.absolute_offset().x);
    })
    .into();

    let mut tree = Tree::new(&view);
    let mut renderer = iced::Renderer::new(Font::DEFAULT, Pixels(16.0));
    let bounds = Size::new(360.0, 88.0);
    let limits = layout::Limits::new(Size::ZERO, bounds);
    let node = view.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let root = Layout::new(&node);
    let viewport = Rectangle::with_size(root.bounds().size());
    renderer.reset(viewport);
    view.as_widget().draw(
        &tree,
        &mut renderer,
        &iced::Theme::Light,
        &iced::advanced::renderer::Style {
            text_color: crate_color_text(),
        },
        root,
        mouse::Cursor::Unavailable,
        &viewport,
    );
    let physical = Size::new(bounds.width as u32, bounds.height as u32);
    let before = renderer.screenshot(physical, 1.0, crate_color_background());
    assert!(count_dark_pixels(&before, physical, 0, 360) > 100);

    // Click the actual horizontal scrollbar near its right endpoint. The
    // Scrollable publishes its absolute offset through on_scroll.
    let mut messages = Vec::new();
    let mut clipboard = iced::advanced::clipboard::Null;
    let event = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
    let mut shell = Shell::new(&mut messages);
    view.as_widget_mut().update(
        &mut tree,
        &event,
        root,
        mouse::Cursor::Available(iced::Point::new(340.0, 83.0)),
        &renderer,
        &mut clipboard,
        &mut shell,
        &viewport,
    );
    assert_eq!(
        shell.event_status(),
        iced::event::Status::Captured,
        "scrollbar press was not handled"
    );
    drop(shell);
    let event = Event::Mouse(mouse::Event::CursorMoved {
        position: iced::Point::new(350.0, 83.0),
    });
    let mut shell = Shell::new(&mut messages);
    view.as_widget_mut().update(
        &mut tree,
        &event,
        root,
        mouse::Cursor::Available(iced::Point::new(350.0, 83.0)),
        &renderer,
        &mut clipboard,
        &mut shell,
        &viewport,
    );
    drop(shell);
    let event = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
    let mut shell = Shell::new(&mut messages);
    view.as_widget_mut().update(
        &mut tree,
        &event,
        root,
        mouse::Cursor::Available(iced::Point::new(350.0, 83.0)),
        &renderer,
        &mut clipboard,
        &mut shell,
        &viewport,
    );
    drop(shell);
    let clicked_offset = offset.get();
    assert!(
        clicked_offset.is_finite() && clicked_offset > 0.0,
        "scrollbar click did not move the finite horizontal offset: {clicked_offset}"
    );

    let node = view.as_widget_mut().layout(&mut tree, &renderer, &limits);
    let root = Layout::new(&node);
    let viewport = Rectangle::with_size(root.bounds().size());
    renderer.reset(viewport);
    view.as_widget().draw(
        &tree,
        &mut renderer,
        &iced::Theme::Light,
        &iced::advanced::renderer::Style {
            text_color: crate_color_text(),
        },
        root,
        mouse::Cursor::Unavailable,
        &viewport,
    );
    let after = renderer.screenshot(physical, 1.0, crate_color_background());
    assert!(
        count_dark_pixels(&after, physical, 0, 360) > 100,
        "horizontal scroll left the code box empty"
    );
    assert_ne!(
        before, after,
        "scrollbar interaction did not change the visible code pixels"
    );
    if let Some(directory) = std::env::var_os("AICOUNCIL_EDITOR_CAPTURE_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        let file = std::fs::File::create(
            std::path::PathBuf::from(directory).join("code-scrollbar-after.png"),
        )
        .unwrap();
        let mut encoder = png::Encoder::new(file, physical.width, physical.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&after)
            .unwrap();
    }
}

fn count_dark_pixels(pixels: &[u8], size: Size<u32>, x0: u32, x1: u32) -> usize {
    (0..size.height)
        .flat_map(|y| (x0..x1).map(move |x| ((y * size.width + x) * 4) as usize))
        .filter(|index| {
            let [r, g, b, _] = pixels[*index..*index + 4].try_into().unwrap();
            r < 180 && g < 180 && b < 180
        })
        .count()
}

fn crate_color_text() -> iced::Color {
    aicouncil::ui_style::palette::TEXT
}

fn crate_color_background() -> iced::Color {
    aicouncil::ui_style::palette::CANVAS
}
