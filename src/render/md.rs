//! Markdown 解析(含 LaTeX 公式定界符识别)→ 块级 IR。
//!
//! 设计要点:
//! - **占位符预处理**:pulldown-cmark 会把 `\[`、`\(`、`\,` 等转义
//!   拆散成多个 Text 事件并做反转义,无法在事件文本上识别公式定界符。
//!   因此先把源文中的公式区域替换为私用区占位符(U+E000…U+E003),
//!   解析后再还原为 [`Part::Math`] / [`Part::DisplayMath`]。
//! - 代码保护:围栏代码块整块跳过;行内代码 span(等长反引号对)跳过;
//!   缩进代码块在事件层还原占位符。`\$` 转义为字面美元。
//! - `$$…$$` 与 `\[…\]` 在段落完成时拆分为独立的 [`Block::MathBlock`];
//!   未闭合的定界符保持字面(流式输出回退,R2)。
//! - 货币保护规则(KaTeX/pandoc 兼容):开 `$` 后不能是空白;闭 `$`
//!   前不能是空白、后不能是数字。

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// 块级文档。
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone)]
pub enum Block {
    Heading {
        level: u8,
        inlines: Inlines,
    },
    Paragraph(Inlines),
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Rule,
    Table {
        aligns: Vec<Align>,
        rows: Vec<Vec<Inlines>>, // 首行为表头
    },
    MathBlock {
        tex: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// 行内内容序列。
#[derive(Debug, Clone, Default)]
pub struct Inlines {
    pub parts: Vec<Part>,
    /// 是否含行内公式(决定走 MathFlow 还是 rich_text 快路径)。
    pub has_math: bool,
}

impl Inlines {
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for p in &self.parts {
            match p {
                Part::Text { content, .. } => s.push_str(content),
                Part::Math { tex, .. } => {
                    s.push('$');
                    s.push_str(tex);
                    s.push('$');
                }
                _ => {}
            }
        }
        s
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text {
        style: Style,
        content: String,
    },
    Math {
        tex: String,
    },
    /// 段落内出现的块级公式标记($$…$$ / \[…\]),段落结束时拆分。
    DisplayMath {
        tex: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub link: Option<String>,
}

/// 解析 Markdown 源文为块级 IR。
pub fn parse(markdown: &str) -> Document {
    let mut o = Options::empty();
    o.insert(Options::ENABLE_TABLES);
    o.insert(Options::ENABLE_STRIKETHROUGH);
    o.insert(Options::ENABLE_TASKLISTS);

    let (protected, stash) = protect_math(markdown);
    let parser = Parser::new_ext(&protected, o);

    let mut builder = Builder {
        stash,
        stack: vec![Container::Root { blocks: Vec::new() }],
        style_stack: Vec::new(),
        image_depth: 0,
    };

    for event in parser {
        builder.event(event);
    }
    builder.finish()
}

// ---------------------------------------------------------------------------
// 占位符预处理
// ---------------------------------------------------------------------------

/// 占位符定界(U+E000 / U+E001)与字面美元(U+E002)。
const PH_OPEN: char = '\u{E000}';
const PH_CLOSE: char = '\u{E001}';
const LITERAL_DOLLAR: char = '\u{E002}';

#[derive(Debug, Clone)]
enum Stashed {
    Inline(String),
    Display(String),
}

/// 把公式区域替换为占位符,并暂存公式源文。
fn protect_math(src: &str) -> (String, Vec<Stashed>) {
    let mut out = String::with_capacity(src.len());
    let mut stash: Vec<Stashed> = Vec::new();

    let stash_push = |out: &mut String, stash: &mut Vec<Stashed>, item: Stashed| {
        let idx = stash.len();
        stash.push(item);
        out.push(PH_OPEN);
        out.push_str(&idx.to_string());
        out.push(PH_CLOSE);
    };

    let bytes = src.as_bytes();
    let mut i = 0;
    let mut in_fence = false;
    let mut fence_ch = b'`';
    let mut fence_len = 0;
    // 行首状态(用于围栏检测)
    let mut line_start = true;

    while i < bytes.len() {
        let b = bytes[i];

        // 围栏代码块:行首(至多 3 空格缩进)的 ``` 或 ~~~
        if line_start && !in_fence && (b == b'`' || b == b'~') {
            let indent = out.chars().rev().take_while(|c| *c == ' ').count();
            if indent <= 3 {
                let mut n = 0;
                while i + n < bytes.len() && bytes[i + n] == b {
                    n += 1;
                }
                if n >= 3 {
                    in_fence = true;
                    fence_ch = b;
                    fence_len = n;
                    out.push_str(&src[i..i + n]);
                    i += n;
                    continue;
                }
            }
        }
        if line_start && in_fence && b == fence_ch {
            let mut n = 0;
            while i + n < bytes.len() && bytes[i + n] == fence_ch {
                n += 1;
            }
            if n >= fence_len {
                in_fence = false;
                out.push_str(&src[i..i + n]);
                i += n;
                continue;
            }
        }
        line_start = b == b'\n';

        if in_fence {
            let l = utf8_len(b);
            out.push_str(&src[i..i + l]);
            i += l;
            continue;
        }

        match b {
            b'`' => {
                // 行内代码 span:等长反引号对
                let mut n = 0;
                while i + n < bytes.len() && bytes[i + n] == b'`' {
                    n += 1;
                }
                let close = find_backtick_run(bytes, i + n, n);
                match close {
                    Some(c) => {
                        out.push_str(&src[i..c + n]);
                        i = c + n;
                    }
                    None => {
                        out.push_str(&src[i..i + n]);
                        i += n;
                    }
                }
            }
            b'\\' if i + 1 < bytes.len() => {
                match bytes[i + 1] {
                    b'$' => {
                        out.push(LITERAL_DOLLAR);
                        i += 2;
                    }
                    b'[' => {
                        if let Some(close) = find_sub(bytes, i + 2, b"\\]") {
                            stash_push(
                                &mut out,
                                &mut stash,
                                Stashed::Display(src[i + 2..close].to_string()),
                            );
                            i = close + 2;
                        } else {
                            out.push('\\');
                            i += 1;
                        }
                    }
                    b'(' => {
                        if let Some(close) = find_sub(bytes, i + 2, b"\\)") {
                            let tex = &src[i + 2..close];
                            if tex.contains('\n') {
                                // 跨行的 \(…\):不视为行内公式
                                out.push('\\');
                                i += 1;
                            } else {
                                stash_push(
                                    &mut out,
                                    &mut stash,
                                    Stashed::Inline(tex.to_string()),
                                );
                                i = close + 2;
                            }
                        } else {
                            out.push('\\');
                            i += 1;
                        }
                    }
                    _ => {
                        out.push('\\');
                        out.push(bytes[i + 1] as char);
                        i += 2;
                    }
                }
            }
            b'$' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'$' {
                    if let Some(close) = find_sub(bytes, i + 2, b"$$") {
                        let tex = &src[i + 2..close];
                        if tex.trim().is_empty() {
                            out.push_str("$$");
                            i += 2;
                        } else {
                            stash_push(
                                &mut out,
                                &mut stash,
                                Stashed::Display(tex.to_string()),
                            );
                            i = close + 2;
                        }
                    } else {
                        out.push_str("$$");
                        i += 2;
                    }
                } else if let Some((close, tex)) = match_inline_dollars(bytes, i) {
                    stash_push(&mut out, &mut stash, Stashed::Inline(tex));
                    i = close + 1;
                } else {
                    out.push('$');
                    i += 1;
                }
            }
            _ => {
                let l = utf8_len(b);
                out.push_str(&src[i..i + l]);
                i += l;
            }
        }
    }
    (out, stash)
}

/// 从 from 开始找长度恰为 n 的反引号串。
fn find_backtick_run(bytes: &[u8], from: usize, n: usize) -> Option<usize> {
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let mut m = 0;
            while i + m < bytes.len() && bytes[i + m] == b'`' {
                m += 1;
            }
            if m == n {
                return Some(i);
            }
            i += m;
        } else {
            i += utf8_len(bytes[i]);
        }
    }
    None
}

