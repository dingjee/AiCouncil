//! IR -> iced Element 视图层。
//!
//! - 无公式的段落走 `rich_text` 快路径(完整样式与链接支持);
//! - 含行内公式的段落走 [`crate::render::flow::MathFlow`];
//! - 代码块用 iced_highlighter(与 iced markdown 组件同一引擎)做
//!   语法高亮,横向滚动;
//! - 块级公式居中,超宽时横向滚动;
//! - 表格用 grid,列表/引用用原生组合。

use std::sync::Arc;

use iced::advanced::text::{Highlight, Span};
use iced::widget::{self, column, container, rich_text, row, rule, scrollable, svg as svg_widget, text};
use iced::{Alignment, Element, Font, Length, Padding};

use super::flow::{FlowContent, MathFlow, MathGfx, ParaFlow};
use super::md::{Align, Block, Inlines, Part, Style as MdStyle};
use super::{RenderedMessage, RenderStyle};

/// 链接点击回调类型。
pub type OnLink<'a, M> = Arc<dyn Fn(String) -> M + 'a>;

const CODE_BG: iced::Color = iced::Color::from_rgb(0.955, 0.96, 0.97);
const LINK_COLOR: iced::Color = iced::Color::from_rgb(0.05, 0.35, 0.75);
const ERROR_COLOR: iced::Color = iced::Color::from_rgb(0.75, 0.22, 0.17);

/// 渲染整条消息。
pub fn view_message<'a, M: 'a>(
    rendered: &'a RenderedMessage,
    style: RenderStyle,
    on_link: OnLink<'a, M>,
) -> Element<'a, M> {
    let mut ctx = Ctx {
        flows: &rendered.flow,
        para_idx: 0,
        block_math_idx: 0,
        style,
        on_link,
    };
    let views: Vec<Element<'a, M>> = rendered.doc.blocks.iter().map(|b| ctx.block(b)).collect();
    column(views).spacing(8.0).width(Length::Fill).into()
}

struct Ctx<'a, M> {
    flows: &'a FlowContent,
    para_idx: usize,
    block_math_idx: usize,
    style: RenderStyle,
    on_link: OnLink<'a, M>,
}

impl<'a, M: 'a> Ctx<'a, M> {
    fn block(&mut self, b: &'a Block) -> Element<'a, M> {
        match b {
            Block::Heading { level, inlines } => self.heading(*level, inlines),
            Block::Paragraph(inlines) => self.inlines(inlines),
            Block::CodeBlock { language, code } => {
                code_block_view(language.as_deref(), code, self.style)
            }
            Block::List { start, items } => self.list(*start, items),
            Block::Quote(inner) => {
                let views: Vec<Element<'a, M>> = inner.iter().map(|b| self.block(b)).collect();
                let content = column(views).spacing(6.0).width(Length::Fill);
                row![
                    rule::vertical(3),
                    container(content).padding(Padding::default().left(10))
                ]
                .spacing(8)
                .width(Length::Fill)
                .into()
            }
            Block::Rule => rule::horizontal(1).into(),
            Block::Table { aligns, rows } => self.table(aligns, rows),
            Block::MathBlock { .. } => {
                let gfx = self.flows.blocks.get(self.block_math_idx).cloned();
                self.block_math_idx += 1;
                match gfx {
                    Some(g) => math_block_view(g),
                    None => text("").into(),
                }
            }
        }
    }

    fn heading(&mut self, level: u8, inlines: &'a Inlines) -> Element<'a, M> {
        // 标题中的行内公式按源文字面显示(罕见场景;公式验收在正文/块级)
        let size = match level {
            1 => self.style.text_size * 2.0,
            2 => self.style.text_size * 1.7,
            3 => self.style.text_size * 1.45,
            4 => self.style.text_size * 1.25,
            5 => self.style.text_size * 1.1,
            _ => self.style.text_size,
        };
        let mut spans = rich_spans(inlines, size);
        for s in &mut spans {
            s.font = Some(match s.font {
                Some(f) => Font {
                    weight: iced::font::Weight::Bold,
                    ..f
                },
                None => Font {
                    weight: iced::font::Weight::Bold,
                    ..Font::DEFAULT
                },
            });
        }
        rich_text(spans).size(size).width(Length::Fill).into()
    }

    fn inlines(&mut self, inlines: &'a Inlines) -> Element<'a, M> {
        if !inlines.has_math {
            let spans = rich_spans(inlines, self.style.text_size);
            return rich_text(spans)
                .size(self.style.text_size)
                .width(Length::Fill)
                .into();
        }
        let flows = self.flows;
        let flow: &'a ParaFlow = flows
            .paras
            .get(self.para_idx)
            .expect("flow paragraphs out of sync with document");
        self.para_idx += 1;
        let on_link = self.on_link.clone();
        let style = self.style;
        MathFlow::new(flow, style)
            .on_link(move |url| on_link(url.to_string()))
            .into()
    }

    fn list(&mut self, start: Option<u64>, items: &'a [Vec<Block>]) -> Element<'a, M> {
        let mut rows = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let marker = match start {
                Some(s) => format!("{}. ", s + i as u64),
                None => "• ".to_string(),
            };
            let views: Vec<Element<'a, M>> = item.iter().map(|b| self.block(b)).collect();
            let content = column(views).spacing(4.0).width(Length::Fill);
            rows.push(
                row![
                    text(marker)
                        .size(self.style.text_size)
                        .color(self.style.color),
                    content
                ]
                .into(),
            );
        }
        column(rows).spacing(4.0).width(Length::Fill).into()
    }

    fn table(&mut self, aligns: &'a [Align], rows: &'a [Vec<Inlines>]) -> Element<'a, M> {
        let mut grid = widget::Grid::new().columns(aligns.len().max(1)).spacing(8);
        for (ri, row) in rows.iter().enumerate() {
            for (ci, cell) in row.iter().enumerate() {
                let align = aligns.get(ci).copied().unwrap_or(Align::None);
                let content = self.inlines(cell);
                let content = if ri == 0 {
                    // 表头加粗
                    let mut spans = rich_spans(cell, self.style.text_size);
                    for s in &mut spans {
                        s.font = Some(match s.font {
                            Some(f) => Font {
                                weight: iced::font::Weight::Bold,
                                ..f
                            },
                            None => Font {
                                weight: iced::font::Weight::Bold,
                                ..Font::DEFAULT
                            },
                        });
                    }
                    rich_text(spans)
                        .size(self.style.text_size)
                        .width(Length::Fill)
                        .into()
                } else {
                    content
                };
                let cell = container(content).width(Length::Fill).align_x(match align {
                    Align::Center => Alignment::Center,
                    Align::Right => Alignment::End,
                    _ => Alignment::Start,
                });
                grid = grid.push(cell);
            }
        }
        container(grid).width(Length::Fill).into()
    }
}

/// 无公式段落的 rich_text spans。
fn rich_spans<'a>(inlines: &'a Inlines, size: f32) -> Vec<Span<'a, String>> {
    let mut spans = Vec::new();
    for part in &inlines.parts {
        let Part::Text { style: st, content } = part else {
            continue;
        };
        let mut span = Span::new(content.as_str()).size(iced::Pixels(size));
        span.font = Some(md_font(st));
        if let Some(url) = &st.link {
            span.link = Some(url.clone());
            span.underline = true;
            span.color = Some(LINK_COLOR);
        }
        if st.strikethrough {
            span.strikethrough = true;
        }
        if st.code {
            span.padding = Padding::new(2.0);
            span.highlight = Some(Highlight {
                background: iced::Background::Color(CODE_BG),
                border: Default::default(),
            });
        }
        spans.push(span);
    }
    spans
}

