//! MathFlow:文本 + 行内公式的流式段落自定义组件(PLAN 4.1/4.2)。
//!
//! 这是**所有**行内内容(段落/标题/表格 cell)的统一渲染与选择表面:
//! - 原子(atom)= 拉丁词(含尾部空白)/ 单个 CJK 字符 / 行内公式(不可
//!   分割)/ 硬换行;软换行建模为空格文本原子(与显示一致)。
//! - 文本原子用 renderer 的 Paragraph 测量与绘制(cosmic-text,与 iced
//!   文本组件同一后端,中文回退一致);公式原子是 iced_math 的 SVG。
//! - **选择支持**:Paragraph hit_test(grapheme 级)+ 拖动选区 +
//!   选区高亮 + 双击选词/三击整块;公式原子不可分割——选区与之相交
//!   即包含整个公式,复制返回含原始定界符的 raw 源文(投影在 document.rs)。
//! - 鼠标事件经回调上抛(App 层 SelectionCoordinator 跨块协调)。

use std::collections::HashMap;
use std::sync::Arc;

use iced::advanced::layout::{self, Layout};
use iced::advanced::svg::Renderer as _;
use iced::advanced::text::Renderer as _;
use iced::advanced::text::{self, LineHeight, Paragraph as _, Shaping, Wrapping};
use iced::advanced::widget::tree;
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget};
use iced::{
    mouse, Background, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Vector,
};
use unicode_segmentation::UnicodeSegmentation;

use super::document::PathSeg;
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
    /// 公式 raw 源文(含原始定界符;复制投影用,PLAN 4.2 合同 3)。
    pub raw: Arc<str>,
}

/// 块内选择位置:原子序号 + 原子内 grapheme 序号。
pub type SelPos = (usize, usize);
/// 公式原子整体标记:选区与之相交即包含整个原子。
pub const CH_WHOLE: usize = usize::MAX;

#[derive(Debug, Clone, PartialEq)]
pub enum Atom {
    Text {
        text: String,
        style: TextAtomStyle,
    },
    Math {
        idx: usize,
    },
    /// 硬换行:强制断行,宽度 0。软换行为空格文本原子。
    Break,
}

/// 影响字体度量的原子样式(缓存键的一部分;颜色/链接只影响绘制)。
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

/// 一个段落的排版输入。
pub struct ParaFlow {
    pub version: u64,
    pub atoms: Vec<Atom>,
    pub decor: Vec<AtomDecor>,
    pub math: Vec<MathGfx>,
    /// 行内公式字号(基线计算用)。
    pub math_font_size: f32,
    /// 正文行高(像素;设计 1.55b)。
    pub line_height: f32,
    /// 文本字号(原子度量用)。
    pub text_size: f32,
}

/// 文档的公式排版产物:每个文本块(段落/标题/cell)一个 [`ParaFlow`],
/// 以稳定块路径为键 —— 取代旧 `paras + para_idx` 的顺序关联
/// (PLAN 4.1:标题公式不再被正文视图错取)。
#[derive(Default)]
pub struct FlowContent {
    pub flows: HashMap<Vec<PathSeg>, Arc<ParaFlow>>,
    /// 块级公式:单 Math 原子的 ParaFlow(公式原子不可分割,
    /// 选择/复制合同与行内一致;raw 源文在 MathGfx)。
    pub math_blocks: HashMap<Vec<PathSeg>, Arc<ParaFlow>>,
}

impl FlowContent {
    /// 从文档构建全部文本块的 ParaFlow 与块级公式 SVG。
    pub fn from_document(doc: &md::Document, style: &RenderStyle, cache: &mut MathCache) -> Self {
        let mut flows = HashMap::new();
        let mut math_blocks = HashMap::new();
        build_blocks(&doc.blocks, &mut Vec::new(), style, cache, &mut flows);
        collect_math(&doc.blocks, &mut Vec::new(), style, cache, &mut math_blocks);
        FlowContent { flows, math_blocks }
    }

    pub fn get(&self, path: &[PathSeg]) -> Option<&Arc<ParaFlow>> {
        self.flows.get(path)
    }
}

