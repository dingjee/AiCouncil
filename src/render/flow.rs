//! MathFlow:文本 + 行内公式的流式段落自定义组件。
//!
//! iced 没有原生的“文本中嵌行内组件”布局,这里实现一个贪心断行的
//! 流式排版:
//! - 原子(atom)= 拉丁词(含尾部空白)/ 单个 CJK 字符 / 行内公式(不可分割)。
//! - 文本原子用 renderer 的 Paragraph 类型测量与绘制(与 iced 文本
//!   组件同一 cosmic-text 后端,中文回退一致);
//! - 公式原子是 iced_math 生成的 SVG,固有尺寸来自 SVG 头部,
//!   垂直中心即数学轴 → 基线 = 轴 + 0.25em(TeX 惯例)。
//! - 文本测量缓存键 = (字体样式, 文本);原子固有宽度与容器宽度无关,
//!   只在段落内容版本变化时重新测量(PLAN P0.5)。

use std::collections::HashMap;
use std::sync::Arc;

use iced::advanced::layout::{self, Layout};
use iced::advanced::text::{self, LineHeight, Paragraph as _, Shaping, Wrapping};
use iced::advanced::widget::tree;
use iced::advanced::svg::Renderer as _;
use iced::advanced::text::Renderer as _;
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget};
use iced::{
    mouse, Background, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Vector,
};

use super::math::MathCache;
use super::md;
use super::RenderStyle;

/// renderer 的具体段落类型(与 iced 文本组件一致)。
pub type Para = <iced::Renderer as text::Renderer>::Paragraph;

/// 一次渲染好的公式图形。
#[derive(Clone)]
pub struct MathGfx {
    pub handle: iced::advanced::svg::Handle,
    pub size: Size,
    /// 解析失败:回退为红色源文文本。
    pub error: Option<Arc<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Atom {
    Text {
        text: String,
        style: TextAtomStyle,
    },
    Math {
        idx: usize,
    },
}

/// 影响字体度量的原子样式(缓存键的一部分;链接/颜色只影响绘制)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextAtomStyle {
    pub bold: bool,
    pub italic: bool,
    pub mono: bool,
}

/// 链接等装饰信息,不进入度量缓存键。
#[derive(Debug, Clone, Default)]
pub struct AtomDecor {
    pub link: Option<String>,
    pub strikethrough: bool,
}

/// 一个含行内公式的段落的排版输入。
pub struct ParaFlow {
    pub version: u64,
    pub atoms: Vec<Atom>,
    pub decor: Vec<AtomDecor>,
    pub math: Vec<MathGfx>,
    /// 行内公式字号(基线计算用)。
    pub math_font_size: f32,
}

/// 一条消息的公式排版产物:
/// - `paras`:含**行内**公式的段落,按文档遍历顺序排列;
/// - `blocks`:**块级**公式图形,按文档遍历顺序排列。
/// 视图层必须以相同的深度优先顺序消费。
pub struct FlowContent {
    pub paras: Vec<Arc<ParaFlow>>,
    pub blocks: Vec<MathGfx>,
}

impl FlowContent {
    /// 从文档提取含公式的段落,并渲染其公式 SVG。
    pub fn from_document(doc: &md::Document, style: &RenderStyle, cache: &mut MathCache) -> Self {
        let mut paras = Vec::new();
        for inlines in iter_inlines(doc) {
            if !inlines.has_math {
                continue;
            }
            let flow = atomize(inlines, style, cache);
            paras.push(Arc::new(flow));
        }
        let mut blocks = Vec::new();
        for tex in iter_block_math(doc) {
            blocks.push(render_block_math(tex, style, cache));
        }
        FlowContent { paras, blocks }
    }
}

/// 文档内行内内容,按渲染顺序(与视图遍历顺序一致)。
fn iter_inlines(doc: &md::Document) -> Vec<&md::Inlines> {
    fn in_blocks(blocks: &[md::Block]) -> Vec<&md::Inlines> {
        let mut out = Vec::new();
        for b in blocks {
            match b {
                md::Block::Heading { inlines, .. } | md::Block::Paragraph(inlines) => {
                    out.push(inlines)
                }
                md::Block::Quote(inner) => out.extend(in_blocks(inner)),
                md::Block::List { items, .. } => {
                    for blocks in items {
                        out.extend(in_blocks(blocks));
                    }
                }
                md::Block::Table { rows, .. } => {
                    for r in rows {
                        out.extend(r.iter());
                    }
                }
                _ => {}
            }
        }
        out
    }
    in_blocks(&doc.blocks)
}

