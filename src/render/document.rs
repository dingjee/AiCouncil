//! 文档模型:稳定块路径 + PlainCopyProjection(PLAN 4.1 / 4.2)。
//!
//! - `PathSeg`:块身份(流式追加时前缀路径稳定);
//! - **复制投影**:可见文本投影,不是"删除所有 Markdown 符号":
//!   粗体/链接/实体取解码后文字,行内代码取内容,公式返回**含原始
//!   定界符的 raw 源文**,跨段落空行,代码保留换行缩进,表格 tab 分列。
//! - leaf 扁平化:文档 → 有序 leaf 列表,选择范围 = 路径区间上的
//!   leaf 过滤与逐 leaf 裁剪;纯函数,可单测。

use super::flow::{FlowContent, ParaFlow, CH_WHOLE};
use super::md::{self, Block, Inlines};
use unicode_segmentation::UnicodeSegmentation;

/// 块路径段。派生序:Block < Item < Cell。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PathSeg {
    Block(usize),
    Item(usize),
    Cell(usize, usize),
}

/// 文档级选择位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocPos {
    pub path: Vec<PathSeg>,
    pub atom: usize,
    pub ch: usize,
}

impl DocPos {
    pub fn new(path: Vec<PathSeg>, atom: usize, ch: usize) -> Self {
        Self { path, atom, ch }
    }
}

fn pos_key(p: &DocPos) -> (&[PathSeg], usize, usize) {
    (
        p.path.as_slice(),
        p.atom,
        if p.ch == CH_WHOLE { usize::MAX } else { p.ch },
    )
}

/// 单消息选择范围(端点无序)。
#[derive(Debug, Clone)]
pub struct Selection {
    pub a: DocPos,
    pub b: DocPos,
}

impl Selection {
    pub fn new(a: DocPos, b: DocPos) -> Self {
        Self { a, b }
    }

    /// 规范化升序端点。
    pub fn ordered(&self) -> (&DocPos, &DocPos) {
        if pos_key(&self.a) <= pos_key(&self.b) {
            (&self.a, &self.b)
        } else {
            (&self.b, &self.a)
        }
    }

    /// 端点重合(空选择)。
    pub fn is_empty(&self) -> bool {
        pos_key(&self.a) == pos_key(&self.b)
    }
}

/// 与选择范围的相对位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rel {
    Before,
    Between,
    After,
    /// 范围端点落在此 leaf(或其容器子树)。
    Endpoint {
        lo_here: bool,
        hi_here: bool,
    },
}

/// prefix 是否覆盖 path(path 位于其子树内,含相等)。
fn covers(prefix: &[PathSeg], path: &[PathSeg]) -> bool {
    path.starts_with(prefix)
}

fn relate(path: &[PathSeg], a: &[PathSeg], b: &[PathSeg]) -> Rel {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    let lo_here = covers(lo, path);
    let hi_here = covers(hi, path);
    if lo_here || hi_here {
        return Rel::Endpoint { lo_here, hi_here };
    }
    if path < lo {
        Rel::Before
    } else if path > hi {
        Rel::After
    } else {
        Rel::Between
    }
}

// ---------------------------------------------------------------------------
// leaf 扁平化
// ---------------------------------------------------------------------------

/// 与前一个 leaf 的连接方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sep {
    /// 文档第一个 leaf。
    None,
    /// 同一容器内换行(表格行间/引用行间/列表项间)。
    NewLine,
    /// 块间空行。
    ParagraphGap,
    /// 表格 cell 间。
    Tab,
}

/// 一个可选择的原子叶。
pub enum Leaf<'a> {
    Inline {
        path: Vec<PathSeg>,
        inlines: &'a Inlines,
        flow: Option<&'a ParaFlow>,
    },
    Code {
        path: Vec<PathSeg>,
        code: &'a str,
    },
    Math {
        path: Vec<PathSeg>,
        raw: &'a str,
    },
    Rule {
        path: Vec<PathSeg>,
    },
}