fn collect_math(
    blocks: &[md::Block],
    path: &mut Vec<PathSeg>,
    style: &RenderStyle,
    cache: &mut MathCache,
    out: &mut HashMap<Vec<PathSeg>, Arc<ParaFlow>>,
) {
    for (i, b) in blocks.iter().enumerate() {
        match b {
            md::Block::MathBlock { tex, raw } => {
                path.push(PathSeg::Block(i));
                let gfx = render_gfx(tex, raw, style.block_math_size(), true, style, cache);
                out.insert(
                    path.clone(),
                    Arc::new(ParaFlow {
                        version: next_version(),
                        atoms: vec![Atom::Math { idx: 0 }],
                        decor: vec![AtomDecor::default()],
                        math: vec![gfx],
                        math_font_size: style.body,
                        line_height: style.body * style.line_height,
                        text_size: style.body,
                    }),
                );
                path.pop();
            }
            md::Block::Quote(inner) => {
                path.push(PathSeg::Block(i));
                collect_math(inner, path, style, cache, out);
                path.pop();
            }
            md::Block::List { items, .. } => {
                path.push(PathSeg::Block(i));
                for (bi, blocks) in items.iter().enumerate() {
                    path.push(PathSeg::Item(bi));
                    collect_math(blocks, path, style, cache, out);
                    path.pop();
                }
                path.pop();
            }
            _ => {}
        }
    }
}

fn build_blocks(
    blocks: &[md::Block],
    path: &mut Vec<PathSeg>,
    style: &RenderStyle,
    cache: &mut MathCache,
    out: &mut HashMap<Vec<PathSeg>, Arc<ParaFlow>>,
) {
    for (i, b) in blocks.iter().enumerate() {
        match b {
            md::Block::Paragraph(inlines) | md::Block::Heading { inlines, .. } => {
                path.push(PathSeg::Block(i));
                let mut block_style = *style;
                if let md::Block::Heading { level, .. } = b {
                    block_style.body *= crate::ui_style::heading_ratio(*level);
                    block_style.line_height = crate::ui_style::HEADING_LINE_HEIGHT;
                }
                out.insert(
                    path.clone(),
                    Arc::new(atomize(inlines, &block_style, cache)),
                );
                path.pop();
            }
            md::Block::Quote(inner) => {
                path.push(PathSeg::Block(i));
                build_blocks(inner, path, style, cache, out);
                path.pop();
            }
            md::Block::List { items, .. } => {
                path.push(PathSeg::Block(i));
                for (bi, blocks) in items.iter().enumerate() {
                    path.push(PathSeg::Item(bi));
                    build_blocks(blocks, path, style, cache, out);
                    path.pop();
                }
                path.pop();
            }
            md::Block::Table { rows, .. } => {
                path.push(PathSeg::Block(i));
                for (ri, row) in rows.iter().enumerate() {
                    for (ci, cell) in row.iter().enumerate() {
                        path.push(PathSeg::Cell(ri, ci));
                        let mut flow = atomize(cell, style, cache);
                        if ri == 0 {
                            for atom in &mut flow.atoms {
                                if let Atom::Text { style, .. } = atom {
                                    style.bold = true;
                                }
                            }
                        }
                        out.insert(path.clone(), Arc::new(flow));
                        path.pop();
                    }
                }
                path.pop();
            }
            _ => {}
        }
    }
}

fn render_gfx(
    tex: &str,
    raw: &str,
    size: f32,
    display: bool,
    style: &RenderStyle,
    cache: &mut MathCache,
) -> MathGfx {
    let cached = cache.get_or_render(tex, size, display, style.color);
    MathGfx {
        handle: cached.handle.clone(),
        size: cached.size,
        error: cached.error.clone().map(Arc::from),
        raw: Arc::from(raw),
    }
}

fn push_word(word: &mut String, st: &md::Style, atoms: &mut Vec<Atom>, decor: &mut Vec<AtomDecor>) {
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
                        if ch.is_whitespace() {
                            push_word(&mut word, st, &mut atoms, &mut decor);
                        }
                    }
                }
                push_word(&mut word, st, &mut atoms, &mut decor);
            }
            md::Part::Math { tex, raw } => {
                let gfx = render_gfx(tex, raw, style.body, false, style, cache);
                atoms.push(Atom::Math { idx: math.len() });
                decor.push(AtomDecor::default());
                math.push(gfx);
            }
            md::Part::HardBreak => {
                atoms.push(Atom::Break);
                decor.push(AtomDecor::default());
            }
            md::Part::SoftBreak => {
                atoms.push(Atom::Text {
                    text: " ".into(),
                    style: TextAtomStyle::default(),
                });
                decor.push(AtomDecor::default());
            }
            md::Part::DisplayMath { .. } => {}
        }
    }
    ParaFlow {
        version: next_version(),
        atoms,
        decor,
        math,
        math_font_size: style.body,
        line_height: style.body * style.line_height,
        text_size: style.body,
    }
}