/// 文档内块级公式源文,按渲染顺序。
fn iter_block_math(doc: &md::Document) -> Vec<&str> {
    fn walk<'b>(blocks: &'b [md::Block], out: &mut Vec<&'b str>) {
        for b in blocks {
            match b {
                md::Block::MathBlock { tex } => out.push(tex.as_str()),
                md::Block::Quote(inner) => walk(inner, out),
                md::Block::List { items, .. } => {
                    for blocks in items {
                        walk(blocks, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&doc.blocks, &mut out);
    out
}

fn render_block_math(tex: &str, style: &RenderStyle, cache: &mut MathCache) -> MathGfx {
    let cached = cache.get_or_render(tex, style.block_math_size, true, style.color);
    MathGfx {
        handle: cached.handle.clone(),
        size: cached.size,
        error: cached.error.clone().map(Arc::from),
    }
}

fn push_word(
    word: &mut String,
    st: &md::Style,
    atoms: &mut Vec<Atom>,
    decor: &mut Vec<AtomDecor>,
) {
    if word.is_empty() {
        return;
    }
    atoms.push(Atom::Text {
        text: std::mem::take(word),
        style: TextAtomStyle {
            bold: st.bold,
            italic: st.italic,
            mono: st.code,
        },
    });
    decor.push(AtomDecor {
        link: st.link.clone(),
        strikethrough: st.strikethrough,
    });
}

/// 原子化:拉丁词为原子(尾部空白跟随),CJK 单字为原子,公式为原子。
fn atomize(inlines: &md::Inlines, style: &RenderStyle, cache: &mut MathCache) -> ParaFlow {
    let mut atoms: Vec<Atom> = Vec::new();
    let mut decor: Vec<AtomDecor> = Vec::new();
    let mut math: Vec<MathGfx> = Vec::new();

    for part in &inlines.parts {
        match part {
            md::Part::Text { style: st, content } => {
                let mut word = String::new();
                for ch in content.chars() {
                    if is_cjk(ch) {
                        push_word(&mut word, st, &mut atoms, &mut decor);
                        atoms.push(Atom::Text {
                            text: ch.to_string(),
                            style: TextAtomStyle {
                                bold: st.bold,
                                italic: st.italic,
                                mono: st.code,
                            },
                        });
                        decor.push(AtomDecor {
                            link: st.link.clone(),
                            strikethrough: st.strikethrough,
                        });
                    } else {
                        word.push(ch);
                    }
                }
                push_word(&mut word, st, &mut atoms, &mut decor);
            }
            md::Part::Math { tex } => {
                let cached = cache.get_or_render(tex, style.inline_math_size, false, style.color);
                let gfx = MathGfx {
                    handle: cached.handle.clone(),
                    size: cached.size,
                    error: cached.error.clone().map(Arc::from),
                };
                atoms.push(Atom::Math { idx: math.len() });
                decor.push(AtomDecor::default());
                math.push(gfx);
            }
            _ => {}
        }
    }
    ParaFlow {
        version: next_version(),
        atoms,
        decor,
        math,
        math_font_size: style.inline_math_size,
    }
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF   // CJK 统一表意
        | 0x3400..=0x4DBF // 扩展 A
        | 0x3000..=0x303F // CJK 标点
        | 0xFF00..=0xFFEF // 全角形式
        | 0x3040..=0x30FF // 平假名/片假名
    )
}

static VERSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn next_version() -> u64 {
    VERSION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// 组件
// ---------------------------------------------------------------------------

/// 文本原子的测量缓存条目。
struct TextMeasure {
    paragraph: Para,
}

/// 布局期放置的原子。
struct PlacedAtom {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    ascent_hint: f32,
    descent_hint: f32,
}

struct LineBox {
    atoms: Vec<PlacedAtom>,
    height: f32,
}

/// 组件状态:内容版本 + 布局宽度 + 文本测量缓存 + 行盒。
pub struct FlowState {
    version: u64,
    width: f32,
    measures: HashMap<TextAtomStyle, HashMap<String, TextMeasure>>,
    lines: Vec<LineBox>,
}

/// 文本基线在行盒高度中的占比近似(cosmic-text 不暴露基线;
/// 主流 UI 字体的 ascent/(ascent+descent) ≈ 0.8)。
const ASCENT_RATIO: f32 = 0.8;
/// 数学轴高度(TeX:axis ≈ 0.25em)。
const AXIS_EM: f32 = 0.25;

pub struct MathFlow<'a, Message> {
    content: &'a ParaFlow,
    style: RenderStyle,
    code_bg: Color,
    error_color: Color,
    on_link: Option<Box<dyn Fn(&str) -> Message + 'a>>,
}

impl<'a, Message: 'a> MathFlow<'a, Message> {
    pub fn new(content: &'a ParaFlow, style: RenderStyle) -> Self {
        Self {
            content,
            style,
            code_bg: Color::from_rgb(0.92, 0.92, 0.94),
            error_color: Color::from_rgb(0.75, 0.22, 0.17),
            on_link: None,
        }
    }

    pub fn on_link(mut self, f: impl Fn(&str) -> Message + 'a) -> Self {
        self.on_link = Some(Box::new(f));
        self
    }

    fn font_of(style: TextAtomStyle) -> Font {
        Font {
            family: if style.mono {
                iced::font::Family::Monospace
            } else {
                iced::font::Family::SansSerif
            },
            weight: if style.bold {
                iced::font::Weight::Bold
            } else {
                iced::font::Weight::Normal
            },
            stretch: Default::default(),
            style: if style.italic {
                iced::font::Style::Italic
            } else {
                iced::font::Style::Normal
            },
        }
    }

    fn measure_text(&self, state: &mut FlowState, key: TextAtomStyle, text: &str) -> (f32, f32) {
        let per_font = state.measures.entry(key).or_default();
        if let Some(m) = per_font.get(text) {
            return (m.paragraph.min_bounds().width, m.paragraph.min_bounds().height);
        }
        let paragraph = Para::with_text(text::Text {
            content: text,
            bounds: Size::new(65_536.0, 1_024.0),
            size: Pixels(self.style.text_size),
            line_height: LineHeight::default(),
            font: Self::font_of(key),
            align_x: text::Alignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::Word,
        });
        let bounds = paragraph.min_bounds();
        let size = (bounds.width, bounds.height);
        per_font.insert(text.to_string(), TextMeasure { paragraph });
        size
    }

    fn layout_lines(&self, state: &mut FlowState, width: f32) {
        if state.version == self.content.version && (state.width - width).abs() < f32::EPSILON {
            return;
        }
        state.version = self.content.version;
        state.width = width;

        let math_size = self.content.math_font_size;
        let mut lines: Vec<LineBox> = Vec::new();
        let mut cur: Vec<PlacedAtom> = Vec::new();
        let mut x = 0.0;

        let finish_line = |cur: &mut Vec<PlacedAtom>, lines: &mut Vec<LineBox>| {
            let line_ascent = cur.iter().map(|p| p.ascent_hint).fold(0.0_f32, f32::max);
            let line_descent = cur.iter().map(|p| p.descent_hint).fold(0.0_f32, f32::max);
            for p in cur.iter_mut() {
                p.y = line_ascent - p.ascent_hint;
            }
            lines.push(LineBox {
                atoms: std::mem::take(cur),
                height: line_ascent + line_descent,
            });
        };

        for atom in &self.content.atoms {
            let (w, h, ascent_hint, descent_hint) = match atom {
                Atom::Text { text, style } => {
                    let (w, h) = self.measure_text(state, *style, text);
                    let h = h.max(1.0);
                    let a = h * ASCENT_RATIO;
                    (w, h, a, h - a)
                }
                Atom::Math { idx: m } => {
                    let gfx = &self.content.math[*m];
                    if gfx.error.is_some() {
                        let err = gfx.error.as_ref().unwrap().to_string();
                        let (w, h) = self.measure_text(
                            state,
                            TextAtomStyle {
                                mono: true,
                                ..Default::default()
                            },
                            &err,
                        );
                        let h = h.max(1.0);
                        let a = h * ASCENT_RATIO;
                        (w, h, a, h - a)
                    } else {
                        let h = gfx.size.height;
                        let axis = AXIS_EM * math_size;
                        (gfx.size.width, h, axis + h / 2.0, (h / 2.0 - axis).max(0.0))
                    }
                }
            };
            if x > 0.0 && x + w > width && w <= width {
                finish_line(&mut cur, &mut lines);
                x = 0.0;
            }
            cur.push(PlacedAtom {
                x,
                y: 0.0,
                w,
                h,
                ascent_hint,
                descent_hint,
            });
            x += w;
        }
        if !cur.is_empty() {
            finish_line(&mut cur, &mut lines);
        }
        state.lines = lines;
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for MathFlow<'a, Message> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<FlowState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(FlowState {
            version: 0,
            width: 0.0,
            measures: HashMap::new(),
            lines: Vec::new(),
        })
    }

    fn diff(&self, tree: &mut iced::advanced::widget::Tree) {
        let state = tree.state.downcast_mut::<FlowState>();
        if state.version != 0 && state.version != self.content.version {
            state.measures.clear();
            state.lines.clear();
            state.width = 0.0;
        }
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let width = limits.max().width.max(1.0);
        let state = tree.state.downcast_mut::<FlowState>();
        self.layout_lines(state, width);

        let mut y = 0.0;
        let mut line_nodes = Vec::with_capacity(state.lines.len());
        for line in &state.lines {
            let children = line
                .atoms
                .iter()
                .map(|p| layout::Node::new(Size::new(p.w, p.h)).move_to(Point::new(p.x, y + p.y)))
                .collect();
            line_nodes.push(
                layout::Node::with_children(Size::new(width, line.height), children)
                    .move_to(Point::new(0.0, y)),
            );
            y += line.height;
        }

        layout::Node::with_children(Size::new(width, y), line_nodes)
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
        let state = tree.state.downcast_ref::<FlowState>();
        let base = self.style.color;

        let mut atom_idx = 0usize;
        for line in layout.children() {
            for atom_layout in line.children() {
                let atom = &self.content.atoms[atom_idx];
                let bounds = atom_layout.bounds();
                atom_idx += 1;
                match atom {
                    Atom::Text { text, style } => {
                        let Some(m) = state
                            .measures
                            .get(style)
                            .and_then(|per| per.get(text.as_str()))
                        else {
                            continue;
                        };
                        if style.mono {
                            renderer.fill_quad(
                                iced::advanced::renderer::Quad {
                                    bounds: Rectangle::new(
                                        bounds.position() - Vector::new(2.0, 1.0),
                                        Size::new(bounds.width + 4.0, bounds.height + 2.0),
                                    ),
                                    ..Default::default()
                                },
                                Background::Color(self.code_bg),
                            );
                        }
                        renderer.fill_paragraph(&m.paragraph, bounds.position(), base, *viewport);
                    }
                    Atom::Math { idx } => {
                        let gfx = &self.content.math[*idx];
                        if let Some(err) = &gfx.error {
                            if let Some(m) = state
                                .measures
                                .get(&TextAtomStyle {
                                    mono: true,
                                    ..Default::default()
                                })
                                .and_then(|per| per.get(err.to_string().as_str()))
                            {
                                renderer.fill_paragraph(
                                    &m.paragraph,
                                    bounds.position(),
                                    self.error_color,
                                    *viewport,
                                );
                            }
                        } else {
                            renderer.draw_svg(
                                iced::advanced::svg::Svg::new(gfx.handle.clone()),
                                bounds,
                                *viewport,
                            );
                        }
                    }
                }
            }
        }
    }

    fn update(
        &mut self,
        _tree: &mut iced::advanced::widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let Some(on_link) = &self.on_link else {
            return;
        };
        let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event else {
            return;
        };
        let Some(pos) = cursor.position() else {
            return;
        };

        let mut atom_idx = 0usize;
        for line in layout.children() {
            for atom_layout in line.children() {
                let decor = self.content.decor.get(atom_idx);
                atom_idx += 1;
                if atom_layout.bounds().contains(pos) {
                    if let Some(url) = decor.and_then(|d| d.link.as_deref()) {
                        shell.publish(on_link(url));
                    }
                }
            }
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
        let Some(pos) = cursor.position() else {
            return mouse::Interaction::None;
        };
        let mut atom_idx = 0usize;
        for line in layout.children() {
            for atom_layout in line.children() {
                let decor = self.content.decor.get(atom_idx);
                atom_idx += 1;
                if atom_layout.bounds().contains(pos)
                    && decor.and_then(|d| d.link.clone()).is_some()
                {
                    return mouse::Interaction::Pointer;
                }
            }
        }
        mouse::Interaction::None
    }
}

impl<'a, Message: 'a> From<MathFlow<'a, Message>>
    for Element<'a, Message, iced::Theme, iced::Renderer>
{
    fn from(flow: MathFlow<'a, Message>) -> Self {
        Element::new(flow)
    }
}
