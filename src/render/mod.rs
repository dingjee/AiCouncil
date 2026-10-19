//! 渲染层:Markdown + LaTeX 公式的解析、原生排版与**可选择文档**。
//!
//! 结构(PLAN 4.1):
//! - [`md`]:pulldown-cmark 事件驱动的解析,产出块级 IR(含公式提取,
//!   Strict / LlmCompatible 双策略)
//! - [`math`]:LaTeX -> SVG 的渲染与缓存(iced_math)
//! - [`flow`]:所有行内内容的统一排版/选择组件(文本 + 行内公式)
//! - [`document`]:稳定块路径 + PlainCopyProjection(复制投影)
//! - [`view`]:IR -> iced Element 视图(表格/代码/块公式/组装)
//!
//! 约束:渲染层不做网络/存储操作;耗时工作(解析、SVG)一律缓存,
//! 缓存键含公式源文/字号/颜色/显示模式;原始 Markdown 永不回写。

pub mod code_flow;
pub mod document;
pub mod flow;
pub mod math;
pub mod md;
pub mod view;

use std::sync::Arc;

/// 渲染参数(缓存键的一部分;来自 design.rs 的 Soft Sage 令牌)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderStyle {
    /// 正文字号(逻辑像素;设置可调 12–22)。
    pub body: f32,
    /// 正文行高倍率(设计 1.55)。
    pub line_height: f32,
    /// 块公式字号倍率(相对正文)。
    pub block_math_ratio: f32,
    /// 正文颜色(影响 SVG 公式缓存键)。
    pub color: iced::Color,
    /// 链接颜色。
    pub link: iced::Color,
    /// 行内代码底色。
    pub code_bg: iced::Color,
    /// 错误/公式回退色。
    pub error: iced::Color,
    /// 选区高亮色。
    pub selection: iced::Color,
    /// 字号/主题修订:变化时清空字号相关的组件内测量缓存。
    pub revision: u64,
}

impl RenderStyle {
    /// Soft Sage 默认(3.1.1):暖灰纸面 + 米白正文 + 低饱和灰绿强调。
    pub fn soft_sage(body: f32) -> Self {
        use crate::ui_style::palette as p;
        Self {
            body,
            line_height: crate::ui_style::BODY_LINE_HEIGHT,
            block_math_ratio: 1.13,
            color: p::TEXT,
            link: p::LINK,
            code_bg: p::CODE_SURFACE,
            error: p::ERROR,
            selection: p::SELECTION,
            revision: body.to_bits() as u64 ^ 0x5A5A_5A5A_5A5A_5A5A,
        }
    }

    /// 兼容旧调用点(15px 正文)。
    pub fn light() -> Self {
        Self::soft_sage(15.0)
    }

    pub fn inline_math_size(&self) -> f32 {
        self.body
    }

    pub fn block_math_size(&self) -> f32 {
        self.body * self.block_math_ratio
    }
}

/// 一条消息的完整渲染产物(共享 + 缓存)。
pub struct RenderedMessage {
    pub doc: Arc<md::Document>,
    pub flow: Arc<flow::FlowContent>,
}

/// 解析一条消息并生成渲染产物。
///
/// `policy`:用户消息用 Strict,assistant 消息用 LlmCompatible(PLAN 4.3)。
/// 公式 SVG 在此渲染并进入 `math::MathCache`(跨消息去重);
/// 视图层只读取产物,不再做耗时工作。
pub fn render_message(
    markdown: &str,
    style: &RenderStyle,
    policy: md::ParsePolicy,
    cache: &mut math::MathCache,
) -> RenderedMessage {
    let doc = md::parse(markdown, policy);
    let flow = flow::FlowContent::from_document(&doc, style, cache);
    RenderedMessage {
        doc: Arc::new(doc),
        flow: Arc::new(flow),
    }
}