/// 行内 `$…$` 匹配。返回 (闭定界符位置, 公式内容)。
/// 规则:开 `$` 后非空白非 `$`;闭 `$` 前非空白、后非数字;内部无换行。
fn match_inline_dollars(bytes: &[u8], open: usize) -> Option<(usize, String)> {
    if open + 1 >= bytes.len() {
        return None;
    }
    let first = bytes[open + 1];
    if first.is_ascii_whitespace() || first == b'$' {
        return None;
    }
    let mut j = open + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\n' => return None,
            b'$' => {
                if bytes[j - 1].is_ascii_whitespace() {
                    return None;
                }
                if j + 1 < bytes.len() && bytes[j + 1].is_ascii_digit() {
                    // $20,000 and $30:闭定界后跟数字,视为货币
                    j += 1;
                    continue;
                }
                let tex = String::from_utf8_lossy(&bytes[open + 1..j]).into_owned();
                if tex.trim().is_empty() {
                    return None;
                }
                return Some((j, tex));
            }
            b'\\' if j + 1 < bytes.len() => j += 2,
            _ => j += utf8_len(bytes[j]),
        }
    }
    None
}

fn find_sub(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() || haystack.len() - from < needle.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// 把事件文本中的占位符还原为 parts。
fn split_placeholders(text: &str, stash: &[Stashed]) -> Vec<Part> {
    if !text.contains(PH_OPEN) && !text.contains(LITERAL_DOLLAR) {
        return vec![Part::Text {
            style: Style::default(),
            content: text.to_string(),
        }];
    }
    let mut parts = Vec::new();
    let mut buf = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            PH_OPEN => {
                // 读取到 PH_CLOSE 的数字索引
                let mut idx_str = String::new();
                let mut end = i;
                for (j, cj) in chars.by_ref() {
                    if cj == PH_CLOSE {
                        end = j;
                        break;
                    }
                    idx_str.push(cj);
                }
                if let Ok(idx) = idx_str.parse::<usize>() {
                    if !buf.is_empty() {
                        parts.push(Part::Text {
                            style: Style::default(),
                            content: std::mem::take(&mut buf),
                        });
                    }
                    match stash.get(idx) {
                        Some(Stashed::Inline(tex)) => parts.push(Part::Math { tex: tex.clone() }),
                        Some(Stashed::Display(tex)) => {
                            parts.push(Part::DisplayMath { tex: tex.clone() })
                        }
                        None => {}
                    }
                    let _ = end;
                } else {
                    buf.push(c);
                }
            }
            LITERAL_DOLLAR => buf.push('$'),
            _ => buf.push(c),
        }
    }
    if !buf.is_empty() {
        parts.push(Part::Text {
            style: Style::default(),
            content: buf,
        });
    }
    parts
}

