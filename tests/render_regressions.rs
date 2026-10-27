//! Rendering regressions: CJK 字面、代码块字体与滚动帧成本。
//!
//! - `code_block_cjk_uses_sans_serif`:代码块里的中文必须与正文同一张
//!   字面(cosmic-text 的等宽回退只有 NSimSun / MS Gothic,会把中文排成
//!   宋体/日文字形,与正文的 Microsoft YaHei UI 完全不符)。
//! - `cjk_merged_group_matches_individual_advances`:逐字排布与合并段落
//!   的字形位置必须完全一致(合并绘制的前提)。
//! - `steady_frame_does_not_rasterize_whitespace_again`:稳定帧不得再为
//!   空白字形跑一遍标量器(每帧几十次,直接决定滚动帧耗时)。

use aicouncil::render::{math::MathCache, md, render_message, view, RenderStyle};
use iced::advanced::{
    layout,
    renderer::{Headless, Renderer as _},
    text::{self, LineHeight, Paragraph as _, Shaping, Wrapping},
    widget::Tree,
    Layout,
};
use iced::{Font, Pixels, Rectangle, Size};
use std::sync::atomic::Ordering;

/// renderer 的具体段落类型(与渲染层一致)。
type Para = <iced::Renderer as text::Renderer>::Paragraph;

/// 用**同一个** renderer 连续画 `frames` 帧,返回每帧之后的字形缓存未命中数。
///
/// 同一个 renderer 才能复用字形缓存(新建 renderer 会清空缓存)。
fn draw_frames(markdown: &str, frames: usize, width: f32, height: f32) -> Vec<u64> {
    let style = RenderStyle::soft_sage(15.0);
    let mut cache = MathCache::new();
    let rendered = render_message(markdown, &style, md::ParsePolicy::LlmCompatible, &mut cache);
    let hooks = || aicouncil::render::flow::SelectionHooks::<()> {
        on_start: std::sync::Arc::new(|_, _| ()),
        on_extend: std::sync::Arc::new(|_, _| ()),
        on_finish: std::sync::Arc::new(|| ()),
        on_range: std::sync::Arc::new(|_, _, _| ()),
    };
    let on_link: view::OnLink<'_, ()> = std::sync::Arc::new(|_| ());
    let mut element = view::view_message(&rendered, style, None, hooks(), on_link);

    let mut tree = Tree::new(&element);
    let mut renderer = iced::Renderer::new(Font::DEFAULT, Pixels(style.body));
    let bounds = Size::new(width, height);
    let limits = layout::Limits::new(Size::ZERO, bounds);
    let mut misses = Vec::new();
    for _ in 0..frames {
        let node = element
            .as_widget_mut()
            .layout(&mut tree, &renderer, &limits);
        let root = Layout::new(&node);
        let viewport = Rectangle::with_size(root.bounds().size());
        renderer.reset(viewport);
        element.as_widget().draw(
            &tree,
            &mut renderer,
            &iced::Theme::Light,
            &iced::advanced::renderer::Style {
                text_color: aicouncil::ui_style::palette::TEXT,
            },
            root,
            iced::mouse::Cursor::Unavailable,
            &viewport,
        );
        let physical = Size::new(bounds.width as u32, bounds.height as u32);
        let _ = renderer.screenshot(physical, 1.0, aicouncil::ui_style::palette::CANVAS);
        misses.push(iced_tiny_skia::GLYPH_CACHE_MISSES.load(Ordering::Relaxed));
    }
    misses
}

