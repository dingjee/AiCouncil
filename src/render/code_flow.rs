//! CodeFlow:代码块的选择表面(PLAN 4.2)。
//!
//! (line, col) 粒度:等宽字符宽度换算列,行高恒定;高亮 spans 由
//! iced_highlighter 在视图层生成后传入。选区投影/事件协议与
//! [`super::flow::MathFlow`] 相同(SelectionHooks)。

use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{self, LineHeight, Shaping, Wrapping};
use iced::advanced::text::{Paragraph as _, Renderer as _};
use iced::advanced::widget::tree;
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget};
use iced::{mouse, Background, Element, Event, Font, Length, Pixels, Point, Rectangle, Size};
use std::hash::{Hash, Hasher};
use unicode_segmentation::UnicodeSegmentation;

use super::document::PathSeg;
use super::flow::{word_range, SelPos, SelectionHooks};
use super::RenderStyle;

/// renderer 的段落类型。
type Para = <iced::Renderer as text::Renderer>::Paragraph;

const DOUBLE_CLICK_MS: u128 = 400;
const DOUBLE_CLICK_SLOP: f32 = 4.0;

/// 代码块组件状态。
pub struct CodeFlowState {
    version: u64,
    char_w: f32,
    line_h: f32,
    /// 每行已排版的段落(高亮 spans)。
    paragraphs: Vec<Para>,
    last_hover: Option<SelPos>,
    last_click: Option<(std::time::Instant, f32, f32)>,
    click_count: u32,
}

pub struct CodeFlow<'a, Message> {
    /// 代码行(不含换行;持有数据,视图函数内构造无生命周期问题)。
    pub lines: Vec<String>,
    /// 每行的高亮 spans(与 lines 等长;由 iced_highlighter 生成)。
    pub spans: Vec<Vec<iced::advanced::text::Span<'static, ()>>>,
    style: RenderStyle,
    path_id: Vec<PathSeg>,
    /// (起始行, 起始列)-(结束行, 结束列)。
    selection: Option<((usize, usize), (usize, usize))>,
    hooks: Option<SelectionHooks<'a, Message>>,
}

