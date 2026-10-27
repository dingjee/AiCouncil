//! 并行回答组:回答集合与视图状态(纯逻辑,可单测)。
//!
//! - 「回答集合」= 同一问题下的 assistant 子节点;顺序确定(创建时间 + id),
//!   不再依赖 HashMap 迭代序(旧实现每次启动顺序都可能不同);
//! - 同一重试 lineage 的多次尝试折叠为最新一次,旧尝试计数可见;
//! - 视图与导出共用该定义,避免“导出 4 路、界面 3 路”的口径分歧。

use std::collections::HashMap;

use crate::domain::{Generation, GenerationId, GenerationStatus, MessageId, MessageTree, Role};

/// 回答组呈现模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LayoutMode {
    /// 单栏:只显示当前回答。
    #[default]
    Solo,
    /// 双栏并排:同时显示两个回答,便于对比。
    Split,
}

/// 一个回答(标签条的一项)。
#[derive(Debug, Clone, PartialEq)]
pub struct AnswerItem {
    /// assistant 消息 ID。
    pub message_id: MessageId,
    /// 模型 ID(原始;展示名由调用方归一化)。
    pub model_id: String,
    /// 第几次尝试(1 = 首次)。
    pub attempt: usize,
    /// 同一 lineage 的尝试总数。
    pub attempts_total: usize,
    /// 生成状态(无 generation 记录时为 None)。
    pub status: Option<GenerationStatus>,
    /// 是否被更新的尝试取代(默认折叠)。
    pub superseded: bool,
}

impl AnswerItem {
    /// 是否仍在生成(标签上的状态点)。
    pub fn is_live(&self) -> bool {
        matches!(
            self.status,
            Some(
                GenerationStatus::Queued
                    | GenerationStatus::Connecting
                    | GenerationStatus::Streaming
            )
        )
    }
}

/// 回答组视图状态(按问题)。
#[derive(Debug, Clone, Default)]
pub struct AnswerView {
    /// 呈现模式。
    pub mode: LayoutMode,
    /// 活动回答(1..=2;顺序即显示顺序)。
    pub active: Vec<MessageId>,
}

impl AnswerView {
    /// 点击标签:单栏时独占显示;双栏时替换第二栏(第一栏保持)。
    pub fn select(&mut self, id: MessageId) {
        match self.mode {
            LayoutMode::Solo => self.active = vec![id],
            LayoutMode::Split => {
                if self.active.len() < 2 {
                    self.active = vec![id];
                    return;
                }
                if self.active[0] == id {
                    // 点第一栏的标签 → 交换两栏(便于换出想要的组合)
                    self.active.swap(0, 1);
                } else {
                    self.active[1] = id;
                }
            }
        }
    }

    /// 对比开关:把回答加入/移出对比对(上限 2)。
    ///
    /// - 已在对比对中且对里有两项 → 移出(退回单栏);
    /// - 已在对比对中且只有它 → 保持(不能空集);
    /// - 不在对比对中 → 加入(已满两项时替换第二项)。
    pub fn toggle_compare(&mut self, id: MessageId) {
        if let Some(pos) = self.active.iter().position(|x| *x == id) {
            if self.active.len() > 1 {
                self.active.remove(pos);
                self.mode = LayoutMode::Solo;
            }
            return;
        }
        if self.active.len() < 2 {
            self.active.push(id);
        } else {
            self.active[1] = id;
        }
        self.mode = LayoutMode::Split;
    }

    /// 切换对比模式:进入时补齐第二个(取同组中未激活的下一个)。
    pub fn toggle_split(&mut self, all: &[MessageId]) {
        match self.mode {
            LayoutMode::Split => {
                self.mode = LayoutMode::Solo;
                self.active.truncate(1);
            }
            LayoutMode::Solo => {
                if self.active.len() < 2 {
                    if let Some(next) = all.iter().find(|id| !self.active.contains(id)) {
                        self.active.push(*next);
                    }
                }
                self.mode = LayoutMode::Split;
            }
        }
    }

