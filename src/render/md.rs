//! Markdown 解析(含 LaTeX 公式定界符识别)→ 块级 IR。
//!
//! 设计要点(PLAN 4.1 / 4.3):
//! - **占位符预处理**:pulldown-cmark 会把 `\[`、`\(` 等转义拆散,
//!   因此先把源文中的公式区域替换为私用区占位符(U+E000…U+E002),
//!   解析后还原为 [`Part::Math`] / [`Part::DisplayMath`]。
//! - **公式保真**:每个公式同时保存 `raw`(含**原始定界符**的源文,
//!   供复制投影)与 `tex`(渲染用内部内容);`align → aligned` 等兼容
//!   变换只发生在渲染缓存键,绝不回写或影响复制。
//! - **解析策略**:[`ParsePolicy::Strict`] 为 CommonMark 基线(用户原文);
//!   [`ParsePolicy::LlmCompatible`] 针对 assistant 输出追加保守的
//!   中文标点边界 `**…**` 强调识别(见 `compat_emphasis`),只放宽
//!   实测确认的情况,不做全局 regex,不跨容器配对。
//! - **表格**:parser 给出的每行按列数保存 cell,空 cell 不删除。
//! - `$$…$$` 与 `\[…\]` 在段落完成时拆分为独立的 [`Block::MathBlock`];
//!   未闭合的定界符保持字面(流式输出回退)。
//! - 货币保护规则(KaTeX/pandoc 兼容):开 `$` 后不能是空白;闭 `$`
//!   前不能是空白、后不能是数字。
//! - SoftBreak / HardBreak 分别建模:显示与复制投影都由此派生,
//!   不再统一折叠成空格。

use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// 解析策略(版本化;LlmCompatible 仅对 assistant 正文启用)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParsePolicy {
    /// CommonMark 基线(用户原文默认)。
    #[default]
    Strict,
    /// assistant 输出:在严格基线上追加保守兼容规则。
    LlmCompatible,
}

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
        /// 含原始定界符的完整源文(复制用)。
        raw: String,
        /// 内部内容(渲染用)。
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
    /// 是否含行内公式(决定走 MathFlow 还是纯文本快路径)。
    pub has_math: bool,
}