impl Leaf<'_> {
    fn path(&self) -> &[PathSeg] {
        match self {
            Leaf::Inline { path, .. }
            | Leaf::Code { path, .. }
            | Leaf::Math { path, .. }
            | Leaf::Rule { path } => path,
        }
    }
}

/// 输出前缀(引用/列表标记)。
#[derive(Debug, Clone, Default)]
pub struct Prefix {
    pub quote_depth: usize,
    pub marker: Option<String>,
}

impl Prefix {
    fn render(&self) -> String {
        let mut s = "> ".repeat(self.quote_depth);
        if let Some(m) = &self.marker {
            s.push_str(m);
        }
        s
    }
}

/// 扁平化文档:leaf + 连接方式 + 前缀,顺序 = 阅读顺序 = 路径字典序。
pub struct FlatDoc<'a> {
    pub leaves: Vec<(Leaf<'a>, Sep, Prefix)>,
}

impl<'a> FlatDoc<'a> {
    /// 文档 → 有序 leaf 列表。
    pub fn build(doc: &'a md::Document, flows: &'a FlowContent) -> Self {
        let mut leaves = Vec::new();
        let mut first = true;
        walk_blocks(
            &doc.blocks,
            &mut Vec::new(),
            flows,
            0,
            Sep::ParagraphGap,
            &mut None,
            &mut first,
            &mut leaves,
        );
        FlatDoc { leaves }
    }

    /// 距离 pos 最近的 leaf 路径(点击落在边距/空白时钳位)。
    pub fn clamp_path(&self, pos: &DocPos) -> Option<&[PathSeg]> {
        let mut best: Option<(&[PathSeg], usize)> = None;
        for (leaf, _, _) in &self.leaves {
            let p = leaf.path();
            let d = ordinal_distance(p, &pos.path);
            if best.map(|(_, bd)| d < bd).unwrap_or(true) {
                best = Some((p, d));
            }
        }
        best.map(|(p, _)| p)
    }

    /// 剪贴板纯文本(PLAN 4.2 合同 / T12)。
    pub fn copy_text(&self, sel: &Selection) -> String {
        let (lo, hi) = sel.ordered();
        let mut out = String::new();
        for (leaf, sep, prefix) in &self.leaves {
            let rel = relate(leaf.path(), &lo.path, &hi.path);
            let text = match rel {
                Rel::Before | Rel::After => continue,
                Rel::Between => project_full(leaf),
                Rel::Endpoint {
                    lo_here: true,
                    hi_here: true,
                } => project_between(leaf, lo, hi),
                Rel::Endpoint { lo_here: true, .. } => project_from(leaf, lo),
                Rel::Endpoint { hi_here: true, .. } => project_to(leaf, hi),
                // 不可达(端点必落在某一侧);防御性按整块处理
                Rel::Endpoint {
                    lo_here: false,
                    hi_here: false,
                } => project_full(leaf),
            };
            if matches!(leaf, Leaf::Rule { .. }) && text.is_empty() {
                continue;
            }
            match sep {
                Sep::None => {}
                // 范围首 leaf 不输出前置换行/制表符
                Sep::NewLine => {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                }
                Sep::ParagraphGap => {
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                }
                Sep::Tab => {
                    if !out.is_empty() {
                        out.push('\t');
                    }
                }
            }
            let pfx = prefix.render();
            if !pfx.is_empty() {
                out.push_str(&pfx);
            }
            out.push_str(&text);
        }
        out
    }
}

/// 路径的近似距离(仅用于钳位:公共前缀后长度差)。
fn ordinal_distance(a: &[PathSeg], b: &[PathSeg]) -> usize {
    let mut i = 0;
    while i < a.len() && i < b.len() {
        if a[i] != b[i] {
            return a.len() + b.len() - 2 * i;
        }
        i += 1;
    }
    a.len().abs_diff(b.len())
}