    /// 当前应显示的回答(按模式取 1 或 2 个)。
    pub fn visible_answers(&self, all: &[MessageId]) -> Vec<MessageId> {
        let mut out: Vec<MessageId> = Vec::new();
        for id in &self.active {
            if all.contains(id) && !out.contains(id) {
                out.push(*id);
            }
        }
        if out.is_empty() {
            if let Some(first) = all.first() {
                out.push(*first);
            }
        }
        if self.mode == LayoutMode::Split && out.len() == 1 {
            if let Some(next) = all.iter().find(|id| !out.contains(id)) {
                out.push(*next);
            }
        }
        out
    }
}

/// 合并数据库快照与在流正文。
///
/// 落库按 300ms/4KB 节流,数据库快照会比流式落后一截;兄弟分支终态
/// 触发的整树重载若直接覆盖,其它在流分支的正文会回退成旧快照(肉眼
/// 即“卡片忽显忽隐”)。仍在流(`live`)的消息保留内存正文,终态消息
/// 以数据库为准。
pub fn merge_live_content(
    mut fresh: MessageTree,
    previous: Option<&MessageTree>,
    live: &std::collections::HashSet<MessageId>,
) -> MessageTree {
    let Some(previous) = previous else {
        return fresh;
    };
    for (id, message) in fresh.messages.iter_mut() {
        if !live.contains(id) {
            continue;
        }
        if let Some(prev) = previous.get(*id) {
            if prev.content.len() > message.content.len() {
                message.content = prev.content.clone();
            }
        }
    }
    fresh
}

/// 取该问题的回答集合(确定性顺序;重试尝试折叠标记)。
pub fn answer_set(
    tree: &MessageTree,
    generations: &[Generation],
    question_id: MessageId,
) -> Vec<AnswerItem> {
    let mut ids: Vec<MessageId> = tree
        .children_of(Some(question_id))
        .into_iter()
        .filter(|id| tree.get(*id).is_some_and(|m| m.role == Role::Assistant))
        .collect();
    // 确定性顺序:创建时间 → 消息 ID(并列时不再依赖 HashMap 迭代序)。
    ids.sort_by(|a, b| {
        let ta = tree.get(*a).map(|m| m.created_at);
        let tb = tree.get(*b).map(|m| m.created_at);
        ta.cmp(&tb).then_with(|| a.as_str().cmp(&b.as_str()))
    });

    let gen_of: HashMap<MessageId, Option<Generation>> = ids
        .iter()
        .map(|id| (*id, latest_generation(generations, *id)))
        .collect();

    let root_of = |id: &MessageId| gen_of.get(id).and_then(|g| g.as_ref()).map(lineage_root);

    let mut items = Vec::with_capacity(ids.len());
    for (idx, id) in ids.iter().enumerate() {
        let gen = gen_of.get(id).and_then(|g| g.clone());
        let root = root_of(id);
        let attempt = match root {
            Some(r) => {
                ids.iter()
                    .enumerate()
                    .filter(|(j, other)| *j < idx && root_of(other) == Some(r))
                    .count()
                    + 1
            }
            None => 1,
        };
        let attempts_total = match root {
            Some(r) => ids
                .iter()
                .filter(|other| root_of(other) == Some(r))
                .count()
                .max(1),
            None => 1,
        };
        let superseded = match root {
            Some(r) => ids
                .iter()
                .enumerate()
                .any(|(j, other)| j > idx && root_of(other) == Some(r)),
            None => false,
        };
        items.push(AnswerItem {
            message_id: *id,
            model_id: gen.as_ref().map(|g| g.model_id.clone()).unwrap_or_default(),
            attempt,
            attempts_total,
            status: gen.as_ref().map(|g| g.status),
            superseded,
        });
    }
    items
}

/// 默认可见的回答(折叠被取代的尝试)。
pub fn visible_items(items: &[AnswerItem]) -> Vec<&AnswerItem> {
    items.iter().filter(|i| !i.superseded).collect()
}

/// 被折叠的旧尝试数。
pub fn hidden_attempts(items: &[AnswerItem]) -> usize {
    items.iter().filter(|i| i.superseded).count()
}