impl Inlines {
    /// 可见文本投影:粗体/链接取内容,实体为解码后字符(pulldown 已解),
    /// 行内代码取内容,公式取含原始定界符的源文。非"删除所有 Markdown 符号"。
    pub fn plain_text(&self) -> String {
        let mut s = String::new();
        for p in &self.parts {
            match p {
                Part::Text { content, .. } => s.push_str(content),
                Part::SoftBreak => s.push(' '),
                Part::HardBreak => s.push('\n'),
                Part::Math { raw, .. } | Part::DisplayMath { raw, .. } => s.push_str(raw),
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
    /// 软换行(源文单个换行;显示为空格)。
    SoftBreak,
    /// 硬换行(行尾两空格/反斜杠;显示为换行)。
    HardBreak,
    /// 行内公式:raw 含原始定界符($…$ 或 \(…\)),tex 为内部内容。
    Math {
        raw: String,
        tex: String,
    },
    /// 段落内出现的块级公式($$…$$ / \[…\]),段落结束时拆分。
    DisplayMath {
        raw: String,
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
pub fn parse(markdown: &str, policy: ParsePolicy) -> Document {
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
    let mut doc = builder.finish();
    if policy == ParsePolicy::LlmCompatible {
        compat_emphasis(&mut doc);
    }
    doc
}

// ---------------------------------------------------------------------------
// LlmCompatible:保守的中文标点边界强调识别(4.3)
// ---------------------------------------------------------------------------

/// 对解析后仍保持字面的 `**…**` 做保守补识别。
///
/// CommonMark 的 delimiter flanking 规则导致紧邻中文标点/引号的
/// `**` 无法开合(实测:`借用**"X"**的机制` 无 bold)。此处理只在
/// **同一个已解析 Text part 内部**寻找成对 `**`,且要求:
/// - 内容非空、无换行、内部不含 `**`;
/// - 内容首尾非空白;
/// - 开或闭定界符紧邻 CJK 字符(开侧:前为 CJK 字符且后为 Unicode
///   标点;闭侧:前为 Unicode 标点且后为 CJK 字符)——正是严格规则
///   判失败而人眼认定强调的组合;
/// - 所在 part 不在代码/链接上下文。
///
/// 不跨 part、不跨段落、不猜测未闭合结尾;单星、下划线、`##标题`、
/// 乘法等歧义场景一律不触碰。输入每字符只作有限次扫描。
fn compat_emphasis(doc: &mut Document) {
    for block in &mut doc.blocks {
        compat_block(block);
    }
}

fn compat_block(block: &mut Block) {
    match block {
        Block::Paragraph(inlines) => compat_inlines(inlines),
        Block::Heading { inlines, .. } => compat_inlines(inlines),
        Block::Quote(inner) => {
            for b in inner {
                compat_block(b);
            }
        }
        Block::List { items, .. } => {
            for blocks in items {
                for b in blocks {
                    compat_block(b);
                }
            }
        }
        Block::Table { rows, .. } => {
            for row in rows {
                for cell in row {
                    compat_inlines(cell);
                }
            }
        }
        _ => {}
    }
}

fn compat_inlines(inlines: &mut Inlines) {
    // pulldown 会把未解释的 `*` 拆成独立 Text 事件;先合并相邻同样式
    // 的纯文本 part(渲染语义不变),再做同 part 内配对。
    let mut merged: Vec<Part> = Vec::with_capacity(inlines.parts.len());
    for part in inlines.parts.drain(..) {
        match (merged.last_mut(), part) {
            (
                Some(Part::Text {
                    style: s_prev,
                    content: c_prev,
                }),
                Part::Text {
                    style: s_cur,
                    content: c_cur,
                },
            ) if *s_prev == s_cur && !s_prev.code && s_prev.link.is_none() => {
                c_prev.push_str(&c_cur);
            }
            (_, other) => merged.push(other),
        }
    }
    inlines.parts = merged;
    let mut idx = 0;
    while idx < inlines.parts.len() {
        let Part::Text { style, content } = &inlines.parts[idx] else {
            idx += 1;
            continue;
        };
        if style.code || style.link.is_some() {
            idx += 1;
            continue;
        }
        match compat_text(content) {
            Some((head, body, tail)) => {
                let base = style.clone();
                let mut replacement = Vec::with_capacity(3);
                if !head.is_empty() {
                    replacement.push(Part::Text {
                        style: base.clone(),
                        content: head,
                    });
                }
                if !body.is_empty() {
                    replacement.push(Part::Text {
                        style: Style {
                            bold: true,
                            ..base.clone()
                        },
                        content: body,
                    });
                }
                if !tail.is_empty() {
                    replacement.push(Part::Text {
                        style: base,
                        content: tail,
                    });
                }
                let n = replacement.len();
                inlines.parts.splice(idx..idx + 1, replacement);
                idx += n;
            }
            None => idx += 1,
        }
    }
}

/// 兼容扫描的一条实现:把 `text` 拆为 (前段, bold 内容, 后段)。
/// 至多处理第一对符合条件的定界符(其余留待后续版本,复杂度有界)。
fn compat_text(text: &str) -> Option<(String, String, String)> {
    let chars: Vec<char> = text.chars().collect();
    // 找所有 `**` 位置(char index)
    let mut stars = Vec::new();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '*' && chars[i + 1] == '*' {
            stars.push(i);
            i += 2;
        } else {
            i += 1;
        }
    }
    if stars.len() < 2 {
        return None;
    }
    for w in stars.windows(2) {
        let (open, close) = (w[0], w[1]);
        // 廉价检查先行,避免病态输入的重复分配(复杂度合同 4.3)
        let mut has_forbidden = false;
        for &c in &chars[open + 2..close] {
            if c == '*' || c == '\n' {
                has_forbidden = true;
                break;
            }
        }
        if has_forbidden {
            continue;
        }
        let content: Vec<char> = chars[open + 2..close].to_vec();
        if content.is_empty() {
            continue;
        }
        if content.first().is_some_and(|c| c.is_whitespace())
            || content.last().is_some_and(|c| c.is_whitespace())
        {
            continue;
        }
        let before_open = if open == 0 {
            None
        } else {
            Some(chars[open - 1])
        };
        let after_open = content.first().copied();
        let before_close = content.last().copied();
        let after_close = chars.get(close + 2).copied();
        // 开侧:前为 CJK 字符 且 后为 Unicode 标点
        let open_side = matches!(before_open, Some(c) if is_cjk_ish(c))
            && matches!(after_open, Some(c) if is_unicode_punct(c));
        // 闭侧:前为 Unicode 标点 且 后为 CJK 字符
        let close_side = matches!(before_close, Some(c) if is_unicode_punct(c))
            && matches!(after_close, Some(c) if is_cjk_ish(c));
        if open_side || close_side {
            let head: String = chars[..open].iter().collect();
            let body: String = content.iter().collect();
            let tail: String = chars[close + 2..].iter().collect();
            return Some((head, body, tail));
        }
    }
    None
}

/// CJK 表意/扩展A/日文(与 flow::is_cjk 同口径,另含全角标点由 punct 覆盖)。
fn is_cjk_ish(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF
        | 0x3400..=0x4DBF
        | 0x3040..=0x30FF
        | 0xFF00..=0xFFEF
    )
}

/// Unicode 标点(CommonMark 定义:P* 类 + 符号子集;此处取常用闭集,
/// 覆盖中英引号/括号/逗号句号等实测组合)。
fn is_unicode_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(c as u32,
            0x2013..=0x2022 // – — ‘ ’ “ ” •
            | 0x2026        // …
            | 0x2032..=0x2035
            | 0x3001..=0x3011 // 、。〝〞〈〉《》「」『』【】
            | 0xFF01..=0xFF0F // !＂#＄%＆'()*+,-./
            | 0xFF1A..=0xFF20 // :;<=>?＠
            | 0xFF3B..=0xFF40 // [\]^_`
            | 0xFF5B..=0xFF65 // {|}~〃
        )
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
    Inline { tex: String, raw: String },
    Display { tex: String, raw: String },
}

/// 把公式区域替换为占位符,并暂存公式源文(raw 含原始定界符)。
fn protect_math(src: &str) -> (String, Vec<Stashed>) {
    let mut out = String::with_capacity(src.len());
    let mut stash: Vec<Stashed> = Vec::new();

    let stash_push = |out: &mut String,
                      stash: &mut Vec<Stashed>,
                      tex: String,
                      delim_open: &str,
                      delim_close: &str| {
        let idx = stash.len();
        // 行内: $…$ 与 \(…\);块级: $$…$$ 与 \[…\]
        stash.push(match delim_open {
            "$" | "\\(" => Stashed::Inline {
                raw: format!("{delim_open}{tex}{delim_close}"),
                tex,
            },
            _ => Stashed::Display {
                raw: format!("{delim_open}{tex}{delim_close}"),
                tex,
            },
        });
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
                            let tex = src[i + 2..close].to_string();
                            stash_push(&mut out, &mut stash, tex, "\\[", "\\]");
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
                                let tex = tex.to_string();
                                stash_push(&mut out, &mut stash, tex, "\\(", "\\)");
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
                            let tex = tex.to_string();
                            stash_push(&mut out, &mut stash, tex, "$$", "$$");
                            i = close + 2;
                        }
                    } else {
                        out.push_str("$$");
                        i += 2;
                    }
                } else if let Some((close, tex)) = match_inline_dollars(bytes, i) {
                    stash_push(&mut out, &mut stash, tex, "$", "$");
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
                        Some(Stashed::Inline { tex, raw }) => {
                            parts.push(Part::Math {
                                tex: tex.clone(),
                                raw: raw.clone(),
                            });
                        }
                        Some(Stashed::Display { tex, raw }) => {
                            parts.push(Part::DisplayMath {
                                tex: tex.clone(),
                                raw: raw.clone(),
                            });
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

/// 代码内容中的占位符还原为原始源文(代码块不解析公式)。
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
                            Some(Stashed::Inline { raw, .. }) => {
                                out.push_str(raw);
                                continue;
                            }
                            Some(Stashed::Display { raw, .. }) => {
                                out.push_str(raw);
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
        cols: usize,
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
            Event::SoftBreak => {
                if let Some(Container::CodeBlock { code, .. }) = self.stack.last_mut() {
                    code.push('\n');
                } else if self.image_depth == 0 {
                    self.push_parts(vec![Part::SoftBreak]);
                }
            }
            Event::HardBreak => {
                if let Some(Container::CodeBlock { code, .. }) = self.stack.last_mut() {
                    code.push('\n');
                } else if self.image_depth == 0 {
                    self.push_parts(vec![Part::HardBreak]);
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
                let aligns: Vec<Align> = aligns
                    .into_iter()
                    .map(|a| match a {
                        pulldown_cmark::Alignment::None => Align::None,
                        pulldown_cmark::Alignment::Left => Align::Left,
                        pulldown_cmark::Alignment::Center => Align::Center,
                        pulldown_cmark::Alignment::Right => Align::Right,
                    })
                    .collect();
                let cols = aligns.len();
                self.stack.push(Container::Table {
                    aligns,
                    cols,
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
                    // 标题文字加粗(3.2:H1–H4 bold,H5/H6 视觉由标题
                    // 模式接管;原子带 bold 使 MathFlow 按粗体度量绘制)
                    let parts = parts
                        .into_iter()
                        .map(|p| match p {
                            Part::Text { mut style, content } => {
                                if !style.code {
                                    style.bold = true;
                                }
                                Part::Text { style, content }
                            }
                            other => other,
                        })
                        .collect();
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
                    self.finish_into(move |c| push_block(c, Block::CodeBlock { language, code }));
                }
            }
            TagEnd::Table => {
                let cont = self.stack.pop().unwrap();
                if let Container::Table {
                    aligns, rows, row, ..
                } = cont
                {
                    // 末行未满也保存(parser 已保证对齐;保底)
                    if !row.is_empty() {
                        let mut rows = rows;
                        rows.push(row);
                        self.finish_into(move |c| push_block(c, Block::Table { aligns, rows }));
                    } else {
                        self.finish_into(move |c| push_block(c, Block::Table { aligns, rows }));
                    }
                }
            }
            TagEnd::TableCell => {
                // 空 cell 必须保留,否则后续列左移(T11 / PLAN 2 表格列错位)
                if let Some(Container::Table { row, cell, .. }) = self.stack.last_mut() {
                    row.push(to_inlines(std::mem::take(cell)));
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(Container::Table {
                    cols, rows, row, ..
                }) = self.stack.last_mut()
                {
                    // 补齐缺失列(异常表格按列数补空,不偷偷合并)
                    while row.len() < *cols {
                        row.push(Inlines::default());
                    }
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
        let same_kind = |m: &Mark| {
            matches!(
                (m, kind),
                (Mark::Bold, MarkKind::Bold)
                    | (Mark::Italic, MarkKind::Italic)
                    | (Mark::Strike, MarkKind::Strike)
            )
        };
        if let Some(pos) = self.style_stack.iter().rposition(same_kind) {
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
                Container::Paragraph { parts: dst } | Container::Heading { parts: dst, .. } => {
                    dst.extend(parts)
                }
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
        Container::Root { blocks } | Container::Quote { blocks } | Container::Item { blocks } => {
            blocks.push(b)
        }
        _ => {}
    }
}

/// 把段落 parts 转为块序列:块级公式拆分出独立块。
fn paragraph_blocks(parts: Vec<Part>) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut current: Vec<Part> = Vec::new();

    for p in parts {
        match p {
            Part::DisplayMath { raw, tex } => {
                if !current.is_empty() {
                    blocks.push(Block::Paragraph(to_inlines(std::mem::take(&mut current))));
                }
                blocks.push(Block::MathBlock { raw, tex });
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

    fn parse_s(md: &str) -> Document {
        parse(md, ParsePolicy::Strict)
    }
    fn parse_l(md: &str) -> Document {
        parse(md, ParsePolicy::LlmCompatible)
    }

    fn parts_text(md: &str) -> Vec<String> {
        parse_s(md)
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(i) => Some(i.plain_text()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn inline_dollar_math() {
        let doc = parse_s(r"质量 $E = mc^2$ 很著名");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks);
        };
        assert!(i.has_math);
        assert_eq!(
            i.parts[1],
            Part::Math {
                raw: "$E = mc^2$".into(),
                tex: "E = mc^2".into()
            }
        );
    }

    #[test]
    fn paren_math_keeps_original_delimiters() {
        // 原文 \(…\) 复制时保留原定界形式(PLAN 4.2 合同 3)
        let doc = parse_s(r"正切 \(\tan x\) 公式");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(
            i.parts[1],
            Part::Math {
                raw: r"\(\tan x\)".into(),
                tex: r"\tan x".into()
            }
        );
        assert_eq!(i.plain_text(), r"正切 \(\tan x\) 公式");
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
        let doc = parse_s("```rust\nlet x = \"$5\";\n```\n");
        let Block::CodeBlock { language, code } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(language.as_deref(), Some("rust"));
        assert!(code.contains("\"$5\""), "{code}");
    }

    #[test]
    fn display_math_splits_paragraph_with_raw() {
        let doc = parse_s("前文 $$a+b$$ 后文");
        assert_eq!(doc.blocks.len(), 3, "{:?}", doc.blocks);
        let Block::MathBlock { raw, tex } = &doc.blocks[1] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(raw, "$$a+b$$");
        assert_eq!(tex, "a+b");
    }

    #[test]
    fn bracket_display_math() {
        let doc = parse_s("\\[ x^2 \\]");
        let Block::MathBlock { raw, tex } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(raw, "\\[ x^2 \\]");
        assert_eq!(tex.trim(), "x^2");
    }

    #[test]
    fn unclosed_dollar_stays_literal() {
        assert_eq!(
            parts_text("未闭合 $x^2 +"),
            vec!["未闭合 $x^2 +".to_string()]
        );
    }

    #[test]
    fn unclosed_display_dollar_stays_literal() {
        assert_eq!(parts_text("未闭合 $$x^2"), vec!["未闭合 $$x^2".to_string()]);
    }

    #[test]
    fn bold_italic_strike_link() {
        let doc = parse_s("*斜体*、**粗体**、~~删除~~、[链接](https://example.com)");
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
    fn table_empty_cells_preserved() {
        let doc = parse_s("| a | | c |\n| --- | --- | --- |\n| | 2 | |\n");
        let Block::Table { rows, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(row.len(), 3, "空 cell 不能丢失: {row:?}");
        }
        assert_eq!(rows[0][1].plain_text(), "");
        assert_eq!(rows[1][0].plain_text(), "");
        assert_eq!(rows[1][1].plain_text(), "2");
    }

    #[test]
    fn table_missing_trailing_cells_padded() {
        // 异常缺列:按表头列数补空(T11)
        let doc = parse_s("| a | b | c |\n| --- | --- | --- |\n| 1 |\n");
        let Block::Table { rows, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(rows[1].len(), 3);
        assert_eq!(rows[1][1].plain_text(), "");
    }

    #[test]
    fn table_cell_with_math() {
        let doc = parse_s("| $x$ | b |\n| --- | --- |\n| $y$ | 2 |\n");
        let Block::Table { rows, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(rows[0][0].has_math);
        assert!(rows[1][0].has_math);
    }

    #[test]
    fn list_parses() {
        let doc = parse_s("- 一项\n- 二项\n");
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
        let doc = parse_s("3. 三\n4. 四\n");
        let Block::List { start, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(*start, Some(3));
    }

    #[test]
    fn heading_and_quote() {
        let doc = parse_s("# 标题\n\n> 引用\n");
        assert!(matches!(doc.blocks[0], Block::Heading { level: 1, .. }));
        assert!(matches!(doc.blocks[1], Block::Quote(_)));
    }

    #[test]
    fn hard_break_is_newline_soft_break_is_space() {
        let doc = parse_s("行一  \n行二\n行三");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(i.parts.contains(&Part::HardBreak), "{:?}", i.parts);
        assert!(i.parts.contains(&Part::SoftBreak), "{:?}", i.parts);
        let pt = i.plain_text();
        assert!(pt.contains("行一\n行二 行三"), "{pt}");
    }

    // ----- LlmCompatible 兼容强调(T09) -----

    #[test]
    fn compat_cjk_quote_bold() {
        // 实测失败样例(2.2):中文 + 引号边界 → bold
        let doc = parse_l("借用**\"把负面状态传给队友\"**的机制");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(
            i.parts
                .iter()
                .any(|p| matches!(p, Part::Text { style, .. } if style.bold && content_contains(p, "把负面状态"))),
            "{:?}",
            i.parts
        );
        // 原文不回写:IR 文本不含星号定界符本身
        assert!(i.plain_text().contains("把负面状态传给队友"));
    }

    #[test]
    fn compat_cjk_fullwidth_quote_bold() {
        let doc = parse_l("借用**“把负面状态传给队友”**的机制");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(
            i.parts
                .iter()
                .any(|p| matches!(p, Part::Text { style, .. } if style.bold)),
            "{:?}",
            i.parts
        );
    }

    fn content_contains(p: &Part, needle: &str) -> bool {
        match p {
            Part::Text { content, .. } => content.contains(needle),
            _ => false,
        }
    }

    #[test]
    fn compat_strict_baseline_unchanged_cases() {
        // 正常 bold 不受影响
        let doc = parse_l("借用**把负面状态传给队友**的机制");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(i.parts.iter().any(
            |p| matches!(p, Part::Text { style, .. } if style.bold && content_contains(p, "把负面状态"))
        ));
    }

    #[test]
    fn compat_ambiguity_stays_strict() {
        // 反例(T09):乘法、单星、下划线、hashtag、未闭合 —— 全部保持字面
        for md in [
            "3 * 4 = 12 乘法",
            "a * b * c",
            "snake_case_identifier",
            "##标题无空格",
            "#hashtag 标签",
            "前文**尚未闭合",
            "snake_case 与 __双下划线__ 保持",
        ] {
            let l = parse_l(md);
            let s = parse_s(md);
            let lt: Vec<String> = l
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Paragraph(i) => Some(i.plain_text()),
                    _ => None,
                })
                .collect();
            let st: Vec<String> = s
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Paragraph(i) => Some(i.plain_text()),
                    _ => None,
                })
                .collect();
            assert_eq!(lt, st, "歧义场景 Strict/LlmCompatible 必须一致: {md}");
        }
    }

    #[test]
    fn compat_no_bold_inside_code_or_link() {
        // LlmCompatible 结果与 Strict 基线完全一致([**link**] 的 bold
        // 来自 CommonMark 本身,不是兼容层产物)
        let src = "`code ** x` 与 [**link**](https://e.com)";
        let l = parse_l(src);
        let s = parse_s(src);
        assert_eq!(format!("{l:?}"), format!("{s:?}"));
        let Block::Paragraph(i) = &l.blocks[0] else {
            panic!("{:?}", l.blocks)
        };
        assert!(
            i.parts
                .iter()
                .any(|p| matches!(p, Part::Text { style, .. } if style.link.is_some())),
            "{:?}",
            i.parts
        );
    }

    #[test]
    fn latex_braces_survive_in_math() {
        let doc = parse_s(r"集合 $\{x \mid x > 0\}$ 非空");
        let Block::Paragraph(i) = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(
            i.parts[1],
            Part::Math {
                raw: r"$\{x \mid x > 0\}$".into(),
                tex: r"\{x \mid x > 0\}".into()
            }
        );
    }

    #[test]
    fn table_parses() {
        let doc = parse_s("| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        let Block::Table { rows, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].plain_text(), "a");
        assert_eq!(rows[1][1].plain_text(), "2");
    }

    #[test]
    fn multiline_display_math() {
        let doc = parse_s("$$\n\\int_0^1 x \\, dx\n$$");
        let Block::MathBlock { tex, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks)
        };
        assert!(tex.contains(r"\int_0^1"), "{tex}");
        assert!(tex.contains(r"\, dx"), "{tex}");
    }

    #[test]
    fn dollar_inside_bold() {
        let doc = parse_s(r"**重点 $x^2$ 内容**");
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
        assert_eq!(parts_text("部分 $$x^"), vec!["部分 $$x^".to_string()]);
        let doc = parse_s("$$x^2$$");
        assert!(matches!(doc.blocks[0], Block::MathBlock { .. }));
    }

    #[test]
    fn fenced_code_with_backticks_inside_placeholder_source() {
        let doc = parse_s("````\nlet a = `$5`;\n````\n\n正文 $x$ 继续");
        assert!(matches!(doc.blocks[1], Block::Paragraph(ref p) if p.has_math));
    }

    #[test]
    fn inline_code_with_dollar_pair() {
        assert_eq!(
            parts_text("代码 `p = $q$ and $r` 保持"),
            vec!["代码 p = $q$ and $r 保持".to_string()]
        );
    }

    #[test]
    fn chunked_stream_equals_full_parse() {
        // T10 简化版:分块到达后拼接重解析 == 完整解析(同一 policy)
        let full = "## 标题\n\n借用**\"引号强调\"**的机制与 $x^2$ 公式。\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n";
        let chunks = [
            "## 标",
            "题\n\n借用**\"引号",
            "强调\"**的机制与 $x",
            "^2$ 公式。\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
        ];
        let joined: String = chunks.concat();
        assert_eq!(joined, full);
        let a = parse_l(full);
        let b = parse_l(&joined);
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }
}