/// 代码内容中的占位符还原为原始源文(代码块不解析公式,R2)。
fn restore_code(text: &str, stash: &[Stashed]) -> String {
    if !text.contains(PH_OPEN) && !text.contains(LITERAL_DOLLAR) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            PH_OPEN => {
                let mut idx_str = String::new();
                let mut closed = false;
                for cj in chars.by_ref() {
                    if cj == PH_CLOSE {
                        closed = true;
                        break;
                    }
                    idx_str.push(cj);
                }
                if closed {
                    if let Ok(idx) = idx_str.parse::<usize>() {
                        match stash.get(idx) {
                            Some(Stashed::Inline(tex)) => {
                                out.push('$');
                                out.push_str(tex);
                                out.push('$');
                                continue;
                            }
                            Some(Stashed::Display(tex)) => {
                                out.push_str("$$");
                                out.push_str(tex);
                                out.push_str("$$");
                                continue;
                            }
                            None => {}
                        }
                    }
                }
                out.push(PH_OPEN);
                out.push_str(&idx_str);
                if closed {
                    out.push(PH_CLOSE);
                }
            }
            LITERAL_DOLLAR => out.push('$'),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 事件驱动的 IR 构建
// ---------------------------------------------------------------------------

enum Container {
    Root {
        blocks: Vec<Block>,
    },
    Quote {
        blocks: Vec<Block>,
    },
    List {
        start: Option<u64>,
        items: Vec<Vec<Block>>,
    },
    Item {
        blocks: Vec<Block>,
    },
    Paragraph {
        parts: Vec<Part>,
    },
    Heading {
        level: u8,
        parts: Vec<Part>,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    Table {
        aligns: Vec<Align>,
        rows: Vec<Vec<Inlines>>,
        row: Vec<Inlines>,
        cell: Vec<Part>,
    },
}

#[derive(Clone)]
enum Mark {
    Bold,
    Italic,
    Strike,
    Link(String),
}

struct Builder {
    stash: Vec<Stashed>,
    stack: Vec<Container>,
    style_stack: Vec<Mark>,
    /// 图片替代文本深度:图片事件内的 Text 不进入正文。
    image_depth: u32,
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(cow) => {
                if self.image_depth > 0 {
                    return;
                }
                if let Some(Container::CodeBlock { code, .. }) = self.stack.last_mut() {
                    code.push_str(&restore_code(&cow, &self.stash));
                    return;
                }
                let parts = split_placeholders(&cow, &self.stash);
                self.push_parts(parts);
            }
            Event::Code(c) => {
                if self.image_depth > 0 {
                    return;
                }
                self.push_parts(vec![Part::Text {
                    style: Style {
                        code: true,
                        ..Default::default()
                    },
                    content: restore_code(&c, &self.stash),
                }]);
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(Container::CodeBlock { code, .. }) = self.stack.last_mut() {
                    code.push('\n');
                } else if self.image_depth == 0 {
                    self.push_parts(vec![Part::Text {
                        style: Style::default(),
                        content: " ".into(),
                    }]);
                }
            }
            Event::Rule => {
                self.finish_into(|c| push_block(c, Block::Rule));
            }
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                self.push_parts(vec![Part::Text {
                    style: Style::default(),
                    content: mark.to_string(),
                }]);
            }
            // 首版忽略:HTML、脚注、pulldown 自带 math 事件(未启用)。
            Event::Html(_)
            | Event::InlineHtml(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_)
            | Event::FootnoteReference(_) => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.stack.push(Container::Paragraph { parts: Vec::new() });
            }
            Tag::Heading { level, .. } => {
                self.stack.push(Container::Heading {
                    level: level as u8,
                    parts: Vec::new(),
                });
            }
            Tag::BlockQuote(_) => {
                self.stack.push(Container::Quote { blocks: Vec::new() });
            }
            Tag::List(start) => {
                self.stack.push(Container::List {
                    start,
                    items: Vec::new(),
                });
            }
            Tag::Item => {
                self.stack.push(Container::Item { blocks: Vec::new() });
            }
            Tag::CodeBlock(kind) => {
                let language = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(info) => {
                        let info = info.into_string();
                        info.split_whitespace().next().map(str::to_owned)
                    }
                    pulldown_cmark::CodeBlockKind::Indented => None,
                };
                self.stack.push(Container::CodeBlock {
                    language,
                    code: String::new(),
                });
            }
            Tag::Table(aligns) => {
                let aligns = aligns
                    .into_iter()
                    .map(|a| match a {
                        pulldown_cmark::Alignment::None => Align::None,
                        pulldown_cmark::Alignment::Left => Align::Left,
                        pulldown_cmark::Alignment::Center => Align::Center,
                        pulldown_cmark::Alignment::Right => Align::Right,
                    })
                    .collect();
                self.stack.push(Container::Table {
                    aligns,
                    rows: Vec::new(),
                    row: Vec::new(),
                    cell: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {}
            Tag::TableCell => {
                if let Some(Container::Table { cell, .. }) = self.stack.last_mut() {
                    cell.clear();
                }
            }
            Tag::Emphasis => self.style_stack.push(Mark::Italic),
            Tag::Strong => self.style_stack.push(Mark::Bold),
            Tag::Strikethrough => self.style_stack.push(Mark::Strike),
            Tag::Link { dest_url, .. } => {
                self.style_stack.push(Mark::Link(dest_url.into_string()));
            }
            Tag::Image { .. } => self.image_depth += 1,
            // 首版忽略
            Tag::FootnoteDefinition(_)
            | Tag::HtmlBlock
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.close_paragraph(),
            TagEnd::Heading(_) => {
                let cont = self.stack.pop().unwrap();
                if let Container::Heading { level, parts } = cont {
                    let inlines = to_inlines(parts);
                    self.finish_into(move |c| push_block(c, Block::Heading { level, inlines }));
                }
            }
            TagEnd::BlockQuote(_) => {
                let cont = self.stack.pop().unwrap();
                if let Container::Quote { blocks } = cont {
                    self.finish_into(move |c| push_block(c, Block::Quote(blocks)));
                }
            }
            TagEnd::List(_) => {
                let cont = self.stack.pop().unwrap();
                if let Container::List { start, items } = cont {
                    self.finish_into(move |c| push_block(c, Block::List { start, items }));
                }
            }
            TagEnd::Item => {
                // tight 列表里 Item 可能还有未闭合的段落
                self.close_paragraph();
                let cont = self.stack.pop().unwrap();
                if let Container::Item { blocks } = cont {
                    self.finish_into(move |c| {
                        if let Container::List { items, .. } = c {
                            items.push(blocks);
                        }
                    });
                }
            }
            TagEnd::CodeBlock => {
                let cont = self.stack.pop().unwrap();
                if let Container::CodeBlock { language, code } = cont {
                    self.finish_into(move |c| {
                        push_block(c, Block::CodeBlock { language, code })
                    });
                }
            }
            TagEnd::Table => {
                let cont = self.stack.pop().unwrap();
                if let Container::Table { aligns, rows, .. } = cont {
                    self.finish_into(move |c| push_block(c, Block::Table { aligns, rows }));
                }
            }
            TagEnd::TableCell => {
                if let Some(Container::Table { row, cell, .. }) = self.stack.last_mut() {
                    if !cell.is_empty() {
                        row.push(to_inlines(std::mem::take(cell)));
                    }
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(Container::Table { rows, row, .. }) = self.stack.last_mut() {
                    if !row.is_empty() {
                        rows.push(std::mem::take(row));
                    }
                }
            }
            TagEnd::Emphasis => self.pop_mark(&MarkKind::Italic),
            TagEnd::Strong => self.pop_mark(&MarkKind::Bold),
            TagEnd::Strikethrough => self.pop_mark(&MarkKind::Strike),
            TagEnd::Link => {
                if let Some(pos) = self
                    .style_stack
                    .iter()
                    .rposition(|m| matches!(m, Mark::Link(_)))
                {
                    self.style_stack.remove(pos);
                }
            }
            TagEnd::Image => self.image_depth = self.image_depth.saturating_sub(1),
            _ => {}
        }
    }

    fn pop_mark(&mut self, kind: &MarkKind) {
        let same_kind = |m: &Mark| match (m, kind) {
            (Mark::Bold, MarkKind::Bold)
            | (Mark::Italic, MarkKind::Italic)
            | (Mark::Strike, MarkKind::Strike) => true,
            _ => false,
        };
        if let Some(pos) = self.style_stack.iter().rposition(|m| same_kind(m)) {
            self.style_stack.remove(pos);
        }
    }

    fn close_paragraph(&mut self) {
        if !matches!(self.stack.last(), Some(Container::Paragraph { .. })) {
            return;
        }
        let cont = self.stack.pop().unwrap();
        if let Container::Paragraph { parts } = cont {
            for block in paragraph_blocks(parts) {
                self.finish_into(move |c| push_block(c, block));
            }
        }
    }

    fn push_parts(&mut self, parts: Vec<Part>) {
        let style = self.current_style();
        let parts: Vec<Part> = parts
            .into_iter()
            .map(|p| match p {
                Part::Text { content, style: s } => Part::Text {
                    style: if s.code {
                        Style {
                            code: true,
                            link: style.link.clone(),
                            ..Default::default()
                        }
                    } else {
                        style.clone()
                    },
                    content,
                },
                other => other,
            })
            .collect();

        // tight 列表:文本直接出现在 Item 下,合成段落容器
        if matches!(self.stack.last(), Some(Container::Item { .. })) {
            self.stack.push(Container::Paragraph { parts: Vec::new() });
        }

        if let Some(top) = self.stack.last_mut() {
            match top {
                Container::Paragraph { parts: dst }
                | Container::Heading { parts: dst, .. } => dst.extend(parts),
                Container::Table { cell, .. } => cell.extend(parts),
                _ => {}
            }
        }
    }

    fn current_style(&self) -> Style {
        let mut s = Style::default();
        for m in &self.style_stack {
            match m {
                Mark::Bold => s.bold = true,
                Mark::Italic => s.italic = true,
                Mark::Strike => s.strikethrough = true,
                Mark::Link(url) => s.link = Some(url.clone()),
            }
        }
        s
    }

    fn finish_into(&mut self, f: impl FnOnce(&mut Container)) {
        if let Some(top) = self.stack.last_mut() {
            f(top);
        }
    }

    fn finish(self) -> Document {
        let mut blocks = Vec::new();
        let mut it = self.stack.into_iter();
        if let Some(Container::Root { blocks: root }) = it.next() {
            blocks = root;
        }
        Document { blocks }
    }
}

enum MarkKind {
    Bold,
    Italic,
    Strike,
}

fn push_block(c: &mut Container, b: Block) {
    match c {
        Container::Root { blocks }
        | Container::Quote { blocks }
        | Container::Item { blocks } => blocks.push(b),
        _ => {}
    }
}

/// 把段落 parts 转为块序列:块级公式拆分出独立块。
fn paragraph_blocks(parts: Vec<Part>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current: Vec<Part> = Vec::new();

    for p in parts {
        match p {
            Part::DisplayMath { tex } => {
                if !current.is_empty() {
                    blocks.push(Block::Paragraph(to_inlines(std::mem::take(&mut current))));
                }
                blocks.push(Block::MathBlock { tex });
            }
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        blocks.push(Block::Paragraph(to_inlines(current)));
    }
    blocks
}

fn to_inlines(mut parts: Vec<Part>) -> Inlines {
    let mut has_math = false;
    for p in &parts {
        if matches!(p, Part::Math { .. }) {
            has_math = true;
        }
    }
    // 去除首尾纯空白
    while let Some(Part::Text { content, .. }) = parts.first() {
        if content.trim().is_empty() {
            parts.remove(0);
        } else {
            break;
        }
    }
    while let Some(Part::Text { content, .. }) = parts.last() {
        if content.trim().is_empty() {
            parts.pop();
        } else {
            break;
        }
    }
    Inlines { parts, has_math }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts_text(md: &str) -> Vec<String> {
        let doc = parse(md);
        doc.blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(i) => Some(i.plain_text()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn inline_dollar_math() {
        let doc = parse(r"质量 $E = mc^2$ 很著名");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks);
        };
        assert!(i.has_math);
        assert_eq!(
            i.parts[0],
            Part::Text {
                style: Style::default(),
                content: "质量 ".into()
            }
        );
        assert_eq!(i.parts[1], Part::Math { tex: "E = mc^2".into() });
    }

    #[test]
    fn currency_not_math() {
        assert_eq!(
            parts_text(r"价格是 $100 左右"),
            vec!["价格是 $100 左右".to_string()]
        );
        assert_eq!(
            parts_text(r"成本 $3 与 $5 之间"),
            vec!["成本 $3 与 $5 之间".to_string()]
        );
        assert_eq!(
            parts_text(r"从 $20,000 到 $30,000 的范围"),
            vec!["从 $20,000 到 $30,000 的范围".to_string()]
        );
    }

    #[test]
    fn escaped_dollar_literal() {
        assert_eq!(
            parts_text(r"成本 \$x\$ 与 \$y\$ 保持字面"),
            vec!["成本 $x$ 与 $y$ 保持字面".to_string()]
        );
    }

    #[test]
    fn code_span_protects_dollar() {
        assert_eq!(
            parts_text("行内代码 `a $ b` 中的美元"),
            vec!["行内代码 a $ b 中的美元".to_string()]
        );
    }

    #[test]
    fn code_block_protects_dollar() {
        let doc = parse("```rust\nlet x = \"$5\";\n```\n");
        let Block::CodeBlock { language, code } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert!(code.contains("\"$5\""), "{code}");
    }

    #[test]
    fn display_math_splits_paragraph() {
        let doc = parse("前文 $$a+b$$ 后文");
        assert_eq!(doc.blocks.len(), 3, "{:?}", doc.blocks);
        assert!(matches!(doc.blocks[1], Block::MathBlock { .. }));
    }

    #[test]
    fn bracket_display_math() {
        let doc = parse("\\[ x^2 \\]");
        let Block::MathBlock { tex } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(tex.trim(), "x^2");
    }

    #[test]
    fn paren_inline_math() {
        let doc = parse(r"正切 \(\tan x\) 公式");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(i.has_math);
        assert_eq!(
            i.parts[1],
            Part::Math {
                tex: r"\tan x".into()
            }
        );
    }

    #[test]
    fn unclosed_dollar_stays_literal() {
        assert_eq!(parts_text("未闭合 $x^2 +"), vec!["未闭合 $x^2 +".to_string()]);
    }

    #[test]
    fn unclosed_display_dollar_stays_literal() {
        assert_eq!(parts_text("未闭合 $$x^2"), vec!["未闭合 $$x^2".to_string()]);
    }

    #[test]
    fn latex_braces_survive_in_math() {
        let doc = parse(r"集合 $\{x \mid x > 0\}$ 非空");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(
            i.parts[1],
            Part::Math {
                tex: r"\{x \mid x > 0\}".into()
            }
        );
    }

    #[test]
    fn bold_italic_strike_link() {
        let doc = parse("*斜体*、**粗体**、~~删除~~、[链接](https://example.com)");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        let styles: Vec<&Style> = i
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { style, .. } => Some(style),
                _ => None,
            })
            .collect();
        assert!(styles.iter().any(|s| s.italic), "{:?}", styles);
        assert!(styles.iter().any(|s| s.bold), "{:?}", styles);
        assert!(styles.iter().any(|s| s.strikethrough), "{:?}", styles);
        assert!(
            styles
                .iter()
                .any(|s| s.link.as_deref() == Some("https://example.com")),
            "{:?}",
            styles
        );
    }

    #[test]
    fn table_parses() {
        let doc = parse("| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        let Block::Table { rows, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].plain_text(), "a");
        assert_eq!(rows[1][1].plain_text(), "2");
    }

    #[test]
    fn table_cell_with_math() {
        let doc = parse("| $x$ | b |\n| --- | --- |\n| $y$ | 2 |\n");
        let Block::Table { rows, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(rows[0][0].has_math);
        assert!(rows[1][0].has_math);
    }

    #[test]
    fn list_parses() {
        let doc = parse("- 一项\n- 二项\n");
        let Block::List { start, items } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(*start, None);
        assert_eq!(items.len(), 2);
        let Block::Paragraph(i) = &items[0][0] else {
            panic!("{:?}", items[0])
        };
        assert_eq!(i.plain_text().trim_end(), "一项");
    }

    #[test]
    fn ordered_list_start() {
        let doc = parse("3. 三\n4. 四\n");
        let Block::List { start, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(*start, Some(3));
    }

    #[test]
    fn heading_and_quote() {
        let doc = parse("# 标题\n\n> 引用\n");
        assert!(matches!(doc.blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(doc.blocks[1], Block::Quote(_)));
    }

    #[test]
    fn multiline_display_math() {
        let doc = parse("$$\n\\int_0^1 x \\, dx\n$$");
        let Block::MathBlock { tex } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(tex.contains(r"\int_0^1"), "{tex}");
        assert!(tex.contains(r"\, dx"), "{tex}");
    }

    #[test]
    fn dollar_inside_bold() {
        let doc = parse(r"**重点 $x^2$ 内容**");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(i.has_math);
        let has_bold = i
            .parts
            .iter()
            .any(|p| matches!(p, Part::Text { style, .. } if style.bold));
        assert!(has_bold, "{:?}", i.parts);
    }

    #[test]
    fn streaming_unclosed_then_closed() {
        // 流式:先未闭合(字面),闭合后应识别为公式 —— 重新解析即可
        assert_eq!(parts_text("部分 $$x^"), vec!["部分 $$x^".to_string()]);
        let doc = parse("$$x^2$$");
        assert!(matches!(doc.blocks[0], Block::MathBlock { .. }));
    }

    #[test]
    fn fenced_code_with_backticks_inside_placeholder_source() {
        // 代码块里包含 $$$ 与反引号,不破坏外层解析
        let doc = parse("````\nlet a = `$5`;\n````\n\n正文 $x$ 继续");
        assert!(matches!(doc.blocks[1], Block::Paragraph(ref p) if p.has_math));
    }

    #[test]
    fn inline_code_with_dollar_pair() {
        assert_eq!(
            parts_text("代码 `p = $q$ and $r` 保持"),
            vec!["代码 p = $q$ and $r 保持".to_string()]
        );
    }
}