/// 取某 assistant 消息的最新一次 generation。
fn latest_generation(generations: &[Generation], assistant: MessageId) -> Option<Generation> {
    generations
        .iter()
        .filter(|g| g.assistant_message_id == assistant)
        .cloned()
        .max_by_key(|g| g.started_at.unwrap_or_else(crate::storage::now_dt))
}

/// 重试 lineage 根(原请求的 root 为自己)。
fn lineage_root(g: &Generation) -> GenerationId {
    g.retry_root_id.unwrap_or(g.request_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{GenerationId, Message, Role};
    use chrono::{DateTime, Utc};

    fn tree_with(messages: Vec<crate::domain::Message>) -> MessageTree {
        // 同一会话(跨会话父子会被树拒绝)
        let conv = crate::domain::ConversationId::new();
        let messages: Vec<crate::domain::Message> = messages
            .into_iter()
            .map(|mut m| {
                m.conversation_id = conv;
                m
            })
            .collect();
        let (tree, orphans) = MessageTree::from_messages(messages);
        assert_eq!(orphans, 0, "测试消息树不应有孤儿");
        tree
    }

    fn msg(id: MessageId, parent: Option<MessageId>, role: Role, at: DateTime<Utc>) -> Message {
        Message {
            id,
            conversation_id: crate::domain::ConversationId::new(),
            parent_id: parent,
            role,
            content: String::new(),
            attachments: Vec::new(),
            created_at: at,
        }
    }

    fn gen(
        request: GenerationId,
        assistant: MessageId,
        user: MessageId,
        model: &str,
        root: Option<GenerationId>,
        at: DateTime<Utc>,
    ) -> Generation {
        Generation {
            request_id: request,
            batch_id: crate::domain::BatchId::new(),
            user_message_id: user,
            assistant_message_id: assistant,
            provider_id: crate::domain::ProviderId::from_uuid_str(
                "00000000-0000-4000-8000-0000000000a1",
            ),
            model_id: model.to_string(),
            status: GenerationStatus::Completed,
            params: None,
            system_prompt: None,
            context_snapshot: Vec::new(),
            price_snapshot: None,
            remote_generation_id: None,
            error: None,
            retry_of: None,
            retry_root_id: root,
            failure: None,
            request_snapshot: None,
            retry_not_before_ms: None,
            dispatch_phase: crate::domain::DispatchPhase::NotDispatched,
            started_at: Some(at),
            finished_at: Some(at),
        }
    }

    fn sample() -> (MessageTree, MessageId, Vec<Generation>) {
        let now = Utc::now();
        let user = MessageId::new();
        let a1 = MessageId::new();
        let a2 = MessageId::new();
        let a3 = MessageId::new();
        let tree = tree_with(vec![
            msg(user, None, Role::User, now),
            msg(a1, Some(user), Role::Assistant, now),
            msg(a2, Some(user), Role::Assistant, now),
            msg(a3, Some(user), Role::Assistant, now),
        ]);
        let gens = vec![
            gen(GenerationId::new(), a1, user, "vendor/alpha", None, now),
            gen(GenerationId::new(), a2, user, "vendor/beta", None, now),
            gen(GenerationId::new(), a3, user, "vendor/gamma", None, now),
        ];
        (tree, user, gens)
    }

    #[test]
    fn order_is_deterministic_even_with_equal_timestamps() {
        let (tree, user, gens) = sample();
        let ids: Vec<String> = answer_set(&tree, &gens, user)
            .iter()
            .map(|i| i.message_id.as_str())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "同创建时间按消息 ID 升序,不受 HashMap 顺序影响"
        );
    }

    #[test]
    fn retry_attempts_are_folded_and_counted() {
        let now = Utc::now();
        let user = MessageId::new();
        let first = MessageId::new();
        let retry = MessageId::new();
        let tree = tree_with(vec![
            msg(user, None, Role::User, now),
            msg(first, Some(user), Role::Assistant, now),
            msg(
                retry,
                Some(user),
                Role::Assistant,
                now + chrono::Duration::seconds(5),
            ),
        ]);
        let root = GenerationId::new();
        let gens = vec![
            gen(root, first, user, "vendor/alpha", Some(root), now),
            gen(
                GenerationId::new(),
                retry,
                user,
                "vendor/alpha",
                Some(root),
                now + chrono::Duration::seconds(5),
            ),
        ];
        let items = answer_set(&tree, &gens, user);
        assert_eq!(items.len(), 2);
        assert!(items[0].superseded, "旧尝试被折叠");
        assert_eq!(items[0].attempt, 1);
        assert_eq!(items[0].attempts_total, 2);
        assert!(!items[1].superseded, "最新尝试可见");
        assert_eq!(items[1].attempt, 2);
        assert_eq!(hidden_attempts(&items), 1);
        assert_eq!(visible_items(&items).len(), 1);
    }

    #[test]
    fn merge_keeps_live_content_of_streaming_messages() {
        let now = Utc::now();
        let user = MessageId::new();
        let live_msg = MessageId::new();
        let done_msg = MessageId::new();
        let mut old = tree_with(vec![
            msg(user, None, Role::User, now),
            msg(live_msg, Some(user), Role::Assistant, now),
            msg(done_msg, Some(user), Role::Assistant, now),
        ]);
        old.get_mut(live_msg).unwrap().content = "流式中的新正文".to_string();
        old.get_mut(done_msg).unwrap().content = "终态正文".to_string();

        // 数据库快照:落库节流,正文落后
        let mut fresh = tree_with(vec![
            msg(user, None, Role::User, now),
            msg(live_msg, Some(user), Role::Assistant, now),
            msg(done_msg, Some(user), Role::Assistant, now),
        ]);
        fresh.get_mut(live_msg).unwrap().content = "流式中的旧".to_string();
        fresh.get_mut(done_msg).unwrap().content = "终态旧".to_string();

        let mut live = std::collections::HashSet::new();
        live.insert(live_msg);
        let merged = merge_live_content(fresh, Some(&old), &live);

        assert_eq!(
            merged.get(live_msg).unwrap().content,
            "流式中的新正文",
            "在流消息不得被旧快照回退"
        );
        assert_eq!(
            merged.get(done_msg).unwrap().content,
            "终态旧",
            "终态消息以数据库为准"
        );
    }

    #[test]
    fn solo_and_split_selection_rules() {
        let mut view = AnswerView::default();
        let a = MessageId::new();
        let b = MessageId::new();
        let all = vec![a, b];

        view.select(a);
        assert_eq!(view.mode, LayoutMode::Solo);
        assert_eq!(view.visible_answers(&all), vec![a]);

        // 对比开关:加入第二项 → 双栏
        view.toggle_compare(b);
        assert_eq!(view.mode, LayoutMode::Split);
        assert_eq!(view.visible_answers(&all), vec![a, b]);

        // 已满 2 时再加入第三项 → 替换第二项
        let c = MessageId::new();
        view.toggle_compare(c);
        assert_eq!(view.active, vec![a, c]);

        // 移出 → 退回单栏
        view.toggle_compare(c);
        assert_eq!(view.mode, LayoutMode::Solo);
        assert_eq!(view.active, vec![a]);

        // 切到对比模式 → 自动补第二个
        view.toggle_split(&all);
        assert_eq!(view.mode, LayoutMode::Split);
        assert_eq!(view.visible_answers(&all), vec![a, b]);
        view.toggle_split(&all);
        assert_eq!(view.mode, LayoutMode::Solo);
    }

    #[test]
    fn selecting_a_tab_in_split_mode_replaces_second_pane() {
        let mut view = AnswerView::default();
        let a = MessageId::new();
        let b = MessageId::new();
        let c = MessageId::new();
        let all = vec![a, b, c];

        view.select(a);
        view.toggle_split(&all);
        assert_eq!(view.active, vec![a, b]);

        // 双栏下点标签 → 替换第二栏,第一栏不动
        view.select(c);
        assert_eq!(view.active, vec![a, c]);
        assert_eq!(view.mode, LayoutMode::Split);

        // 点第一栏的标签 → 两栏交换(便于换出想要的组合)
        view.select(a);
        assert_eq!(view.active, vec![c, a]);
    }
}
