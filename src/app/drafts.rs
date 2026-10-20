//! Composer 草稿协调(单一写入点)。
//!
//! 约束(0.1.7 草稿持久化方案):
//! - 一个会话一个 composer 状态,身份始终绑定 [`ConversationId`];
//!   迟到回执按 (会话, 版本) 判定,不用“当前会话”猜测。
//! - 权威内容是 `conversations` 行(`draft` / `draft_attachments_json` 与
//!   独立的 `edit_draft*` 缓冲);本模块是这些列的唯一写入点,内存镜像
//!   与数据库行同步,切换会话时重新按权威行恢复。
//! - `draft_revision` 是内容版本(用户编辑计数),发送事务按它 CAS 消费
//!   草稿:提交后又输入了新内容则不清空,迟到回执不会抹掉新草稿。
//! - “编辑最新问题”使用独立缓冲:普通草稿与编辑缓冲互不覆盖。
//! - 未发送的新会话槽位(`ui_state.pending_new_conversation`)持久化,
//!   连点 New 回到同一槽位;首次发送落库成功后由发送事务释放。

use std::collections::HashMap;
use std::sync::Arc;

use crate::domain::{Attachment, Conversation, ConversationId, MessageId};
use crate::storage::{Db, DraftConsumption, DraftConsumptionRequest};

/// Composer 模式(当前会话的输入框含义)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerMode {
    /// 普通输入:内容属于会话的普通草稿。
    Normal,
    /// 编辑最新问题:内容属于编辑缓冲;普通草稿保持不动。
    Edit(MessageId),
}

/// 一个会话的 composer 状态(数据库行的镜像)。
#[derive(Debug, Clone, Default)]
pub struct DraftState {
    /// 普通 composer 正文。
    pub text: String,
    /// 普通 composer 附件。
    pub attachments: Vec<Attachment>,
    /// “编辑最新问题”缓冲正文。
    pub edit_text: String,
    /// “编辑最新问题”缓冲附件。
    pub edit_attachments: Vec<Attachment>,
    /// “编辑最新问题”针对的消息(为空即不在编辑模式)。
    pub edit_target: Option<MessageId>,
    /// 内容版本(用户编辑计数;发送事务的 CAS 令牌)。
    pub revision: i64,
}

impl DraftState {
    /// 草稿是否有有效内容(纯空白且无附件按空草稿处理)。
    pub fn has_content(&self) -> bool {
        has_content(&self.text, &self.attachments)
    }
}

/// 有效内容判定:trim 只用于判断,保存/恢复保留原文空白与换行。
pub fn has_content(text: &str, attachments: &[Attachment]) -> bool {
    !text.trim().is_empty() || !attachments.is_empty()
}

/// 发送时冻结的 composer 快照。
#[derive(Debug, Clone, Default)]
pub struct ComposerSnapshot {
    pub text: String,
    pub attachments: Vec<Attachment>,
    /// 冻结时的内容版本(消费草稿的 CAS 期望值)。
    pub revision: i64,
}

/// 草稿协调器:数据库列的唯一写入点 + 内存镜像。
pub struct DraftStore {
    db: Arc<Db>,
    /// 会话 → composer 状态(与数据库行同步)。
    states: HashMap<ConversationId, DraftState>,
    /// 未成功提交首条提问的新会话槽位。
    pending_new: Option<ConversationId>,
}

impl DraftStore {
    pub fn new(db: Arc<Db>) -> Self {
        Self {
            db,
            states: HashMap::new(),
            pending_new: None,
        }
    }

    // ----- 载入与镜像 -----

    /// 按权威行同步单个会话的 composer 状态。
    pub fn sync_from_row(&mut self, row: &Conversation) {
        self.states.insert(
            row.id,
            DraftState {
                text: row.draft.clone(),
                attachments: row.draft_attachments.clone(),
                edit_text: row.edit_draft.clone(),
                edit_attachments: row.edit_draft_attachments.clone(),
                edit_target: row.edit_target_id,
                revision: row.draft_revision,
            },
        );
    }

    /// 移除会话的镜像(删除会话时调用)。
    pub fn forget(&mut self, conv: ConversationId) {
        self.states.remove(&conv);
    }

    /// 启动:载入全部会话状态与新会话槽位指针。
    ///
    /// 指针校验:会话不存在或已发送过提问(不再是“未发送槽位”)时安全
    /// 清除,不修改其他会话内容。
    pub fn load_on_start(
        &mut self,
        conversations: &[Conversation],
        is_unsent: &dyn Fn(ConversationId) -> bool,
    ) {
        for row in conversations {
            self.sync_from_row(row);
        }
        let pointer = self.db.pending_new_conversation().unwrap_or(None);
        let valid = pointer
            .filter(|id| conversations.iter().any(|c| c.id == *id))
            .filter(|id| is_unsent(*id));
        match (pointer, valid) {
            (Some(_), None) => {
                // 悬空/已发送指针:安全修复
                let _ = self.db.clear_pending_new_conversation(None);
                self.pending_new = None;
            }
            (Some(_), Some(valid)) => self.pending_new = Some(valid),
            (None, _) => self.pending_new = None,
        }
    }

