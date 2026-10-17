//! 渲染层:Markdown + LaTeX 公式的解析与原生排版。
//!
//! 结构:
//! - [`md`]:pulldown-cmark 事件驱动的解析,产出块级 IR(含公式提取)
//! - [`math`]:LaTeX -> SVG 的渲染与缓存(iced_math)
//! - [`view`]:IR -> iced Element 视图
//! - [`flow`]:文本 + 行内公式的流式段落自定义组件(断行与基线对齐)
//!
//! 约束:渲染层不做网络/存储操作;耗时工作(解析、SVG)一律缓存,
//! 缓存键含公式源文/字号/颜色/显示模式(PLAN P0.5)。

pub mod flow;
pub mod math;
pub mod md;
pub mod view;

use std::sync::Arc;

/// 渲染参数(缓存键的一部分)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderStyle {
    /// 正文字号(逻辑像素)。
    pub text_size: f32,
    /// 行内公式字号。
    pub inline_math_size: f32,
    /// 块公式字号。
    pub block_math_size: f32,
    /// 文本颜色(影响 SVG 公式缓存键)。
    pub color: iced::Color,
}

impl RenderStyle {
    /// 明亮主题默认参数。
    pub fn light() -> Self {
        Self {
            text_size: 15.0,
            inline_math_size: 15.0,
            block_math_size: 17.0,
            color: iced::Color::from_rgb(0.13, 0.13, 0.14),
        }
    }
}

/// 一条消息的完整渲染产物(共享 + 缓存)。
pub struct RenderedMessage {
    pub doc: Arc<md::Document>,
    pub flow: Arc<flow::FlowContent>,
}

/// 解析一条消息并生成渲染产物。
///
/// 公式 SVG 在此渲染并进入 `math::MathCache`(跨消息去重);
/// 视图层只读取产物,不再做耗时工作。
pub fn render_message(
    markdown: &str,
    style: &RenderStyle,
    cache: &mut math::MathCache,
) -> RenderedMessage {
    let doc = md::parse(markdown);
    let flow = flow::FlowContent::from_document(&doc, style, cache);
    RenderedMessage {
        doc: Arc::new(doc),
        flow: Arc::new(flow),
    }
}
