//! 命中区域与绘制一致的裁剪容器。
//!
//! iced 的 `container(...).clip(true)` 只裁剪**绘制**:它把裁剪后的
//! viewport 交给子节点的 `draw`,而 `update` / `mouse_interaction`
//! 仍然原样转发父级 viewport。被裁掉的正文(典型场景:折叠的问题
//! 卡片只显示 140px)因此仍保留自己的布局边界,会伸到卡片之外
//! (例如回答标签条上方),从而:
//!
//! - 抢走光标形态(`Interaction::Text` 的序数高于 `Pointer`,容器按
//!   最大值聚合),标签条显示成文本插入光标;
//! - 吃掉落在该区域上的点击(在不可见的正文里开始文本选择)。
//!
//! 本控件让裁剪矩形同时约束**绘制**与**命中**:光标不在裁剪矩形内
//! 时,子节点不参与命中判定,但仍接收释放等收尾事件;绘制仍然由内部的
//! `container(...).clip(true)` 完成,两者边界一致。

use iced::advanced::widget::tree;
use iced::advanced::{Clipboard, Layout, Shell, Widget};
use iced::{mouse, Element, Event, Length, Rectangle, Size};

/// 把裁剪矩形用于命中判定的容器包装。
pub struct ClipHit<'a, Message> {
    inner: Element<'a, Message>,
}

impl<'a, Message> ClipHit<'a, Message> {
    /// 包装一个已经设置好裁剪的容器(通常是
    /// `container(...).clip(true)`)。
    pub fn new(inner: impl Into<Element<'a, Message>>) -> Self {
        Self {
            inner: inner.into(),
        }
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for ClipHit<'a, Message> {
    fn tag(&self) -> tree::Tag {
        self.inner.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.inner.as_widget().state()
    }

    fn children(&self) -> Vec<tree::Tree> {
        self.inner.as_widget().children()
    }

    fn diff(&self, tree: &mut tree::Tree) {
        self.inner.as_widget().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut tree::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) -> iced::advanced::layout::Node {
        self.inner.as_widget_mut().layout(tree, renderer, limits)
    }

    fn update(
        &mut self,
        tree: &mut tree::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let clipped = layout.bounds().intersection(viewport).unwrap_or_default();
        let cursor = if cursor.is_over(clipped) {
            cursor
        } else {
            mouse::Cursor::Unavailable
        };
        // Release must reach a selection that began inside, even after the
        // pointer leaves the clip. An unavailable cursor prevents new hits.
        self.inner.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, &clipped,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &tree::Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(clipped) = layout.bounds().intersection(viewport) else {
            return mouse::Interaction::None;
        };
        if !cursor.is_over(clipped) {
            return mouse::Interaction::None;
        }
        self.inner
            .as_widget()
            .mouse_interaction(tree, layout, cursor, &clipped, renderer)
    }

    fn draw(
        &self,
        tree: &tree::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.inner
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport)
    }

    fn operate(
        &mut self,
        tree: &mut tree::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.inner
            .as_widget_mut()
            .operate(tree, layout, renderer, operation)
    }
}

impl<'a, Message: 'a> From<ClipHit<'a, Message>>
    for Element<'a, Message, iced::Theme, iced::Renderer>
{
    fn from(value: ClipHit<'a, Message>) -> Self {
        Element::new(value)
    }
}
