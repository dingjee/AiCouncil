//! IR -> iced Element 视图层(PLAN 4.2 / 4.4)。
//!
//! - **所有行内内容**(段落/标题/表格 cell)走 [`flow::MathFlow`]:
//!   统一样式与局部选择表面;标题中的公式按所在级别渲染;
//! - 代码块走 [`flow::CodeFlow`]((line, col) 粒度选择) +
//!   iced_highlighter 语法高亮;
//! - 块级公式 = 单原子 MathFlow:选区相交即整式,复制 raw 源文;
//! - 表格:行高由内容决定(无 Grid 正方形默认),1px 细线经背景色
//!   缝隙一次绘制;空 cell 占位;对齐应用于文字;
//! - 标题:φ 字级(3.2),前 0.9×字号 / 后 0.35×字号 / 段间 0.55b,
//!   块首不加前距。

use std::sync::Arc;

use iced::advanced::text::Span;
use iced::widget::{column, container, row, rule, scrollable, text, Space, Stack};
use iced::{Element, Length, Padding};

use super::code_flow::CodeFlow;
use super::document::{self, PathSeg};
use super::flow::{MathFlow, ParaFlow, SelectionHooks};
use super::md::{Align, Block, Inlines};
use super::{RenderStyle, RenderedMessage};

/// 链接点击回调类型。
pub type OnLink<'a, M> = Arc<dyn Fn(String) -> M + 'a>;

/// 单元格内边距:上下 6px、左右 10px(4.4)。
const CELL_PADDING: Padding = Padding {
    top: 6.0,
    bottom: 6.0,
    left: 10.0,
    right: 10.0,
};

/// 渲染一条消息(带选择上下文)。
pub fn view_message<'a, M: Clone + 'a>(
    rendered: &'a RenderedMessage,
    style: RenderStyle,
    selection: Option<&'a document::Selection>,
    hooks: SelectionHooks<'a, M>,
    on_link: OnLink<'a, M>,
) -> Element<'a, M> {
    let mut ctx = Ctx {
        rendered,
        style,
        selection,
        hooks,
        on_link,
    };
    let mut out: Vec<Element<'a, M>> = Vec::new();
    push_blocks(
        &mut ctx,
        &rendered.doc.blocks,
        &mut Vec::new(),
        true,
        &mut out,
    );
    column(out).width(Length::Fill).into()
}

/// 块间垂直间距(3.2/4.4):
/// - 进入标题:0.9×该级字号;离开标题:0.35×字号;
/// - 段落间/默认:0.55b;表格前后:0.65b。块首不加前距。
fn gap_between(prev: Option<&Block>, next: &Block, body: f32) -> f32 {
    let para_gap = 0.55 * body;
    let top_margin = match next {
        Block::Heading { level, .. } => 0.9 * (body * crate::ui_style::heading_ratio(*level)),
        Block::Table { .. } => 0.65 * body,
        _ => para_gap,
    };
    let prev_bottom = match prev {
        Some(Block::Heading { level, .. }) => {
            0.35 * (body * crate::ui_style::heading_ratio(*level))
        }
        Some(Block::Table { .. }) => 0.65 * body,
        None => 0.0,
        _ => para_gap,
    };
    prev_bottom.max(top_margin)
}
#[allow(clippy::ptr_arg)]
fn push_blocks<'a, M: Clone + 'a>(
    ctx: &mut Ctx<'a, M>,
    blocks: &'a [Block],
    path: &mut Vec<PathSeg>,
    _is_root: bool,
    out: &mut Vec<Element<'a, M>>,
) {
    for (i, b) in blocks.iter().enumerate() {
        // 块间空隙
        if let Some(prev) = blocks.get(i.wrapping_sub(1)) {
            let gap = gap_between(Some(prev), b, ctx.style.body);
            out.push(
                Space::new()
                    .height(Length::Fixed(gap))
                    .width(Length::Fill)
                    .into(),
            );
        }
        path.push(PathSeg::Block(i));
        out.push(ctx.block(b, path));
        path.pop();
    }
}

struct Ctx<'a, M> {
    rendered: &'a RenderedMessage,
    style: RenderStyle,
    selection: Option<&'a document::Selection>,
    hooks: SelectionHooks<'a, M>,
    on_link: OnLink<'a, M>,
}

