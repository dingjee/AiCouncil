//! 会话与消息树实体。

use chrono::{DateTime, Utc};

use super::ids::{ConversationId, GroupId, MessageId};

/// 会话分组(单层)。
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub sort_order: i64,
}

/// 会话。`group_id` 为空即“未分组”。
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub id: ConversationId,
    pub title: String,
    pub group_id: Option<GroupId>,
    pub system_prompt: String,
    /// 当前主分支叶子(下一轮默认继续位置)。
    pub main_leaf_id: Option<MessageId>,
    /// 输入框草稿。
    pub draft: String,
    /// 输入区待发送附件草稿(P7B)。
    pub draft_attachments: Vec<super::attachment::Attachment>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 标题来源(自动标题服务,PLAN 7.1)。
    pub title_source: TitleSource,
    /// 标题状态。
    pub title_status: TitleStatus,
    /// 标题版本(手动/自动写入的乐观锁)。
    pub title_revision: i64,
    /// 已尝试的标题请求次数(退避与上限判断)。
    pub title_attempt_count: i64,
}

/// 标题来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleSource {
    /// 空(尚无标题)。
    #[default]
    Empty,
    /// 首问 fallback。
    Fallback,
    /// 自动生成成功。
    Generated,
    /// 手动命名(始终优先)。
    Manual,
}

impl TitleSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Fallback => "fallback",
            Self::Generated => "generated",
            Self::Manual => "manual",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "empty" => Self::Empty,
            "fallback" => Self::Fallback,
            "generated" => Self::Generated,
            "manual" => Self::Manual,
            _ => return None,
        })
    }

    /// 自动标题允许覆盖的来源。
    pub fn allows_auto(&self) -> bool {
        matches!(self, Self::Empty | Self::Fallback)
    }
}

/// 标题状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TitleStatus {
    /// 需要生成(等下一次提问)。
    #[default]
    NeedsGeneration,
    /// 在途(重启后恢复为 needs_generation)。
    InFlight,
    /// 已定(手动或生成成功)。
    Ready,
}

impl TitleStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NeedsGeneration => "needs_generation",
            Self::InFlight => "in_flight",
            Self::Ready => "ready",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "needs_generation" => Self::NeedsGeneration,
            "in_flight" => Self::InFlight,
            "ready" => Self::Ready,
            _ => return None,
        })
    }
}

impl Conversation {
    pub fn new(id: ConversationId, now: DateTime<Utc>) -> Self {
        Self {
            id,
            title: String::new(),
            group_id: None,
            system_prompt: String::new(),
            main_leaf_id: None,
            draft: String::new(),
            draft_attachments: Vec::new(),
            created_at: now,
            updated_at: now,
            title_source: TitleSource::Empty,
            title_status: TitleStatus::NeedsGeneration,
            title_revision: 0,
            title_attempt_count: 0,
        }
    }
}

/// 消息节点(树)。父子必须在同一会话,禁止环(由 [`MessageTree`] 校验)。
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub parent_id: Option<MessageId>,
    pub role: super::Role,
    /// 原始 content(assistant 为累积的 Markdown;中断时保留部分文本)。
    pub content: String,
    /// user 消息的纯文本附件(P7B;发送时拼接进 prompt)。
    pub attachments: Vec<super::attachment::Attachment>,
    pub created_at: DateTime<Utc>,
}

/// 内存中的会话消息树,提供祖先链与环检测。
#[derive(Debug, Clone, Default)]
pub struct MessageTree {
    pub messages: std::collections::HashMap<MessageId, Message>,
}

impl MessageTree {
    pub fn new() -> Self {
        Self::default()
    }

    /// 插入节点;校验父存在、同会话、无环。
    pub fn insert(&mut self, msg: Message) -> Result<(), super::DomainError> {
        if let Some(parent_id) = msg.parent_id {
            let parent = self
                .messages
                .get(&parent_id)
                .ok_or(super::DomainError::MessageNotFound(parent_id))?;
            if parent.conversation_id != msg.conversation_id {
                return Err(super::DomainError::CrossConversationParent);
            }
            // 沿父链找环(同时确认能到根)
            let mut cur = Some(parent_id);
            let mut steps = 0usize;
            while let Some(id) = cur {
                if id == msg.id {
                    return Err(super::DomainError::Cycle(msg.id));
                }
                steps += 1;
                if steps > self.messages.len() + 1 {
                    return Err(super::DomainError::Cycle(msg.id));
                }
                cur = self.messages.get(&id).and_then(|m| m.parent_id);
            }
        }
        self.messages.insert(msg.id, msg);
        Ok(())
    }

    /// 从叶子沿 parent 回溯到根,返回根→叶子顺序。
    pub fn path_from_root(&self, leaf: MessageId) -> Result<Vec<MessageId>, super::DomainError> {
        let mut chain = Vec::new();
        let mut cur = Some(leaf);
        let mut steps = 0usize;
        while let Some(id) = cur {
            let msg = self
                .messages
                .get(&id)
                .ok_or(super::DomainError::MessageNotFound(id))?;
            chain.push(id);
            steps += 1;
            if steps > self.messages.len() + 1 {
                return Err(super::DomainError::Cycle(id));
            }
            cur = msg.parent_id;
        }
        chain.reverse();
        Ok(chain)
    }