impl<'a, Message: Clone + 'a> CodeFlow<'a, Message> {
    pub fn new(
        lines: Vec<String>,
        spans: Vec<Vec<iced::advanced::text::Span<'static, ()>>>,
        style: RenderStyle,
    ) -> Self {
        Self {
            lines,
            spans,
            style,
            path_id: Vec::new(),
            selection: None,
            hooks: None,
        }
    }

    pub fn path(mut self, path: Vec<PathSeg>) -> Self {
        self.path_id = path;
        self
    }

    pub fn selection(mut self, sel: Option<((usize, usize), (usize, usize))>) -> Self {
        self.selection = sel;
        self
    }

    pub fn selection_hooks(mut self, hooks: SelectionHooks<'a, Message>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    fn code_size(&self) -> f32 {
        self.style.body * 0.9
    }

    fn hit(&self, state: &CodeFlowState, x: f32, y: f32) -> SelPos {
        let line = if self.lines.is_empty() {
            0
        } else {
            ((y / state.line_h.max(1.0)).floor() as i64)
                .clamp(0, self.lines.len() as i64 - 1)
                .max(0) as usize
        };
        let col = (x / state.char_w.max(1.0)).round() as usize;
        let count = self
            .lines
            .get(line)
            .map(|l| l.graphemes(true).count())
            .unwrap_or(0);
        (line, col.min(count))
    }

    fn selection_rects(&self, state: &CodeFlowState) -> Vec<Rectangle> {
        let Some(((mut al, mut ac), (mut bl, mut bc))) = self.selection else {
            return Vec::new();
        };
        if (al, ac) > (bl, bc) {
            std::mem::swap(&mut al, &mut bl);
            std::mem::swap(&mut ac, &mut bc);
        }
        let mut rects = Vec::new();
        let last = self.lines.len().saturating_sub(1);
        for li in al..=bl.min(last) {
            let Some(text) = self.lines.get(li) else {
                continue;
            };
            let n = text.graphemes(true).count();
            let s = if li == al { ac.min(n) } else { 0 };
            let e = if li == bl { bc.min(n) } else { n };
            if s >= e {
                continue;
            }
            let x0 = s as f32 * state.char_w;
            let w = ((e - s) as f32 * state.char_w).max(state.char_w);
            rects.push(Rectangle::new(
                Point::new(x0, li as f32 * state.line_h),
                Size::new(w, state.line_h),
            ));
        }
        rects
    }

    fn cache_key(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.style.revision.hash(&mut hasher);
        self.style.body.to_bits().hash(&mut hasher);
        self.style.color.r.to_bits().hash(&mut hasher);
        self.style.color.g.to_bits().hash(&mut hasher);
        self.style.color.b.to_bits().hash(&mut hasher);
        self.style.color.a.to_bits().hash(&mut hasher);
        self.lines.hash(&mut hasher);
        // Span styles include syntax colors/fonts, which can change when the
        // language or highlighting settings change without changing source text.
        format!("{:?}", self.spans).hash(&mut hasher);
        hasher.finish().max(1)
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for CodeFlow<'a, Message>
{
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<CodeFlowState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(CodeFlowState {
            version: 0,
            char_w: 0.0,
            line_h: 0.0,
            paragraphs: Vec::new(),
            last_hover: None,
            last_click: None,
            click_count: 0,
        })
    }

    fn diff(&self, _tree: &mut iced::advanced::widget::Tree) {}

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = self.code_size();
        let line_h = size * 1.5;
        let state = tree.state.downcast_mut::<CodeFlowState>();
        state.line_h = line_h;
        let rev = self.cache_key();
        if state.version != rev {
            state.paragraphs.clear();
            let probe = Para::with_text(text::Text {
                content: "0000000000",
                bounds: Size::new(65_536.0, 1_024.0),
                size: Pixels(size),
                line_height: LineHeight::Absolute(Pixels(line_h)),
                font: Font::MONOSPACE,
                align_x: text::Alignment::Left,
                align_y: iced::alignment::Vertical::Top,
                shaping: Shaping::Basic,
                wrapping: Wrapping::None,
            });
            state.char_w = (probe.min_bounds().width / 10.0).max(1.0);
            for (li, spans) in self.spans.iter().enumerate() {
                let owned: Vec<_> = spans.clone();
                let p = if owned.is_empty() {
                    Para::with_text(text::Text {
                        content: self.lines.get(li).map(String::as_str).unwrap_or(" "),
                        bounds: Size::new(65_536.0, 1_024.0),
                        size: Pixels(size),
                        line_height: LineHeight::Absolute(Pixels(line_h)),
                        font: Font::MONOSPACE,
                        align_x: text::Alignment::Left,
                        align_y: iced::alignment::Vertical::Top,
                        shaping: Shaping::Basic,
                        wrapping: Wrapping::None,
                    })
                } else {
                    Para::with_spans(text::Text {
                        content: &owned,
                        bounds: Size::new(65_536.0, 1_024.0),
                        size: Pixels(size),
                        line_height: LineHeight::Absolute(Pixels(line_h)),
                        font: Font::MONOSPACE,
                        align_x: text::Alignment::Left,
                        align_y: iced::alignment::Vertical::Top,
                        shaping: Shaping::Basic,
                        wrapping: Wrapping::None,
                    })
                };
                state.paragraphs.push(p);
            }
            state.version = rev;
        }
        let intrinsic_width = state
            .paragraphs
            .iter()
            .map(|paragraph| paragraph.min_bounds().width)
            .fold(1.0_f32, f32::max);
        // Horizontal Scrollable intentionally supplies an infinite max width.
        // Returning that value as the child width poisons its viewport and scroll
        // offset with infinity/NaN, which can make code text disappear after a
        // wheel/scrollbar update. Use the longest laid-out line as the intrinsic
        // width, bounded by any finite parent constraint.
        let width = if limits.max().width.is_finite() {
            intrinsic_width
                .max(limits.min().width)
                .min(limits.max().width)
        } else {
            intrinsic_width.max(limits.min().width)
        }
        .max(1.0);
        let height = line_h * self.lines.len().max(1) as f32;
        layout::Node::new(Size::new(width, height))
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<CodeFlowState>();
        let origin = layout.bounds().position();
        for r in self.selection_rects(state) {
            renderer.fill_quad(
                iced::advanced::renderer::Quad {
                    bounds: Rectangle::new(Point::new(origin.x + r.x, origin.y + r.y), r.size()),
                    ..Default::default()
                },
                Background::Color(self.style.selection),
            );
        }
        for (li, p) in state.paragraphs.iter().enumerate() {
            let pos = Point::new(origin.x, origin.y + li as f32 * state.line_h);
            renderer.fill_paragraph(p, pos, self.style.color, *viewport);
        }
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let Some(hooks) = &self.hooks else {
            return;
        };
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<CodeFlowState>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position() else {
                    return;
                };
                if !bounds.contains(pos) {
                    return;
                }
                let local = Point::new(pos.x - bounds.x, pos.y - bounds.y);
                let hit = self.hit(state, local.x, local.y);
                let now = std::time::Instant::now();
                let double = state
                    .last_click
                    .map(|(t, x, y)| {
                        now.duration_since(t).as_millis() <= DOUBLE_CLICK_MS
                            && (local.x - x).abs() <= DOUBLE_CLICK_SLOP
                            && (local.y - y).abs() <= DOUBLE_CLICK_SLOP
                    })
                    .unwrap_or(false);
                if double {
                    state.click_count += 1;
                } else {
                    state.click_count = 1;
                }
                state.last_click = Some((now, local.x, local.y));
                if state.click_count >= 3 {
                    let last = self.lines.len().saturating_sub(1);
                    let cols = self
                        .lines
                        .last()
                        .map(|l| l.graphemes(true).count())
                        .unwrap_or(0);
                    shell.publish((hooks.on_range)(&self.path_id, (0, 0), (last, cols)));
                } else if double && state.click_count == 2 {
                    let text = self.lines.get(hit.0).map(String::as_str).unwrap_or("");
                    let (ws, we) = word_range(text, hit.1);
                    shell.publish((hooks.on_range)(&self.path_id, (hit.0, ws), (hit.0, we)));
                } else {
                    shell.publish((hooks.on_start)(&self.path_id, hit));
                }
                state.last_hover = Some(hit);
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(pos) = cursor.position() else {
                    return;
                };
                if !bounds.contains(pos) {
                    return;
                }
                let local = Point::new(pos.x - bounds.x, pos.y - bounds.y);
                let hit = self.hit(state, local.x, local.y);
                if state.last_hover != Some(hit) {
                    state.last_hover = Some(hit);
                    shell.publish((hooks.on_extend)(&self.path_id, hit));
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                shell.publish((hooks.on_finish)());
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &iced::advanced::widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        cursor
            .position()
            .filter(|p| layout.bounds().contains(*p))
            .map(|_| mouse::Interaction::Text)
            .unwrap_or(mouse::Interaction::None)
    }
}

impl<'a, Message: Clone + 'a> From<CodeFlow<'a, Message>>
    for Element<'a, Message, iced::Theme, iced::Renderer>
{
    fn from(flow: CodeFlow<'a, Message>) -> Self {
        Element::new(flow)
    }
}