impl<'a, M: Clone + 'a> Ctx<'a, M> {
    fn leaf_sel(&self, path: &[PathSeg]) -> Option<((usize, usize), (usize, usize))> {
        self.selection.and_then(|s| document::sel_for_leaf(s, path))
    }

    fn flow_of(&self, path: &[PathSeg]) -> Option<&'a ParaFlow> {
        self.rendered.flow.get(path).map(|a| a.as_ref())
    }

    fn block(&mut self, b: &'a Block, path: &mut Vec<PathSeg>) -> Element<'a, M> {
        match b {
            Block::Heading { level, inlines } => self.heading(*level, inlines, path),
            Block::Paragraph(inlines) => self.inlines(inlines, path),
            Block::CodeBlock { language, code } => {
                self.code_block(language.as_deref(), code, path.clone())
            }
            Block::List { start, items } => self.list(*start, items, path),
            Block::Quote(inner) => {
                let mut views: Vec<Element<'a, M>> = Vec::new();
                push_blocks(self, inner, path, false, &mut views);
                let content = column(views).spacing(4.0).width(Length::Fill);
                row![
                    rule::vertical(3),
                    container(content).padding(Padding::default().left(10))
                ]
                .spacing(8)
                .width(Length::Fill)
                .into()
            }
            Block::Rule => rule::horizontal(1).into(),
            Block::Table { aligns, rows } => self.table(aligns, rows, path),
            Block::MathBlock { .. } => {
                let flow = self.rendered.flow.math_blocks.get(path).map(|a| a.as_ref());
                let Some(flow) = flow else {
                    return text("").into();
                };
                let sel = self.leaf_sel(path);
                let hooks = self.hooks.clone();
                MathFlow::new(flow, self.style)
                    .path(path.clone())
                    .selection(sel)
                    .on_link({
                        let on_link = self.on_link.clone();
                        move |url| on_link(url.to_string())
                    })
                    .selection_hooks(hooks)
                    .into()
            }
        }
    }

    #[allow(clippy::ptr_arg)]
    fn heading(
        &mut self,
        level: u8,
        _inlines: &'a Inlines,
        path: &mut Vec<PathSeg>,
    ) -> Element<'a, M> {
        let flow = self.flow_of(path);
        let Some(flow) = flow else {
            return text("").into();
        };
        let sel = self.selection.and_then(|s| document::sel_for_leaf(s, path));
        let hooks = self.hooks.clone();
        MathFlow::new(flow, self.style)
            .path(path.clone())
            .selection(sel)
            .on_link({
                let on_link = self.on_link.clone();
                move |url| on_link(url.to_string())
            })
            .selection_hooks(hooks)
            .heading(level)
            .into()
    }
    #[allow(clippy::ptr_arg)]
    fn inlines(&mut self, inlines: &'a Inlines, path: &mut Vec<PathSeg>) -> Element<'a, M> {
        let flow = self.flow_of(path);
        let Some(flow) = flow else {
            return text(inlines.plain_text()).size(self.style.body).into();
        };
        let sel = self.leaf_sel(path);
        let hooks = self.hooks.clone();
        MathFlow::new(flow, self.style)
            .path(path.clone())
            .selection(sel)
            .on_link({
                let on_link = self.on_link.clone();
                move |url| on_link(url.to_string())
            })
            .selection_hooks(hooks)
            .into()
    }

    fn code_block(
        &mut self,
        language: Option<&str>,
        code: &'a str,
        path: Vec<PathSeg>,
    ) -> Element<'a, M> {
        // 行与高亮 spans(CodeFlow 持有数据)
        let mut lines: Vec<String> = code.split('\n').map(str::to_string).collect();
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        let mut stream = iced_highlighter::Stream::new(&iced_highlighter::Settings {
            theme: iced_highlighter::Theme::InspiredGitHub,
            token: language.unwrap_or("").to_owned(),
        });
        let mut spans: Vec<Vec<Span<'static, ()>>> = Vec::with_capacity(lines.len());
        for line in &lines {
            let mut row_spans: Vec<Span<'static, ()>> = Vec::new();
            for (range, hl) in stream.highlight_line(line) {
                let mut span = Span::new(line[range].to_string());
                if let Some(c) = hl.color() {
                    span.color = Some(c);
                }
                span.font = hl.font();
                row_spans.push(span);
            }
            stream.commit();
            spans.push(row_spans);
        }

        let sel = self
            .selection
            .and_then(|s| document::sel_for_leaf(s, &path));
        let hooks = self.hooks.clone();
        let code_widget: Element<'a, M> = CodeFlow::new(lines, spans, self.style)
            .path(path)
            .selection(sel)
            .selection_hooks(hooks)
            .into();
        container(
            scrollable(container(code_widget).padding(8.0))
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::default().width(6).scroller_width(6),
                ))
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .padding(6.0)
        .style(crate::ui_style::code_container_style)
        .into()
    }

    fn list(
        &mut self,
        start: Option<u64>,
        items: &'a [Vec<Block>],
        path: &mut Vec<PathSeg>,
    ) -> Element<'a, M> {
        let mut rows = Vec::with_capacity(items.len());
        for (bi, item) in items.iter().enumerate() {
            let marker = match start {
                Some(s) => format!("{}. ", s + bi as u64),
                None => "• ".to_string(),
            };
            let mut views: Vec<Element<'a, M>> = Vec::new();
            path.push(PathSeg::Item(bi));
            push_blocks(self, item, path, false, &mut views);
            path.pop();
            let content = column(views).width(Length::Fill);
            rows.push(
                row![
                    text(marker).size(self.style.body).color(self.style.color),
                    container(content).width(Length::Fill),
                ]
                .width(Length::Fill)
                .into(),
            );
        }
        column(rows).spacing(2.0).width(Length::Fill).into()
    }

    /// 表格(4.4):行高 = 内容 + 上下 6px;列边距 10px;1px 细线经
    /// 背景色缝隙一次绘制(容器底 = line_subtle,cell 间隙 1px);
    /// 空 cell 占位;对齐应用于文字;首行表头浅底加粗。
    fn table(
        &mut self,
        aligns: &'a [Align],
        rows: &'a [Vec<Inlines>],
        path: &mut Vec<PathSeg>,
    ) -> Element<'a, M> {
        let mut row_views: Vec<Element<'a, M>> = Vec::with_capacity(rows.len());
        for (ri, r) in rows.iter().enumerate() {
            let mut cells: Vec<Element<'a, M>> = Vec::with_capacity(r.len());
            let mut backgrounds: Vec<Element<'a, M>> = Vec::with_capacity(r.len());
            for (ci, cell) in r.iter().enumerate() {
                let align = aligns.get(ci).copied().unwrap_or(Align::None);
                path.push(PathSeg::Cell(ri, ci));
                let content: Element<'a, M> = if let Some(flow) = self.flow_of(path) {
                    let on_link = self.on_link.clone();
                    MathFlow::new(flow, self.style)
                        .path(path.clone())
                        .align(align)
                        .selection(self.leaf_sel(path))
                        .selection_hooks(self.hooks.clone())
                        .on_link(move |url| on_link(url.to_string()))
                        .into()
                } else {
                    self.inlines(cell, path)
                };
                path.pop();
                let head_style = crate::ui_style::table_head_style;
                let cell_style = |t: &iced::Theme| crate::ui_style::table_cell_style(t);
                let cell = container(content).width(Length::Fill).padding(CELL_PADDING);
                cells.push(cell.into());
                backgrounds.push(
                    container(Space::new())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .style(if ri == 0 { head_style } else { cell_style })
                        .into(),
                );
            }
            // Content determines row height; every background fills that same
            // height, preserving continuous 1px column borders in wrapped rows.
            row_views.push(
                Stack::with_children([row(cells).spacing(1.0).width(Length::Fill).into()])
                    .push_under(
                        row(backgrounds)
                            .spacing(1.0)
                            .width(Length::Fill)
                            .height(Length::Fill),
                    )
                    .into(),
            );
        }
        // 网格底色容器:行间 1px 缝隙 + 外框(padding 1)
        let grid = column(row_views).spacing(1.0);
        container(container(grid).padding(1.0))
            .width(Length::Fill)
            .style(|_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(
                    crate::ui_style::palette::LINE_SUBTLE,
                )),
                border: iced::Border {
                    radius: 2.0.into(),
                    width: 0.0,
                    color: iced::Color::TRANSPARENT,
                },
                ..Default::default()
            })
            .into()
    }
}