    /// 指定节点下的直接子节点。
    pub fn children_of(&self, id: Option<MessageId>) -> Vec<MessageId> {
        let mut out: Vec<MessageId> = self
            .messages
            .iter()
            .filter(|(_, m)| m.parent_id == id)
            .map(|(k, _)| *k)
            .collect();
        out.sort_by_key(|k| {
            self.messages
                .get(k)
                .map(|m| m.created_at)
                .unwrap_or_else(Utc::now)
        });
        out
    }

    /// 所有叶子(无子节点的消息)。
    pub fn leaves(&self) -> Vec<MessageId> {
        let has_child: std::collections::HashSet<MessageId> =
            self.messages.values().filter_map(|m| m.parent_id).collect();
        let mut out: Vec<MessageId> = self
            .messages
            .keys()
            .filter(|id| !has_child.contains(id))
            .copied()
            .collect();
        out.sort_by_key(|id| {
            self.messages
                .get(id)
                .map(|m| m.created_at)
                .unwrap_or_else(Utc::now)
        });
        out
    }

    pub fn get(&self, id: MessageId) -> Option<&Message> {
        self.messages.get(&id)
    }

    /// 从无序消息集合构建(父先于子插入;同批时间戳并列时可靠)。
    /// 忽略父缺失的孤儿消息并报告其数量。
    pub fn from_messages(msgs: Vec<Message>) -> (Self, usize) {
        let mut tree = Self::new();
        let mut pending = msgs;
        // 拓扑插入:每轮插入父已就绪的消息
        loop {
            let before = pending.len();
            pending.retain(|m| {
                let parent_ready = m
                    .parent_id
                    .map(|p| tree.messages.contains_key(&p))
                    .unwrap_or(true);
                if parent_ready && tree.insert(m.clone()).is_ok() {
                    return false; // 已插入,从 pending 移除
                }
                true
            });
            if pending.len() == before {
                break;
            }
        }
        let orphans = pending.len();
        (tree, orphans)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Role;

    fn msg(id: MessageId, parent: Option<MessageId>, role: Role) -> Message {
        Message {
            id,
            conversation_id: ConversationId::new(),
            parent_id: parent,
            role,
            content: String::new(),
            attachments: Vec::new(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn path_order_and_cycle_detection() {
        let mut tree = MessageTree::new();
        let conv = ConversationId::new();
        let root = MessageId::new();
        let a = MessageId::new();
        let a2 = MessageId::new();
        let b = MessageId::new();

        let mut m = msg(root, None, Role::User);
        m.conversation_id = conv;
        tree.insert(m).unwrap();
        for (id, parent) in [(a, root), (a2, a), (b, root)] {
            let mut m = msg(id, Some(parent), Role::Assistant);
            m.conversation_id = conv;
            tree.insert(m).unwrap();
        }

        assert_eq!(tree.path_from_root(a2).unwrap(), vec![root, a, a2]);
        assert_eq!(tree.path_from_root(b).unwrap(), vec![root, b]);

        // 环:伪造 a2 -> a2(通过先移除再插回带环父链的方式在 API 上不可达,
        // 这里直接验证防御:插入自身为父)
        let mut bad = msg(MessageId::new(), None, Role::User);
        bad.conversation_id = conv;
        tree.insert(bad.clone()).unwrap();
        let id = bad.id;
        bad.parent_id = Some(id);
        // HashMap 覆盖插入绕过校验,验证 path 的步数防御
        tree.messages.insert(id, bad);
        assert!(matches!(
            tree.path_from_root(id),
            Err(super::super::DomainError::Cycle(_))
        ));
    }

    #[test]
    fn cross_conversation_parent_rejected() {
        let mut tree = MessageTree::new();
        let m = msg(MessageId::new(), None, Role::User);
        tree.insert(m).unwrap();
        let mut child = msg(MessageId::new(), Some(MessageId::new()), Role::Assistant);
        child.conversation_id = ConversationId::new();
        assert!(tree.insert(child).is_err());
    }

    #[test]
    fn leaves_and_children() {
        let mut tree = MessageTree::new();
        let conv = ConversationId::new();
        let root = MessageId::new();
        let a = MessageId::new();
        let b = MessageId::new();
        for (id, parent) in [(root, None), (a, Some(root)), (b, Some(root))] {
            let mut m = msg(id, parent, Role::User);
            m.conversation_id = conv;
            tree.insert(m).unwrap();
        }
        let mut leaves = tree.leaves();
        leaves.sort();
        let mut expect = vec![a, b];
        expect.sort();
        assert_eq!(leaves, expect);
        assert_eq!(tree.children_of(Some(root)).len(), 2);
        assert_eq!(tree.children_of(None), vec![root]);
    }
}