#[test]
fn code_block_cjk_uses_sans_serif() {
    // 代码块内中文走 sans-serif 族(与正文同字面);拉丁仍是等宽。
    use aicouncil::render::code_flow::script_spans;
    use iced::advanced::text::Span;

    let mut span = Span::new("变量 variable 中文".to_string());
    span.color = Some(iced::Color::BLACK);
    let out = script_spans(vec![span]);

    assert!(out.len() > 1, "混合文本必须被拆成多段: {out:?}");
    let families: Vec<_> = out
        .iter()
        .map(|s| {
            s.font
                .as_ref()
                .map(|f| f.family)
                .unwrap_or(iced::font::Family::Monospace)
        })
        .collect();
    // 「变量 」是中文 → sans-serif;「variable 」是拉丁 → 保持等宽;
    // 「中文」→ sans-serif。
    assert_eq!(families[0], iced::font::Family::SansSerif, "{families:?}");
    assert_eq!(families[1], iced::font::Family::Monospace, "{families:?}");
    assert_eq!(families[2], iced::font::Family::SansSerif, "{families:?}");
    // 颜色等装饰在拆分后保留。
    assert!(out.iter().all(|s| s.color == Some(iced::Color::BLACK)));
}

#[test]
fn cjk_merged_group_matches_individual_advances() {
    // 合并绘制的前提:同一行内连续 CJK 原子逐字排布的 x 位置,
    // 必须与合并成一个段落后的 grapheme 位置一致。
    use aicouncil::render::document::PathSeg;
    use aicouncil::render::flow::{Atom, FlowContent};

    let style = RenderStyle::soft_sage(15.0);
    let mut cache = MathCache::new();
    let text = "观察精神病态：克拉因在临床上大量接触了精神分裂症";
    let doc = md::parse(text, md::ParsePolicy::Strict);
    let flow = FlowContent::from_document(&doc, &style, &mut cache);
    let content = flow.get(&[PathSeg::Block(0)]).unwrap();

    // 原子按 CJK 单字切分(正文排版的实际形态)。
    assert!(content.atoms.len() > 10, "{:?}", content.atoms.len());

    let text_size = content.text_size;
    let line_height = content.line_height;
    let make = |content: &str, wrapping| {
        Para::with_text(text::Text {
            content,
            bounds: Size::new(65_536.0, 1_024.0),
            size: Pixels(text_size),
            line_height: LineHeight::Absolute(Pixels(line_height)),
            font: Font::DEFAULT,
            align_x: text::Alignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping,
        })
    };

    // 逐字宽度:每个原子单独排版得到的宽度。
    let mut widths = Vec::new();
    for atom in &content.atoms {
        let Atom::Text { text, .. } = atom else {
            continue;
        };
        widths.push(make(text.as_str(), Wrapping::Word).min_bounds().width);
    }
    assert_eq!(widths.len(), content.atoms.len());

    // 合并成一个段落后的推进量必须逐字相等。
    let merged_text: String = content
        .atoms
        .iter()
        .map(|a| match a {
            Atom::Text { text, .. } => text.clone(),
            _ => String::new(),
        })
        .collect();
    let merged = make(&merged_text, Wrapping::None);
    let mut x = 0.0f32;
    for (i, w) in widths.iter().enumerate() {
        let gx = merged
            .grapheme_position(0, i)
            .map(|p| p.x)
            .unwrap_or_default();
        assert!(
            (gx - x).abs() < 0.01,
            "第 {i} 个字形位置不一致:合并={gx} 逐字累计={x} (单字宽={w})"
        );
        x += w;
    }
}

#[test]
fn steady_frame_does_not_rasterize_whitespace_again() {
    // 空白字形没有墨迹:必须被缓存成"空结果",
    // 否则每一帧都会为屏幕上的每个空格重跑一次标量器
    // (长中文段落几十个空格 → 滚动帧多出十几毫秒)。
    let markdown = "这是一段带有 English words 和数字 12345 的中文文本，用来验证空白字形的缓存。\n\n第二段同样包含 English words 与标点符号。";
    let misses = draw_frames(markdown, 3, 700.0, 260.0);

    assert!(misses[0] > 0, "首帧应当产生若干缓存未命中");
    assert_eq!(
        misses[1], misses[0],
        "第二帧不得再触发标量器(空白字形未缓存)"
    );
    assert_eq!(
        misses[2], misses[0],
        "第三帧不得再触发标量器(空白字形未缓存)"
    );
}
