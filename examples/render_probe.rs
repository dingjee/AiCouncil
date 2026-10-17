//! P0 渲染验收窗口:真实原生窗口展示 Markdown/公式/中英混排/流式追加,
//! 不连接任何 API(PLAN P0.2-P0.5)。
//!
//! 运行:`cargo run --example render_probe`
//!
//! 覆盖:
//! - `tests/fixtures/render_cases.md` 的全部用例(内嵌,不依赖 cwd)
//! - 20,000 字历史 + 50 个公式的滚动压力样例
//! - 流式追加模拟(30ms 一段),验证输入区不冻结、未闭合公式回退

use std::sync::Arc;
use std::time::Duration;

use aicouncil::render::{self, math::MathCache, view, RenderStyle, RenderedMessage};
use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Element, Length, Subscription, Task};

const FIXTURE: &str = include_str!("../tests/fixtures/render_cases.md");

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aicouncil=info".into()),
        )
        .init();

    iced::application(Probe::new, Probe::update, Probe::view)
        .title(|_: &Probe| "AiCouncil 渲染验收 (render_probe)".to_string())
        .window_size((1100.0, 800.0))
        .theme(|_: &Probe| iced::Theme::Light)
        .subscription(Probe::subscription)
        .run()
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    ResetStream,
    InputChanged(String),
    LinkClicked(String),
}

struct Probe {
    fixture_msg: RenderedMessage,
    stress_msg: RenderedMessage,
    stream_msg: RenderedMessage,
    stream_pos: usize,
    input: String,
    math_cache: MathCache,
    style: RenderStyle,
}

fn section<'a, M: 'a>(title: &str, body: Element<'a, M>) -> Element<'a, M> {
    column![
        text(title.to_string()).size(13).color([0.4, 0.4, 0.45]),
        body
    ]
    .spacing(8)
    .into()
}

/// 流式模拟源文:包含未闭合公式 → 闭合 → 完整块公式 的推进过程。
const STREAM_SOURCE: &str = r#"## 流式输出模拟

正在生成的回答,包含行内公式 $E = mc^2$ 与块公式:

$$
\int_0^1 x^2 \, dx = \frac{1}{3}
$$

以及矩阵:

$$
\begin{pmatrix} 1 & 2 \\ 3 & 4 \end{pmatrix}
$$

中文与 English 混排的很长的一段文字,用于观察流式追加时输入框是否保持响应,行内公式 $\sum_{i=1}^n i$ 应当作为一个整体参与断行,不被从中间截断。最后是未闭合公式 $x^2 + "#;

fn stream_chunks() -> &'static [String] {
    static CHUNKS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    CHUNKS.get_or_init(|| {
        STREAM_SOURCE
            .chars()
            .collect::<Vec<_>>()
            .chunks(24)
            .map(|c| c.iter().collect())
            .collect()
    })
}

impl Probe {
    fn new() -> (Self, Task<Message>) {
        let style = RenderStyle::light();
        let mut cache = MathCache::new();
        let fixture_msg = render::render_message(FIXTURE, &style, &mut cache);
        let stress_msg = render::render_message(&stress_doc(), &style, &mut cache);
        let stream_msg = render::render_message("", &style, &mut cache);
        (
            Self {
                fixture_msg,
                stress_msg,
                stream_msg,
                stream_pos: 0,
                input: String::new(),
                math_cache: cache,
                style,
            },
            Task::none(),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let chunks = stream_chunks();
                if self.stream_pos < chunks.len() {
                    self.stream_pos += 2.min(chunks.len() - self.stream_pos);
                    let src: String = chunks[..self.stream_pos].concat();
                    let mut cache = std::mem::take(&mut self.math_cache);
                    self.stream_msg = render::render_message(&src, &self.style, &mut cache);
                    self.math_cache = cache;
                }
            }
            Message::ResetStream => {
                self.stream_pos = 0;
                let mut cache = std::mem::take(&mut self.math_cache);
                self.stream_msg = render::render_message("", &self.style, &mut cache);
                self.math_cache = cache;
            }
            Message::InputChanged(s) => self.input = s,
            Message::LinkClicked(url) => {
                tracing::info!(%url, "链接点击");
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let style = self.style;
        let on_link: view::OnLink<'_, Message> = Arc::new(|url: String| Message::LinkClicked(url));
        let fixture = view::view_message(&self.fixture_msg, style, on_link.clone());
        let stress = view::view_message(&self.stress_msg, style, on_link.clone());
        let stream = view::view_message(&self.stream_msg, style, on_link);

        let stream_indicator = if self.stream_pos >= stream_chunks().len() {
            "(流式完成)".to_string()
        } else {
            format!("(流式 {}/{})", self.stream_pos, stream_chunks().len())
        };

        let body = column![
            row![
                text("AiCouncil 渲染验收 · P0").size(18),
                button(text("重放流式").size(13)).on_press(Message::ResetStream),
            ]
            .spacing(16)
            .align_y(iced::Alignment::Center),
            section("1. 验收样例 fixtures/render_cases.md", fixture),
            section("2. 压力样例:20,000 字 + 50 公式", stress),
            section(&format!("3. 流式追加模拟 {stream_indicator}"), stream),
        ]
        .spacing(24)
        .max_width(860);

        let content =
            scrollable(container(body).padding(24).width(Length::Fill)).height(Length::Fill);

        // 输入区:流式期间保持可交互(P0 出口条件)
        let input_bar = row![
            text_input("输入中文验证输入法与响应…", &self.input)
                .on_input(Message::InputChanged)
                .size(15),
            button(text("发送").size(13)),
        ]
        .spacing(8);

        column![
            container(content).height(Length::Fill),
            container(input_bar).padding(12)
        ]
        .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_millis(30)).map(|_| Message::Tick)
    }
}

/// 生成 20,000+ 字、50 个公式的压力样例。
fn stress_doc() -> String {
    let mut doc = String::with_capacity(60_000);
    doc.push_str("# 压力样例\n\n");
    let formulas = [
        "E = mc^2",
        "\\frac{\\partial u}{\\partial t} = \\alpha \\nabla^2 u",
        "\\int_{-\\infty}^{\\infty} e^{-x^2} dx = \\sqrt{\\pi}",
        "\\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}",
        "\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}",
        "\\nabla \\times \\vec{B} = \\mu_0 \\vec{J}",
        "\\lim_{x \\to 0} \\frac{\\sin x}{x} = 1",
        "\\sqrt{x^2 + y^2 + z^2}",
    ];
    let paragraph = "这是一段用于压力测试的中文文本,包含常见的词汇与标点,用来验证长历史的滚动性能与文字渲染的正确性。混入一些 English words 以及数字 12345 与标点符号,观察字体回退是否正常。";
    let mut formula_count = 0;
    while doc.chars().count() < 20_000 {
        for _ in 0..3 {
            doc.push_str(paragraph);
            doc.push('\n');
            doc.push_str(paragraph);
            doc.push_str("\n\n");
        }
        if formula_count < 50 {
            let f = formulas[formula_count % formulas.len()];
            if formula_count % 2 == 0 {
                doc.push_str(&format!("行内公式 ${f}$ 出现在正文中。\n\n"));
            } else {
                doc.push_str(&format!("$$\n{f}\n$$\n\n"));
            }
            formula_count += 1;
        }
    }
    doc
}