#[allow(clippy::too_many_arguments)]
fn walk_blocks<'a>(
    blocks: &'a [Block],
    path: &mut Vec<PathSeg>,
    flows: &'a FlowContent,
    quote_depth: usize,
    sep_default: Sep,
    list_marker: &mut Option<String>,
    first: &mut bool,
    out: &mut Vec<(Leaf<'a>, Sep, Prefix)>,
) {
    for (i, b) in blocks.iter().enumerate() {
        path.push(PathSeg::Block(i));
        match b {
            Block::Paragraph(inlines) | Block::Heading { inlines, .. } => {
                emit_leaf(
                    out,
                    first,
                    sep_default,
                    Leaf::Inline {
                        path: path.clone(),
                        inlines,
                        flow: flows.get(path).map(|a| a.as_ref()),
                    },
                    list_marker.take(),
                    quote_depth,
                );
            }
            Block::CodeBlock { code, .. } => {
                emit_leaf(
                    out,
                    first,
                    sep_default,
                    Leaf::Code {
                        path: path.clone(),
                        code,
                    },
                    list_marker.take(),
                    quote_depth,
                );
            }
            Block::MathBlock { raw, .. } => {
                emit_leaf(
                    out,
                    first,
                    sep_default,
                    Leaf::Math {
                        path: path.clone(),
                        raw,
                    },
                    list_marker.take(),
                    quote_depth,
                );
            }
            Block::Rule => {
                emit_leaf(
                    out,
                    first,
                    sep_default,
                    Leaf::Rule { path: path.clone() },
                    list_marker.take(),
                    quote_depth,
                );
            }
            Block::Quote(inner) => {
                // 引用内容各占一行
                walk_blocks(
                    inner,
                    path,
                    flows,
                    quote_depth + 1,
                    Sep::NewLine,
                    list_marker,
                    first,
                    out,
                );
            }
            Block::List { start, items } => {
                for (bi, item) in items.iter().enumerate() {
                    path.push(PathSeg::Item(bi));
                    let marker = match start {
                        Some(s) => format!("{}. ", s + bi as u64),
                        None => "- ".to_string(),
                    };
                    let mut marker_opt = Some(marker);
                    // item 内首个 leaf 带标记;项与项之间换行(非空行)
                    walk_blocks(
                        item,
                        path,
                        flows,
                        quote_depth,
                        Sep::NewLine,
                        &mut marker_opt,
                        first,
                        out,
                    );
                    path.pop();
                }
            }
            Block::Table { rows, .. } => {
                for (ri, row) in rows.iter().enumerate() {
                    for (ci, cell) in row.iter().enumerate() {
                        path.push(PathSeg::Cell(ri, ci));
                        let default_sep = if ci > 0 {
                            Sep::Tab
                        } else if ri == 0 {
                            Sep::ParagraphGap
                        } else {
                            Sep::NewLine
                        };
                        emit_leaf(
                            out,
                            first,
                            default_sep,
                            Leaf::Inline {
                                path: path.clone(),
                                inlines: cell,
                                flow: flows.get(path).map(|a| a.as_ref()),
                            },
                            None,
                            quote_depth,
                        );
                        path.pop();
                    }
                }
            }
        }
        path.pop();
    }
}

/// leaf 入列:首个 leaf 无连接;标记只消费一次。
fn emit_leaf<'a>(
    out: &mut Vec<(Leaf<'a>, Sep, Prefix)>,
    first: &mut bool,
    default_sep: Sep,
    leaf: Leaf<'a>,
    marker: Option<String>,
    quote_depth: usize,
) {
    let sep = if *first { Sep::None } else { default_sep };
    *first = false;
    out.push((
        leaf,
        sep,
        Prefix {
            quote_depth,
            marker,
        },
    ));
}

// ---------------------------------------------------------------------------
// 投影
// ---------------------------------------------------------------------------

fn project_full(leaf: &Leaf<'_>) -> String {
    match leaf {
        Leaf::Inline { inlines, .. } => inlines.plain_text(),
        Leaf::Code { code, .. } => (*code).to_string(),
        Leaf::Math { raw, .. } => (*raw).to_string(),
        Leaf::Rule { .. } => String::new(),
    }
}