pub fn is_cjk(c: char) -> bool {
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
// 纯逻辑:选区规范化与词边界(可单测)
// ---------------------------------------------------------------------------

/// 选区端点规范化为升序。CH_WHOLE 在比较中按"整个原子"处理。
pub fn norm_sel(a: SelPos, b: SelPos) -> (SelPos, SelPos) {
    let key = |p: SelPos| (p.0, if p.1 == CH_WHOLE { usize::MAX } else { p.1 });
    if key(a) <= key(b) {
        (a, b)
    } else {
        (b, a)
    }
}

/// 双击选词边界:在文本原子的 grapheme 序列内向两侧扩展"词字符"
/// (字母数字、CJK 或其他非空白非标点——含 emoji)。
/// 返回 [start, end) grapheme 范围。
pub fn word_range(text: &str, ch: usize) -> (usize, usize) {
    let graphemes: Vec<(usize, &str)> = text.grapheme_indices(true).collect();
    if graphemes.is_empty() {
        return (0, 0);
    }
    let ch = ch.min(graphemes.len() - 1);
    let is_word = |s: &str| {
        s.chars()
            .next()
            .is_some_and(|c| !(c.is_whitespace() || c.is_ascii_punctuation() || uni_punct(c)))
    };
    if !is_word(graphemes[ch].1) {
        return (ch, ch);
    }
    let mut s = ch;
    while s > 0 && is_word(graphemes[s - 1].1) {
        s -= 1;
    }
    let mut e = ch + 1;
    while e < graphemes.len() && is_word(graphemes[e].1) {
        e += 1;
    }
    (s, e)
}

/// 常用 Unicode 标点闭集(与 md.rs compat 同口径)。
fn uni_punct(c: char) -> bool {
    matches!(c as u32,
        0x2013..=0x2022 | 0x2026 | 0x3001..=0x3011
        | 0xFF01..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65
    )
}

/// grapheme 前缀计数:byte 偏移之前有多少个 grapheme。
pub fn grapheme_index_at_byte(text: &str, byte: usize) -> usize {
    text.grapheme_indices(true)
        .take_while(|(i, _)| *i < byte)
        .count()
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
    atom_idx: usize,
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
    top: f32,
}

/// 按字体样式分组的文本测量缓存。键含内容,跨 content 版本安全复用;
/// 字号/主题变化经 style_revision 清空。
type PerFontMeasures = HashMap<String, TextMeasure>;

pub struct FlowState {
    version: u64,
    width: f32,
    style_revision: u64,
    measures: HashMap<TextAtomStyle, PerFontMeasures>,
    lines: Vec<LineBox>,
    /// 选择 UX:上次上抛的 hover 位置(去重)。
    last_hover: Option<SelPos>,
    /// 双击检测。
    last_click: Option<(std::time::Instant, f32, f32)>,
    click_count: u32,
}

/// 文本基线在行盒高度中的占比近似(cosmic-text 不暴露基线;
/// 主流 UI 字体的 ascent/(ascent+descent) ≈ 0.8)。
const ASCENT_RATIO: f32 = 0.8;
/// 数学轴高度(TeX:axis ≈ 0.25em)。
const AXIS_EM: f32 = 0.25;
/// 双击判定窗口。
const DOUBLE_CLICK_MS: u128 = 400;
/// 双击位移容差。
const DOUBLE_CLICK_SLOP: f32 = 4.0;

/// 选择回调:块路径 + 块内位置 → 消息。
pub type OnSel<'a, M> = std::sync::Arc<dyn Fn(&[PathSeg], SelPos) -> M + 'a>;
/// 选择回调:块路径 + 块内范围 → 消息。
pub type OnSelRange<'a, M> = std::sync::Arc<dyn Fn(&[PathSeg], SelPos, SelPos) -> M + 'a>;

/// 选择事件回调集(共享:每个 leaf 克隆一份)。
#[derive(Clone)]
pub struct SelectionHooks<'a, M> {
    /// 按下:开始/重设选区(锚点=焦点=该位置)。
    pub on_start: OnSel<'a, M>,
    /// 拖动/移动:延伸焦点(App 按 selecting 标志过滤)。
    pub on_extend: OnSel<'a, M>,
    /// 释放:结束并请求复制。
    pub on_finish: std::sync::Arc<dyn Fn() -> M + 'a>,
    /// 双击选词 / 三击整块(块内范围)。
    pub on_range: OnSelRange<'a, M>,
}

