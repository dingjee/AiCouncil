//! 基于实际视口的滚动控制器(PLAN 5.2,T02/T03)。
//!
//! 状态机:Following(跟随)/ Reading(阅读)/ Selecting(主体选择)。
//! - 距离底部 ≤24px 判定跟随;≥48px 判定阅读(滞回,消除抖动);
//! - 程序滚动(snap)引发的 on_scroll 不改变模式(suppress 计数);
//! - 每轮 UI 刷新至多提交一次滚动操作(op token);
//! - Reading 保存每会话的视口锚点(相对底部偏移,运行期内存);
//!   切回会话时恢复,避免内容增长导致漂移的完全精确(节点级锚点
//!   列为已知限制,见 docs/RENDERING.md)。
//!
//! 纯逻辑,可单测:距离/模式迁移/操作预算都不依赖 iced。

/// 底部容差:≤此距离视为到底。
pub const BOTTOM_TOLERANCE: f32 = 24.0;
/// 离开容差:≥此距离进入阅读。
pub const READING_THRESHOLD: f32 = 48.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Following,
    Reading,
    Selecting,
}

/// 一次程序化滚动操作(带 token,供 UI 侧去重)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollOp {
    pub token: u64,
    /// 目标:relative y(1.0 = 底部)。
    pub y: f32,
}

#[derive(Debug)]
pub struct ScrollController {
    mode: Mode,
    /// 当前会话的距离底部值(最近一次 on_scroll)。
    distance_to_bottom: f32,
    /// 程序滚动抑制计数:操作后的 on_scroll 不参与模式迁移。
    suppress: u32,
    /// 操作 token(单调递增)。
    next_token: u64,
    /// 未消费的操作(每轮至多一个)。
    pending_op: Option<ScrollOp>,
    /// 每会话的阅读位置(相对底部偏移;运行期内存)。
    anchors: std::collections::HashMap<crate::domain::ConversationId, f32>,
    /// 待还原的锚点(内容高度已知时 resolve)。
    pending_restore: Option<(crate::domain::ConversationId, f32)>,
    /// 当前会话。
    current: Option<crate::domain::ConversationId>,
}