/// Start 端:从 pos 到块尾。
fn project_from(leaf: &Leaf<'_>, pos: &DocPos) -> String {
    match leaf {
        Leaf::Inline { flow, .. } => flow.map_or(String::new(), |f| {
            inline_slice(f, (pos.atom, ch_norm(pos.ch)), (usize::MAX, CH_WHOLE))
        }),
        Leaf::Code { code, .. } => {
            let mut out: Vec<String> = Vec::new();
            for (i, l) in code.lines().enumerate() {
                if i > pos.atom {
                    out.push(l.to_string());
                } else if i == pos.atom {
                    out.push(grapheme_sub(l, pos.ch, usize::MAX));
                }
            }
            out.join("\n")
        }
        Leaf::Math { raw, .. } => (*raw).to_string(),
        Leaf::Rule { .. } => String::new(),
    }
}

/// End 端:从块首到 pos。
fn project_to(leaf: &Leaf<'_>, pos: &DocPos) -> String {
    match leaf {
        Leaf::Inline { flow, .. } => flow.map_or(String::new(), |f| {
            inline_slice(f, (0, 0), (pos.atom, ch_norm(pos.ch)))
        }),
        Leaf::Code { code, .. } => {
            let mut out: Vec<String> = Vec::new();
            for (i, l) in code.lines().enumerate() {
                if i < pos.atom {
                    out.push(l.to_string());
                } else if i == pos.atom {
                    out.push(grapheme_sub(l, 0, pos.ch));
                }
            }
            out.join("\n")
        }
        Leaf::Math { raw, .. } => (*raw).to_string(),
        Leaf::Rule { .. } => String::new(),
    }
}

/// 同块两端。
fn project_between(leaf: &Leaf<'_>, lo: &DocPos, hi: &DocPos) -> String {
    match leaf {
        Leaf::Inline { flow, .. } => flow.map_or(String::new(), |f| {
            inline_slice(f, (lo.atom, ch_norm(lo.ch)), (hi.atom, ch_norm(hi.ch)))
        }),
        Leaf::Code { code, .. } => {
            let mut out: Vec<String> = Vec::new();
            for (i, l) in code.lines().enumerate() {
                if i < lo.atom || i > hi.atom {
                    continue;
                }
                if i == lo.atom && i == hi.atom {
                    out.push(grapheme_sub(l, lo.ch, hi.ch));
                } else if i == lo.atom {
                    out.push(grapheme_sub(l, lo.ch, usize::MAX));
                } else if i == hi.atom {
                    out.push(grapheme_sub(l, 0, hi.ch));
                } else {
                    out.push(l.to_string());
                }
            }
            out.join("\n")
        }
        Leaf::Math { raw, .. } => (*raw).to_string(),
        Leaf::Rule { .. } => String::new(),
    }
}

fn ch_norm(ch: usize) -> usize {
    if ch == CH_WHOLE {
        0
    } else {
        ch
    }
}

/// 行内投影:atom 区间 [a, b](b 端 atom 为 usize::MAX 表示到块尾)。
/// 文本 → grapheme 子串;公式 → raw(原始定界符);硬换行 → "\n"。
fn inline_slice(flow: &ParaFlow, a: (usize, usize), b: (usize, usize)) -> String {
    let b_atom = b.0;
    let b_ch = b.1;
    let mut out = String::new();
    for (idx, atom) in flow.atoms.iter().enumerate() {
        if idx < a.0 {
            continue;
        }
        if b_atom != usize::MAX && idx > b_atom {
            break;
        }
        match atom {
            super::flow::Atom::Text { text, .. } => {
                let n = text.graphemes(true).count();
                let s = if idx == a.0 { a.1.min(n) } else { 0 };
                let e = if idx == b_atom { b_ch.min(n) } else { n };
                if s < e {
                    out.push_str(&grapheme_sub(text, s, e));
                }
            }
            super::flow::Atom::Math { idx: mi } => {
                if let Some(g) = flow.math.get(*mi) {
                    out.push_str(&g.raw);
                }
            }
            super::flow::Atom::Break => out.push('\n'),
        }
    }
    out
}