    // ----- 读取 -----

    pub fn state(&self, conv: ConversationId) -> Option<&DraftState> {
        self.states.get(&conv)
    }

    /// 内容版本(未知会话返回 0)。
    pub fn revision(&self, conv: ConversationId) -> i64 {
        self.states.get(&conv).map(|s| s.revision).unwrap_or(0)
    }

    /// 普通草稿正文。
    pub fn normal_text(&self, conv: ConversationId) -> &str {
        self.states
            .get(&conv)
            .map(|s| s.text.as_str())
            .unwrap_or("")
    }

    /// 普通草稿附件。
    pub fn normal_attachments(&self, conv: ConversationId) -> &[Attachment] {
        self.states
            .get(&conv)
            .map(|s| s.attachments.as_slice())
            .unwrap_or(&[])
    }

    /// 编辑缓冲(正文, 附件, 目标)。
    pub fn edit_buffer(&self, conv: ConversationId) -> Option<(&str, &[Attachment], MessageId)> {
        let state = self.states.get(&conv)?;
        let target = state.edit_target?;
        Some((
            state.edit_text.as_str(),
            state.edit_attachments.as_slice(),
            target,
        ))
    }

    /// 当前模式下的内容快照(发送冻结用)。
    pub fn snapshot(&self, conv: ConversationId, mode: ComposerMode) -> ComposerSnapshot {
        let state = self.states.get(&conv);
        let revision = state.map(|s| s.revision).unwrap_or(0);
        match (mode, state) {
            (ComposerMode::Edit(_), Some(state)) => ComposerSnapshot {
                text: state.edit_text.clone(),
                attachments: state.edit_attachments.clone(),
                revision,
            },
            (_, Some(state)) => ComposerSnapshot {
                text: state.text.clone(),
                attachments: state.attachments.clone(),
                revision,
            },
            (_, None) => ComposerSnapshot {
                text: String::new(),
                attachments: Vec::new(),
                revision,
            },
        }
    }

    /// 发送事务内的草稿消费指令(冻结当前版本)。
    pub fn consumption(
        &self,
        conv: ConversationId,
        mode: ComposerMode,
    ) -> Option<DraftConsumptionRequest> {
        let state = self.states.get(&conv)?;
        Some(DraftConsumptionRequest {
            conversation_id: conv,
            expected_revision: state.revision,
            mode: match mode {
                ComposerMode::Normal => DraftConsumption::Normal,
                ComposerMode::Edit(_) => DraftConsumption::LatestEdit,
            },
        })
    }

    // ----- 写入(唯一写入点) -----

    /// 保存普通草稿。
    ///
    /// `changed` = 本次调用代表一次真实的内容变化(版本 +1):即使用户
    /// 重新输入了与库中相同的正文也算变化(发送事务的 CAS 据此区分
    /// “重新输入”与“旧草稿”)。`changed = false` 且内容一致时不写库、
    /// 不涨版本(纯光标/选区/滚动与冗余 flush)。
    pub fn save_normal(
        &mut self,
        conv: ConversationId,
        text: &str,
        attachments: &[Attachment],
        changed: bool,
    ) -> crate::storage::Result<i64> {
        let json = serde_json::to_string(attachments)?;
        let revision = self.db.save_draft_content(conv, text, &json, changed)?;
        let state = self.states.entry(conv).or_default();
        state.text = text.to_string();
        state.attachments = attachments.to_vec();
        state.revision = revision;
        Ok(revision)
    }

    /// 保存“编辑最新问题”缓冲(与普通草稿相互独立)。
    pub fn save_edit(
        &mut self,
        conv: ConversationId,
        text: &str,
        attachments: &[Attachment],
        target: Option<MessageId>,
        changed: bool,
    ) -> crate::storage::Result<i64> {
        let json = serde_json::to_string(attachments)?;
        let revision = self
            .db
            .save_edit_buffer(conv, text, &json, target, changed)?;
        let state = self.states.entry(conv).or_default();
        state.edit_text = text.to_string();
        state.edit_attachments = attachments.to_vec();
        state.edit_target = target;
        state.revision = revision;
        Ok(revision)
    }

    /// 清除“编辑最新问题”缓冲(普通草稿不动)。
    pub fn clear_edit(&mut self, conv: ConversationId) -> crate::storage::Result<i64> {
        let revision = self.db.clear_edit_buffer(conv)?;
        if let Some(state) = self.states.get_mut(&conv) {
            state.edit_text.clear();
            state.edit_attachments.clear();
            state.edit_target = None;
            state.revision = revision;
        }
        Ok(revision)
    }