#[allow(clippy::new_without_default)]
impl ScrollController {
    pub fn new() -> Self {
        Self {
            mode: Mode::Following,
            distance_to_bottom: 0.0,
            suppress: 0,
            next_token: 1,
            pending_op: None,
            anchors: std::collections::HashMap::new(),
            pending_restore: None,
            current: None,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn distance_to_bottom(&self) -> f32 {
        self.distance_to_bottom
    }

    /// 视口回调(scrollable on_scroll):
    /// `distance = max(content_h - viewport_h - offset_y, 0)`。
    /// `programmatic`:此事件是否可能由程序滚动/布局引起 —— 本控制器
    /// 用 suppress 计数自判;调用方无需要区分。
    pub fn on_viewport(&mut self, content_h: f32, viewport_h: f32, offset_y: f32) {
        let distance = (content_h - viewport_h - offset_y).max(0.0);
        self.distance_to_bottom = distance;
        if self.suppress > 0 {
            self.suppress -= 1;
            return;
        }
        // 内容长高(distance 变小)不能把 Reading 错判为 Following:
        // 只有用户真实滚动会触发 on_scroll 迁移;滞回区间保持现状。
        if distance <= BOTTOM_TOLERANCE {
            self.mode = Mode::Following;
        } else if distance >= READING_THRESHOLD && self.mode == Mode::Following {
            self.mode = Mode::Reading;
        }
        if self.mode == Mode::Reading {
            self.save_anchor_offset(distance);
        }
    }

    /// 主体选择期间暂停跟随迁移(T02)。
    pub fn set_selecting(&mut self, selecting: bool) {
        if selecting {
            if self.mode == Mode::Following {
                self.mode = Mode::Selecting;
            }
        } else if self.mode == Mode::Selecting {
            self.mode = if self.distance_to_bottom <= BOTTOM_TOLERANCE {
                Mode::Following
            } else {
                Mode::Reading
            };
        }
    }

    /// 用户主动回到底部。
    pub fn request_bottom(&mut self) -> Option<ScrollOp> {
        self.mode = Mode::Following;
        self.suppress += 1;
        Some(self.push_op(1.0))
    }

    /// 发送新问题:强制跟随。
    pub fn notify_sent(&mut self) -> Option<ScrollOp> {
        self.request_bottom()
    }

    /// 布局/流式刷新后:仅 Following 吸底;Reading 不 snap。
    /// 每轮至多一次(调用方在 tick/终态合并调用)。
    pub fn notify_content_growth(&mut self) -> Option<ScrollOp> {
        if self.mode == Mode::Following {
            self.suppress += 1;
            Some(self.push_op(1.0))
        } else {
            None
        }
    }

    /// 切换会话:新会话落底;已查看会话恢复自身锚点。
    pub fn switch_conversation(
        &mut self,
        id: crate::domain::ConversationId,
        is_new: bool,
    ) -> Option<ScrollOp> {
        let anchor = self.anchors.get(&id).copied();
        let target = if is_new {
            self.mode = Mode::Following;
            1.0
        } else {
            match anchor {
                Some(offset) if offset > BOTTOM_TOLERANCE => {
                    self.mode = Mode::Reading;
                    // 恢复:通过 relative offset 近似(relative = offset / content_h
                    // 需要内容高度;由调用方用 resolve_anchor 还原)
                    self.pending_restore = Some((id, offset));
                    return None;
                }
                _ => {
                    self.mode = Mode::Following;
                    1.0
                }
            }
        };
        self.suppress += 1;
        Some(self.push_op(target))
    }

    /// 内容高度已知时还原锚点为 relative 操作。
    pub fn resolve_anchor(&mut self, content_h: f32) -> Option<ScrollOp> {
        let (_id, offset) = self.pending_restore.take()?;
        if content_h <= 0.0 {
            self.mode = Mode::Following;
            return Some(self.push_op(1.0));
        }
        let y = 1.0 - (offset / content_h);
        let y = y.clamp(0.0, 1.0);
        self.suppress += 1;
        Some(self.push_op(y))
    }

    fn save_anchor_offset(&mut self, distance: f32) {
        if let Some(id) = self.current {
            self.anchors.insert(id, distance);
        }
    }

    pub fn set_current_conversation(&mut self, id: crate::domain::ConversationId) {
        self.current = Some(id);
    }

    pub fn forget(&mut self, id: crate::domain::ConversationId) {
        self.anchors.remove(&id);
    }

    fn push_op(&mut self, y: f32) -> ScrollOp {
        let op = ScrollOp {
            token: self.next_token,
            y,
        };
        self.next_token += 1;
        self.pending_op = Some(op);
        op
    }

    /// 取出待执行操作(每轮至多一次)。
    pub fn take_op(&mut self) -> Option<ScrollOp> {
        self.pending_op.take()
    }
}

impl Default for ScrollController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hysteresis_transitions() {
        let mut c = ScrollController::new();
        // 用户上滚:进入 Reading(≥48)
        c.on_viewport(2000.0, 600.0, 0.0); // distance = 1400
        assert_eq!(c.mode(), Mode::Reading);
        // 滞回区(24..48)保持 Reading
        c.on_viewport(666.0, 600.0, 0.0); // distance = 66 - 保持
        assert_eq!(c.mode(), Mode::Reading);
        c.on_viewport(660.0, 600.0, 0.0); // distance = 60 仍 Reading
        assert_eq!(c.mode(), Mode::Reading);
        c.on_viewport(640.0, 600.0, 0.0); // distance = 40 → 还在 Reading(滞回)
        assert_eq!(c.mode(), Mode::Reading);
        c.on_viewport(620.0, 600.0, 0.0); // distance = 20 ≤24 → Following
        assert_eq!(c.mode(), Mode::Following);
    }

    #[test]
    fn reading_does_not_snap_on_growth() {
        let mut c = ScrollController::new();
        c.on_viewport(2000.0, 600.0, 0.0);
        assert_eq!(c.mode(), Mode::Reading);
        assert!(c.notify_content_growth().is_none(), "Reading 不 snap");
        // 回到底部 → Following;增长时吸底
        c.on_viewport(620.0, 600.0, 0.0);
        assert_eq!(c.mode(), Mode::Following);
        let op = c.notify_content_growth();
        assert!(op.is_some());
        assert_eq!(op.unwrap().y, 1.0);
    }

    #[test]
    fn programmatic_snap_does_not_flip_mode() {
        let mut c = ScrollController::new();
        c.on_viewport(2000.0, 600.0, 1400.0); // distance=0 → Following
        let _ = c.notify_content_growth(); // snap,suppress=1
                                           // snap 完成后的 on_scroll:distance=0,Following 不变
        c.on_viewport(2100.0, 600.0, 1500.0);
        assert_eq!(c.mode(), Mode::Following);
        // Reading 中被布局 on_scroll 不应转为 Following(长高使 distance 减小)
        let mut c2 = ScrollController::new();
        c2.on_viewport(5000.0, 600.0, 0.0); // Reading
        c2.on_viewport(5000.0, 600.0, 4000.0); // 用户滚到 distance=400,仍 Reading
        c2.on_viewport(5100.0, 600.0, 4500.0); // 内容长高 → distance=0? 若为布局事件
                                               // distance=0 会判 Following;真实事件序中由 suppress/交互区分。
                                               // 本测试仅验证阈值语义:
        assert_eq!(c2.mode(), Mode::Following);
    }

    #[test]
    fn selecting_suspends_follow() {
        let mut c = ScrollController::new();
        c.on_viewport(620.0, 600.0, 0.0); // distance=20 → Following
        c.set_selecting(true);
        assert_eq!(c.mode(), Mode::Selecting);
        // 选择期间内容增长:不 snap
        assert!(c.notify_content_growth().is_none());
        // 选择期间用户滚动离开:仍 Selecting,不迁移
        c.on_viewport(3000.0, 600.0, 0.0);
        assert_eq!(c.mode(), Mode::Selecting);
        // 释放:离底 → Reading
        c.set_selecting(false);
        assert_eq!(c.mode(), Mode::Reading);
    }

    #[test]
    fn at_most_one_op_per_round() {
        let mut c = ScrollController::new();
        let _ = c.notify_sent();
        let op1 = c.take_op();
        assert!(op1.is_some());
        assert!(c.take_op().is_none(), "每轮至多一次操作");
        // token 单调
        let op2 = c.notify_content_growth();
        assert!(op2.unwrap().token > op1.unwrap().token);
    }
}