fn grapheme_sub(text: &str, s: usize, e: usize) -> String {
    text.graphemes(true)
        .skip(s)
        .take(e.saturating_sub(s))
        .collect()
}

// ---------------------------------------------------------------------------
// App 侧辅助:把 leaf 内选择裁剪为组件坐标
// ---------------------------------------------------------------------------

/// 计算 leaf 的组件内选区(None = 不在本 leaf)。
pub fn sel_for_leaf(sel: &Selection, path: &[PathSeg]) -> Option<((usize, usize), (usize, usize))> {
    let (lo, hi) = sel.ordered();
    match relate(path, &lo.path, &hi.path) {
        Rel::Before | Rel::After => None,
        Rel::Between => Some(((0, 0), (usize::MAX, CH_WHOLE))),
        Rel::Endpoint {
            lo_here: true,
            hi_here: true,
        } => Some(((lo.atom, ch_norm(lo.ch)), (hi.atom, ch_norm(hi.ch)))),
        Rel::Endpoint { lo_here: true, .. } => {
            Some(((lo.atom, ch_norm(lo.ch)), (usize::MAX, CH_WHOLE)))
        }
        Rel::Endpoint { hi_here: true, .. } => Some(((0, 0), (hi.atom, ch_norm(hi.ch)))),
        // 不可达;防御性按整块处理
        Rel::Endpoint {
            lo_here: false,
            hi_here: false,
        } => Some(((0, 0), (usize::MAX, CH_WHOLE))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::flow::FlowContent;
    use crate::render::math::MathCache;
    use crate::render::md::{parse, ParsePolicy};
    use crate::render::RenderStyle;

    fn setup(md_src: &str) -> FlatDoc<'static> {
        // 测试辅助:泄漏文档/流以获得 'static 引用(测试内可接受)
        let doc: &'static mut md::Document =
            Box::leak(Box::new(parse(md_src, ParsePolicy::Strict)));
        let mut cache = MathCache::new();
        let flows: &'static mut FlowContent = Box::leak(Box::new(FlowContent::from_document(
            doc,
            &RenderStyle::soft_sage(15.0),
            &mut cache,
        )));
        FlatDoc::build(doc, flows)
    }

    fn whole(block: usize) -> DocPos {
        DocPos::new(vec![PathSeg::Block(block)], usize::MAX, CH_WHOLE)
    }
    fn start(block: usize) -> DocPos {
        DocPos::new(vec![PathSeg::Block(block)], 0, 0)
    }

    #[test]
    fn project_bold_without_markers() {
        let flat = setup("前**粗体**后");
        let sel = Selection::new(start(0), whole(0));
        assert_eq!(flat.copy_text(&sel), "前粗体后");
    }

    #[test]
    fn project_math_returns_raw_with_delimiters() {
        let flat = setup(r"质量 $E = mc^2$ 著名");
        let sel = Selection::new(start(0), whole(0));
        assert_eq!(flat.copy_text(&sel), r"质量 $E = mc^2$ 著名");

        let flat2 = setup(r"正切 \(\tan x\) 式");
        let sel2 = Selection::new(start(0), whole(0));
        assert_eq!(flat2.copy_text(&sel2), r"正切 \(\tan x\) 式");
    }

    #[test]
    fn project_block_math_whole() {
        let flat = setup("前文\n\n$$a+b$$\n\n后文");
        let sel = Selection::new(DocPos::new(vec![PathSeg::Block(1)], 0, CH_WHOLE), whole(1));
        assert_eq!(flat.copy_text(&sel), "$$a+b$$");
    }

    #[test]
    fn project_code_preserves_lines() {
        let flat = setup("```text\nline1\n  line2\n```");
        let sel = Selection::new(start(0), whole(0));
        assert_eq!(flat.copy_text(&sel), "line1\n  line2");
    }

    #[test]
    fn project_table_tabs_and_rows() {
        let flat = setup("| a | b |\n| --- | --- |\n| 1 | 2 |");
        let seg = |r: usize, c: usize| vec![PathSeg::Block(0), PathSeg::Cell(r, c)];
        let sel = Selection::new(
            DocPos::new(seg(0, 0), 0, 0),
            DocPos::new(seg(1, 1), usize::MAX, CH_WHOLE),
        );
        assert_eq!(flat.copy_text(&sel), "a\tb\n1\t2");
    }

    #[test]
    fn project_partial_cell() {
        let flat = setup("| abc | b |\n| --- | --- |\n| 1 | 2 |");
        let seg = vec![PathSeg::Block(0), PathSeg::Cell(0, 0)];
        // atoms: "abc" 为一个原子(grapheme 0..3)
        let sel = Selection::new(DocPos::new(seg.clone(), 0, 1), DocPos::new(seg, 0, 3));
        assert_eq!(flat.copy_text(&sel), "bc");
    }

    #[test]
    fn cross_block_blank_lines() {
        let flat = setup("第一段\n\n第二段");
        let sel = Selection::new(start(0), whole(1));
        assert_eq!(flat.copy_text(&sel), "第一段\n\n第二段");
    }

    #[test]
    fn list_projection_has_markers() {
        let flat = setup("- 甲\n- 乙\n\n后续");
        let sel = Selection::new(
            DocPos::new(
                vec![PathSeg::Block(0), PathSeg::Item(0), PathSeg::Block(0)],
                0,
                0,
            ),
            whole(1),
        );
        assert_eq!(flat.copy_text(&sel), "- 甲\n- 乙\n\n后续");
    }

    #[test]
    fn quote_projection() {
        let flat = setup("上文\n\n> 引用一\n> 引用二\n\n下文");
        let sel = Selection::new(start(1), whole(1));
        let t = flat.copy_text(&sel);
        // 引用内软换行显示为空格;每行 leaf 前缀 "> "
        assert!(t.starts_with("> 引用一 引用二"), "{t}");
    }

    #[test]
    fn relate_orders_paths() {
        let a = vec![PathSeg::Block(0)];
        let b = vec![PathSeg::Block(2)];
        assert_eq!(relate(&[PathSeg::Block(1)], &a, &b), Rel::Between);
        assert_eq!(
            relate(&[PathSeg::Block(0)], &a, &b),
            Rel::Endpoint {
                lo_here: true,
                hi_here: false
            }
        );
        assert_eq!(relate(&[PathSeg::Block(3)], &a, &b), Rel::After);
    }

    #[test]
    fn streaming_prefix_selection_stable() {
        // T14:选中前缀继续追加,投影稳定(端点按实际 atom 布局计算)
        let md_a = "已稳定的开头部分";
        let md_b = "已稳定的开头部分以及后续新到内容";
        let flat = setup(md_b);
        let seg = vec![PathSeg::Block(0)];
        // 用渲染产物定位前缀末端的 (atom, ch)
        let doc = parse(md_b, ParsePolicy::Strict);
        let mut cache = MathCache::new();
        let flows = FlowContent::from_document(&doc, &RenderStyle::soft_sage(15.0), &mut cache);
        let flow = flows.get(&seg).unwrap();
        let mut remaining = md_a.graphemes(true).count();
        let mut end = (0usize, 0usize);
        for (idx, atom) in flow.atoms.iter().enumerate() {
            if let crate::render::flow::Atom::Text { text, style: _ } = atom {
                let n = text.graphemes(true).count();
                if remaining <= n {
                    end = (idx, remaining);
                    break;
                }
                remaining -= n;
            }
        }
        let sel = Selection::new(
            DocPos::new(seg.clone(), 0, 0),
            DocPos::new(seg, end.0, end.1),
        );
        assert_eq!(flat.copy_text(&sel), md_a);
    }

    #[test]
    fn whole_message_projection() {
        let flat = setup("标题段落\n\n**加粗** 与 $x^2$");
        let sel = Selection::new(start(0), whole(1));
        assert_eq!(flat.copy_text(&sel), "标题段落\n\n加粗 与 $x^2$");
    }
}