pub struct MathFlow<'a, Message> {
    content: &'a ParaFlow,
    style: RenderStyle,
    /// 本块在文档中的路径(事件上抛时定位;本组件不直接使用,仅标记)。
    _path: Vec<PathSeg>,
    /// 归一化升序、且已裁剪到本块的选区(None=无)。
    selection: Option<(SelPos, SelPos)>,
    on_link: Option<OnLinkFn<'a, Message>>,
    hooks: Option<SelectionHooks<'a, Message>>,
    align: md::Align,
}

/// 行内链接回调。
pub type OnLinkFn<'a, M> = Box<dyn Fn(&str) -> M + 'a>;

impl<'a, Message: Clone + 'a> MathFlow<'a, Message> {
    pub fn new(content: &'a ParaFlow, style: RenderStyle) -> Self {
        Self {
            content,
            style,
            _path: Vec::new(),
            selection: None,
            on_link: None,
            hooks: None,
            align: md::Align::None,
        }
    }

    pub fn path(mut self, path: Vec<PathSeg>) -> Self {
        self._path = path;
        self
    }

    pub fn align(mut self, align: md::Align) -> Self {
        self.align = align;
        self
    }

    pub fn selection(mut self, sel: Option<(SelPos, SelPos)>) -> Self {
        self.selection = sel;
        self
    }

    pub fn on_link(mut self, f: impl Fn(&str) -> Message + 'a) -> Self {
        self.on_link = Some(Box::new(f));
        self
    }

    pub fn selection_hooks(mut self, hooks: SelectionHooks<'a, Message>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// 标题模式:行高 1.22、H6 次要色(3.2)。字重加粗由解析层
    /// 写入原子样式(md.rs Heading 闭合)。
    pub fn heading(mut self, level: u8) -> Self {
        self.style.line_height = crate::ui_style::HEADING_LINE_HEIGHT;
        self.style.revision = self.style.revision.wrapping_add(0x1000 + level as u64);
        if level >= 6 {
            self.style.color = crate::ui_style::palette::TEXT_MUTED;
        }
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

    fn measure_text(&self, state: &mut FlowState, key: TextAtomStyle, text: &str) -> Size {
        let per_font = state.measures.entry(key).or_default();
        if let Some(m) = per_font.get(text) {
            return m.paragraph.min_bounds();
        }
        let paragraph = Para::with_text(text::Text {
            content: text,
            bounds: Size::new(65_536.0, 1_024.0),
            size: Pixels(self.content.text_size),
            line_height: LineHeight::Absolute(Pixels(self.content.line_height)),
            font: Self::font_of(key),
            align_x: text::Alignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: Shaping::Advanced,
            wrapping: Wrapping::Word,
        });
        let bounds = paragraph.min_bounds();
        per_font.insert(text.to_string(), TextMeasure { paragraph });
        bounds
    }

    /// 已测段落内 grapheme k 的 x 坐标(相对原子原点)。
    fn grapheme_x(state: &FlowState, key: TextAtomStyle, text: &str, k: usize) -> f32 {
        state
            .measures
            .get(&key)
            .and_then(|per| per.get(text))
            .and_then(|m| m.paragraph.grapheme_position(0, k))
            .map(|p| p.x)
            .unwrap_or(0.0)
    }

    fn grapheme_count(text: &str) -> usize {
        text.graphemes(true).count()
    }

    fn layout_lines(&self, state: &mut FlowState, width: f32) {
        let style_changed = state.style_revision != self.style.revision;
        if style_changed {
            state.measures.clear();
            state.style_revision = self.style.revision;
        }
        if !style_changed
            && state.version == self.content.version
            && (state.width - width).abs() < f32::EPSILON
        {
            return;
        }
        state.version = self.content.version;
        state.width = width;
        state.last_hover = None;

        let math_size = self.content.math_font_size;
        let min_line_h = self.content.line_height;
        let mut lines: Vec<LineBox> = Vec::new();
        let mut cur: Vec<PlacedAtom> = Vec::new();
        let mut x = 0.0f32;
        let mut y_base = 0.0f32;

        fn finish_line(
            cur: &mut Vec<PlacedAtom>,
            lines: &mut Vec<LineBox>,
            y_base: &mut f32,
            min_line_h: f32,
        ) {
            if cur.is_empty() {
                return;
            }
            let line_ascent = cur.iter().map(|p| p.ascent_hint).fold(0.0f32, f32::max);
            let line_descent = cur.iter().map(|p| p.descent_hint).fold(0.0f32, f32::max);
            let height = (line_ascent + line_descent).max(min_line_h);
            for p in cur.iter_mut() {
                p.y = line_ascent - p.ascent_hint;
            }
            lines.push(LineBox {
                atoms: std::mem::take(cur),
                height,
                top: *y_base,
            });
            *y_base += height;
        }

        for (atom_idx, atom) in self.content.atoms.iter().enumerate() {
            let (w, h, ascent_hint, descent_hint) = match atom {
                Atom::Break => {
                    finish_line(&mut cur, &mut lines, &mut y_base, min_line_h);
                    x = 0.0;
                    continue;
                }
                Atom::Text { text, style } => {
                    let sz = self.measure_text(state, *style, text);
                    let h = sz.height.max(1.0);
                    let a = h * ASCENT_RATIO;
                    (sz.width, h, a, h - a)
                }
                Atom::Math { idx: m } => {
                    let gfx = &self.content.math[*m];
                    if let Some(err) = gfx.error.as_deref() {
                        let err = err.to_string();
                        let sz = self.measure_text(
                            state,
                            TextAtomStyle {
                                mono: true,
                                ..Default::default()
                            },
                            &err,
                        );
                        let h = sz.height.max(1.0);
                        let a = h * ASCENT_RATIO;
                        (sz.width, h, a, h - a)
                    } else {
                        let h = gfx.size.height;
                        let axis = AXIS_EM * math_size;
                        (gfx.size.width, h, axis + h / 2.0, (h / 2.0 - axis).max(0.0))
                    }
                }
            };
            if x > 0.0 && x + w > width && w <= width {
                finish_line(&mut cur, &mut lines, &mut y_base, min_line_h);
                x = 0.0;
            }
            cur.push(PlacedAtom {
                atom_idx,
                x,
                y: 0.0,
                w,
                h,
                ascent_hint,
                descent_hint,
            });
            x += w;
        }
        finish_line(&mut cur, &mut lines, &mut y_base, min_line_h);
        if lines.is_empty() {
            lines.push(LineBox {
                atoms: Vec::new(),
                height: min_line_h,
                top: 0.0,
            });
        }
        for line in &mut lines {
            let used = line.atoms.last().map_or(0.0, |p| p.x + p.w);
            let spare = (width - used).max(0.0);
            let offset = match self.align {
                md::Align::Center => spare / 2.0,
                md::Align::Right => spare,
                _ => 0.0,
            };
            for atom in &mut line.atoms {
                atom.x += offset;
            }
        }
        state.lines = lines;
    }

    /// 命中:内容局部坐标点 → 块内 SelPos。
    fn hit_pos(state: &FlowState, content: &ParaFlow, x: f32, y: f32) -> SelPos {
        let line = state
            .lines
            .iter()
            .find(|l| y >= l.top && y < l.top + l.height)
            .or_else(|| state.lines.last());
        let Some(line) = line else {
            return (0, 0);
        };
        for p in &line.atoms {
            if x < p.x {
                // 左边距:本行第一个原子开头
                return (p.atom_idx, 0);
            }
            if x >= p.x && x < p.x + p.w {
                return Self::hit_atom(state, content, p, x - p.x);
            }
        }
        // 行尾之后:最后一个原子末尾
        match line.atoms.last() {
            Some(p) => Self::hit_atom(state, content, p, f32::INFINITY),
            None => (0, 0),
        }
    }

    fn hit_atom(state: &FlowState, content: &ParaFlow, p: &PlacedAtom, local_x: f32) -> SelPos {
        match &content.atoms[p.atom_idx] {
            Atom::Math { .. } => (p.atom_idx, CH_WHOLE),
            Atom::Break => (p.atom_idx, 0),
            Atom::Text { text, style } => {
                if local_x.is_infinite() || local_x >= p.w {
                    return (p.atom_idx, Self::grapheme_count(text));
                }
                let byte = state
                    .measures
                    .get(style)
                    .and_then(|per| per.get(text.as_str()))
                    .and_then(|m| m.paragraph.hit_test(Point::new(local_x, p.h * 0.5)))
                    .map(|hit| hit.cursor())
                    .unwrap_or(text.len());
                (p.atom_idx, grapheme_index_at_byte(text, byte))
            }
        }
    }

    /// 选区高亮矩形(内容局部坐标)。
    fn selection_rects(&self, state: &FlowState) -> Vec<Rectangle> {
        let Some((a, b)) = self.selection else {
            return Vec::new();
        };
        let (a, b) = norm_sel(a, b);
        let a_atom = a.0;
        let b_atom = b.0;
        let a_ch = if a.1 == CH_WHOLE { 0 } else { a.1 };
        let b_ch = b.1; // WHOLE 视为很大
        let mut rects = Vec::new();
        for line in &state.lines {
            for p in &line.atoms {
                if p.atom_idx < a_atom || p.atom_idx > b_atom {
                    continue;
                }
                let Atom::Text { text, style } = &self.content.atoms[p.atom_idx] else {
                    // 公式/不可分割原子:整体高亮
                    rects.push(Rectangle::new(
                        Point::new(p.x, line.top + p.y),
                        Size::new(p.w, p.h),
                    ));
                    continue;
                };
                let n = Self::grapheme_count(text);
                let s = if p.atom_idx == a_atom { a_ch.min(n) } else { 0 };
                let e = if p.atom_idx == b_atom { b_ch.min(n) } else { n };
                if s >= e {
                    continue;
                }
                let x0 = if s == 0 {
                    0.0
                } else {
                    Self::grapheme_x(state, *style, text, s)
                };
                let x1 = if e >= n {
                    p.w
                } else {
                    Self::grapheme_x(state, *style, text, e)
                };
                let width = (x1 - x0).max(2.0);
                rects.push(Rectangle::new(
                    Point::new(p.x + x0, line.top + p.y),
                    Size::new(width, p.h),
                ));
            }
        }
        rects
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for MathFlow<'a, Message>
{
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
            style_revision: 0,
            measures: HashMap::new(),
            lines: Vec::new(),
            last_hover: None,
            last_click: None,
            click_count: 0,
        })
    }

    fn diff(&self, tree: &mut iced::advanced::widget::Tree) {
        // measures 内容寻址,跨版本保留;layout 按 style_revision 清空。
        let _ = tree;
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
                // Child coordinates are relative to their line node. Adding y
                // here would apply the line offset twice during drawing.
                .map(|p| layout::Node::new(Size::new(p.w, p.h)).move_to(Point::new(p.x, p.y)))
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
        let origin = layout.bounds().position();
        let base = self.style.color;

        // 1. 选区高亮(在文字之下)
        for r in self.selection_rects(state) {
            renderer.fill_quad(
                iced::advanced::renderer::Quad {
                    bounds: Rectangle::new(Point::new(origin.x + r.x, origin.y + r.y), r.size()),
                    ..Default::default()
                },
                Background::Color(self.style.selection),
            );
        }

        // 2. 原子内容(布局子节点与 state.lines 顺序一一对应)
        let mut line_iter = state.lines.iter();
        for (li, line_layout) in layout.children().enumerate() {
            let Some(line) = line_iter.next() else {
                break;
            };
            let _ = li;
            for (ai, atom_layout) in line_layout.children().enumerate() {
                let Some(p) = line.atoms.get(ai) else {
                    break;
                };
                let bounds = atom_layout.bounds();
                match &self.content.atoms[p.atom_idx] {
                    Atom::Text { text, style } => {
                        let decor = self.content.decor.get(p.atom_idx);
                        let color = if decor.and_then(|d| d.link.as_deref()).is_some() {
                            self.style.link
                        } else {
                            base
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
                                Background::Color(self.style.code_bg),
                            );
                        }
                        if let Some(m) = state
                            .measures
                            .get(style)
                            .and_then(|per| per.get(text.as_str()))
                        {
                            renderer.fill_paragraph(
                                &m.paragraph,
                                bounds.position(),
                                color,
                                *viewport,
                            );
                        }
                        if let Some(d) = decor {
                            if d.link.is_some() {
                                let y = bounds.position().y + bounds.height - 1.5;
                                renderer.fill_quad(
                                    iced::advanced::renderer::Quad {
                                        bounds: Rectangle::new(
                                            Point::new(bounds.position().x, y),
                                            Size::new(bounds.width, 1.0),
                                        ),
                                        ..Default::default()
                                    },
                                    Background::Color(self.style.link),
                                );
                            }
                            if d.strikethrough {
                                let y = bounds.position().y + bounds.height * 0.55;
                                renderer.fill_quad(
                                    iced::advanced::renderer::Quad {
                                        bounds: Rectangle::new(
                                            Point::new(bounds.position().x, y),
                                            Size::new(bounds.width, 1.0),
                                        ),
                                        ..Default::default()
                                    },
                                    Background::Color(color),
                                );
                            }
                        }
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
                                    self.style.error,
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
                    Atom::Break => {}
                }
            }
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
        let state = tree.state.downcast_mut::<FlowState>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(pos) = cursor.position() else {
                    return;
                };
                if !bounds.contains(pos) {
                    return;
                }
                // 链接点击优先(不进入选择)
                let local = Point::new(pos.x - bounds.x, pos.y - bounds.y);
                if let Some(url) = self.link_at(state, local) {
                    if let Some(on_link) = &self.on_link {
                        shell.publish(on_link(&url));
                        return;
                    }
                }
                let hit = Self::hit_pos(state, self.content, local.x, local.y);
                // 双击/三击检测
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
                    // 三击:整块
                    let last = self.content.atoms.len().saturating_sub(1);
                    shell.publish((hooks.on_range)(&self._path, (0, 0), (last, CH_WHOLE)));
                } else if double && state.click_count == 2 {
                    // 双击:选词
                    let range: (SelPos, SelPos) = match self.content.atoms.get(hit.0) {
                        Some(Atom::Text { text, .. }) => {
                            let (ws, we) = word_range(text, hit.1.min(Self::grapheme_count(text)));
                            ((hit.0, ws), (hit.0, we))
                        }
                        _ => ((hit.0, 0), (hit.0, CH_WHOLE)),
                    };
                    shell.publish((hooks.on_range)(&self._path, range.0, range.1));
                } else {
                    shell.publish((hooks.on_start)(&self._path, hit));
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
                let hit = Self::hit_pos(state, self.content, local.x, local.y);
                if state.last_hover != Some(hit) {
                    state.last_hover = Some(hit);
                    shell.publish((hooks.on_extend)(&self._path, hit));
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
        tree: &iced::advanced::widget::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(pos) = cursor.position() else {
            return mouse::Interaction::None;
        };
        let bounds = layout.bounds();
        if !bounds.contains(pos) {
            return mouse::Interaction::None;
        }
        let state = tree.state.downcast_ref::<FlowState>();
        let local = Point::new(pos.x - bounds.x, pos.y - bounds.y);
        if self.link_at(state, local).is_some() {
            return mouse::Interaction::Pointer;
        }
        mouse::Interaction::Text
    }
}

impl<'a, Message: 'a> MathFlow<'a, Message> {
    /// 内容局部坐标下的链接命中(返回 URL)。
    fn link_at(&self, state: &FlowState, local: Point) -> Option<String> {
        let line = state
            .lines
            .iter()
            .find(|l| local.y >= l.top && local.y < l.top + l.height)?;
        for p in &line.atoms {
            if local.x >= p.x && local.x < p.x + p.w {
                return self.content.decor.get(p.atom_idx)?.link.clone();
            }
        }
        None
    }
}

impl<'a, Message: Clone + 'a> From<MathFlow<'a, Message>>
    for Element<'a, Message, iced::Theme, iced::Renderer>
{
    fn from(flow: MathFlow<'a, Message>) -> Self {
        Element::new(flow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_lines_stay_inside_layout_and_match_selection() {
        for body in [12.0, 15.0, 22.0] {
            let style = RenderStyle::soft_sage(body);
            let mut cache = MathCache::new();
            let rendered = super::super::render_message(
                "母亲接到投射后，程序从三种返还里抽取。**粗体内容**，中文与 English words wrap across lines，以及行内公式 $x^2$。反复换行时文字和选择必须处于同一行盒内。",
                &style, md::ParsePolicy::LlmCompatible, &mut cache,
            );
            let content = rendered.flow.get(&[PathSeg::Block(0)]).unwrap();
            let renderer = iced::Renderer::new(Font::DEFAULT, Pixels(body));
            for width in [90.0, 180.0, 360.0] {
                let mut flow = MathFlow::<()>::new(content, style);
                let mut tree =
                    tree::Tree::new(&flow as &dyn Widget<(), iced::Theme, iced::Renderer>);
                let node = flow.layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY)),
                );
                let state = tree.state.downcast_ref::<FlowState>();
                assert!(state.lines.len() >= 2);
                let root = Layout::with_offset(Vector::new(30.0, 50.0), &node);
                let mut previous_bottom = root.bounds().y;
                for (line, line_layout) in state.lines.iter().zip(root.children()) {
                    assert!(line_layout.bounds().y >= previous_bottom - 0.01);
                    previous_bottom = line_layout.bounds().y + line_layout.bounds().height;
                    for (atom, atom_layout) in line.atoms.iter().zip(line_layout.children()) {
                        let bounds = atom_layout.bounds();
                        assert!(bounds.y >= line_layout.bounds().y - 0.01);
                        assert!(
                            bounds.y + bounds.height <= line_layout.bounds().y + line.height + 0.01,
                            "drawn atom escapes its line: {bounds:?}, line {:?}",
                            line_layout.bounds()
                        );
                        assert!((bounds.y - root.bounds().y - line.top - atom.y).abs() < 0.01);
                        let hit = MathFlow::<()>::hit_pos(
                            state,
                            content,
                            atom.x + atom.w / 2.0,
                            line.top + line.height / 2.0,
                        );
                        assert_eq!(hit.0, atom.atom_idx);
                    }
                }
                assert!((previous_bottom - root.bounds().y - node.size().height).abs() < 0.01);
            }
        }
    }

    #[test]
    fn headings_and_empty_cells_have_their_own_metrics() {
        let style = RenderStyle::light();
        let mut cache = MathCache::new();
        let doc = md::parse(
            "# 标题 $x^2$\n\n正文\n\n| 左 | 右 |\n|---|---|\n| | 内容 |",
            md::ParsePolicy::Strict,
        );
        let content = FlowContent::from_document(&doc, &style, &mut cache);
        let heading = content.get(&[PathSeg::Block(0)]).unwrap();
        let size = style.body * crate::ui_style::heading_ratio(1);
        assert_eq!(heading.text_size, size);
        assert_eq!(heading.math_font_size, size);
        assert_eq!(
            heading.line_height,
            size * crate::ui_style::HEADING_LINE_HEIGHT
        );
        let header = content
            .get(&[PathSeg::Block(2), PathSeg::Cell(0, 0)])
            .unwrap();
        assert!(header
            .atoms
            .iter()
            .all(|atom| matches!(atom, Atom::Text { style, .. } if style.bold)));
        let empty = content
            .get(&[PathSeg::Block(2), PathSeg::Cell(1, 0)])
            .unwrap();
        let mut flow = MathFlow::<()>::new(empty, style);
        let mut tree = tree::Tree::new(&flow as &dyn Widget<(), iced::Theme, iced::Renderer>);
        let renderer = iced::Renderer::new(Font::DEFAULT, Pixels(style.body));
        let node = flow.layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(100.0, f32::INFINITY)),
        );
        assert_eq!(node.size().height, style.body * style.line_height);
    }

    #[test]
    fn english_words_wrap_and_table_alignment_moves_glyphs() {
        let style = RenderStyle::light();
        let mut cache = MathCache::new();
        let inlines = md::Inlines {
            parts: vec![md::Part::Text {
                style: md::Style::default(),
                content: "one two three four five six seven eight".into(),
            }],
            has_math: false,
        };
        let content = atomize(&inlines, &style, &mut cache);
        assert_eq!(content.atoms.len(), 8);
        for align in [md::Align::Left, md::Align::Center, md::Align::Right] {
            let flow = MathFlow::<()>::new(&content, style).align(align);
            let mut widget_state = flow.state();
            let state = widget_state.downcast_mut::<FlowState>();
            flow.layout_lines(state, 120.0);
            assert!(state.lines.len() > 1);
            for line in &state.lines {
                let first = line.atoms.first().unwrap();
                let last = line.atoms.last().unwrap();
                let right = last.x + last.w;
                assert!(right <= 120.01);
                match align {
                    md::Align::Center => assert!((first.x - (120.0 - right)).abs() < 0.01),
                    md::Align::Right => assert!((right - 120.0).abs() < 0.01),
                    _ => assert_eq!(first.x, 0.0),
                }
            }
        }
    }

    #[test]
    fn norm_sel_orders_and_whole() {
        assert_eq!(norm_sel((5, 2), (1, 3)), ((1, 3), (5, 2)));
        assert_eq!(norm_sel((2, CH_WHOLE), (2, 0)), ((2, 0), (2, CH_WHOLE)));
        assert_eq!(norm_sel((0, 4), (0, 4)), ((0, 4), (0, 4)));
    }

    #[test]
    fn word_range_cjk_and_latin() {
        assert_eq!(word_range("hello 世界", 1), (0, 5));
        assert_eq!(word_range("hello 世界", 7), (6, 8));
        // 空格处:空选择
        assert_eq!(word_range("a b", 1), (1, 1));
        // emoji 按"词字符"处理(非空白非标点):双击 a🎉b 选中整体
        assert_eq!(word_range("a🎉b", 1), (0, 3));
    }

    #[test]
    fn grapheme_index_at_byte_cjk() {
        let s = "中文abc";
        let byte_of_c = s.char_indices().nth(2).unwrap().0;
        assert_eq!(grapheme_index_at_byte(s, byte_of_c), 2);
        assert_eq!(grapheme_index_at_byte(s, s.len()), 5);
    }
}