    /// 进入编辑模式:普通草稿不动,编辑缓冲写入目标问题内容。
    pub fn begin_edit(
        &mut self,
        conv: ConversationId,
        target: MessageId,
        text: &str,
        attachments: &[Attachment],
    ) -> crate::storage::Result<i64> {
        self.save_edit(conv, text, attachments, Some(target), true)
    }

    // ----- 新会话槽位 -----

    /// 当前的新会话槽位(未发送)。
    pub fn pending_new(&self) -> Option<ConversationId> {
        self.pending_new
    }

    /// 记录新会话槽位。
    pub fn set_pending_new(&mut self, id: ConversationId) -> crate::storage::Result<()> {
        self.db.set_pending_new_conversation(id)?;
        self.pending_new = Some(id);
        Ok(())
    }

    /// 清除新会话槽位。
    ///
    /// `Some(id)` 只在指针指向该会话时清除(不影响后来创建的另一个槽位);
    /// `None` 无条件清除。
    pub fn clear_pending_new(&mut self, id: Option<ConversationId>) -> crate::storage::Result<()> {
        self.db.clear_pending_new_conversation(id)?;
        if id.is_none() || self.pending_new == id {
            self.pending_new = None;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(name: &str) -> Attachment {
        Attachment {
            name: name.into(),
            content: "body".into(),
        }
    }

    #[test]
    fn blank_text_without_attachments_is_not_content() {
        assert!(!has_content("   \n\t ", &[]));
        assert!(has_content("  x ", &[]));
        assert!(has_content("", &[attachment("a.txt")]));
    }

    #[test]
    fn snapshots_follow_the_active_mode() {
        let db = Arc::new(crate::storage::open_in_memory().unwrap());
        let mut store = DraftStore::new(db.clone());
        let conv = ConversationId::new();
        let row = Conversation::new(conv, crate::storage::now_dt());
        db.upsert_conversation(&row).unwrap();
        store.sync_from_row(&row);

        let target = MessageId::new();
        store.save_normal(conv, "normal", &[], true).unwrap();
        store
            .save_edit(conv, "edited", &[], Some(target), true)
            .unwrap();

        let normal = store.snapshot(conv, ComposerMode::Normal);
        assert_eq!(normal.text, "normal");
        let editing = store.snapshot(conv, ComposerMode::Edit(target));
        assert_eq!(editing.text, "edited");
        assert_eq!(normal.revision, editing.revision);
    }

    #[test]
    fn unchanged_content_does_not_bump_the_revision() {
        let db = Arc::new(crate::storage::open_in_memory().unwrap());
        let mut store = DraftStore::new(db.clone());
        let conv = ConversationId::new();
        let row = Conversation::new(conv, crate::storage::now_dt());
        db.upsert_conversation(&row).unwrap();
        store.sync_from_row(&row);

        let first = store.save_normal(conv, "hello", &[], true).unwrap();
        let second = store.save_normal(conv, "hello", &[], false).unwrap();
        assert_eq!(first, second);
        let third = store.save_normal(conv, "hello!", &[], true).unwrap();
        assert_eq!(third, first + 1);
    }

    #[test]
    fn retyping_the_same_text_is_still_a_new_revision() {
        let db = Arc::new(crate::storage::open_in_memory().unwrap());
        let mut store = DraftStore::new(db.clone());
        let conv = ConversationId::new();
        let row = Conversation::new(conv, crate::storage::now_dt());
        db.upsert_conversation(&row).unwrap();
        store.sync_from_row(&row);

        let first = store.save_normal(conv, "same", &[], true).unwrap();
        // 冗余 flush(内容未变):不涨版本。
        assert_eq!(store.save_normal(conv, "same", &[], false).unwrap(), first);
        // 重新输入同样的正文:算一次新变化,版本 +1(发送事务据此区分)。
        let again = store.save_normal(conv, "same", &[], true).unwrap();
        assert_eq!(again, first + 1);
    }

    #[test]
    fn clearing_the_edit_buffer_keeps_the_normal_draft() {
        let db = Arc::new(crate::storage::open_in_memory().unwrap());
        let mut store = DraftStore::new(db.clone());
        let conv = ConversationId::new();
        let row = Conversation::new(conv, crate::storage::now_dt());
        db.upsert_conversation(&row).unwrap();
        store.sync_from_row(&row);

        let target = MessageId::new();
        store.save_normal(conv, "normal", &[], true).unwrap();
        store.begin_edit(conv, target, "edited", &[]).unwrap();
        store.clear_edit(conv).unwrap();

        let back = db.conversation_of(conv).unwrap().unwrap();
        assert_eq!(back.draft, "normal");
        assert!(back.edit_draft.is_empty());
        assert_eq!(back.edit_target_id, None);
    }
}