fn md_font(st: &MdStyle) -> Font {
    Font {
        family: if st.code {
            iced::font::Family::Monospace
        } else {
            iced::font::Family::SansSerif
        },
        weight: if st.bold {
            iced::font::Weight::Bold
        } else {
            iced::font::Weight::Normal
        },
        stretch: Default::default(),
        style: if st.italic {
            iced::font::Style::Italic
        } else {
            iced::font::Style::Normal
        },
    }
}

/// 代码块:iced_highlighter 语法高亮 + 横向滚动 + 圆角底色容器。
fn code_block_view<'a, M: 'a>(language: Option<&str>, code: &str, style: RenderStyle) -> Element<'a, M> {
    let code_size = style.text_size * 0.85;
    let mut stream = iced_highlighter::Stream::new(&iced_highlighter::Settings {
        theme: iced_highlighter::Theme::InspiredGitHub,
        token: language.unwrap_or("").to_owned(),
    });

    let line_elements: Vec<Element<'a, M>> = code
        .split('\n')
        .map(|line| {
            let mut spans: Vec<Span<'a, ()>> = Vec::new();
            for (range, hl) in stream.highlight_line(line) {
                let mut span = Span::new(line[range].to_string());
                if let Some(c) = hl.color() {
                    span.color = Some(c);
                }
                span.font = hl.font();
                spans.push(span);
            }
            if spans.is_empty() {
                spans.push(Span::new(" "));
            }
            stream.commit();
            rich_text(spans)
                .font(Font::MONOSPACE)
                .size(code_size)
                .into()
        })
        .collect();

    let code_column = column(line_elements).width(Length::Shrink);
    let scrolled = scrollable(container(code_column).padding(Padding::new(8.0)))
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default().width(6).scroller_width(6),
        ))
        .width(Length::Fill);
    container(scrolled)
        .width(Length::Fill)
        .padding(6.0)
        .style(|_theme| widget::container::Style {
            background: Some(iced::Background::Color(CODE_BG)),
            border: iced::Border {
                radius: 6.0.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
}

/// 块级公式:居中;超宽横向滚动;解析失败回退红色源文。
fn math_block_view<'a, M: 'a>(gfx: MathGfx) -> Element<'a, M> {
    if let Some(err) = &gfx.error {
        return container(
            text(err.to_string())
                .font(Font::MONOSPACE)
                .color(ERROR_COLOR),
        )
        .center_x(Length::Fill)
        .padding(8)
        .into();
    }
    let svg = svg_widget::Svg::new(gfx.handle)
        .width(Length::Shrink)
        .height(Length::Shrink);
    let inner = scrollable(svg)
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::default().width(6).scroller_width(6),
        ))
        .width(Length::Fill);
    container(inner).center_x(Length::Fill).padding(8).into()
}
