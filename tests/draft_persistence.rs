//! Composer 草稿持久化验收(真实 SQLite 文件 + 重开 + 服务事务)。
//!
//! 覆盖交接计划的草稿验收矩阵:
//! - 逐会话独立保存(无串稿、无旧缓存覆盖);
//! - 未发送的新会话槽位(New 语义、重启后同槽位、不多造空壳);
//! - 空白草稿不复活旧内容;附件按名称与顺序恢复;
//! - 迟到回执按 (会话, 版本) 消费草稿,提交后输入的新内容保留;
//! - “编辑最新问题”缓冲与普通草稿互不覆盖;
//! - 仅光标/选区/滚动不触发写入,重新输入同样文本算新版本。

use std::sync::Arc;

use aicouncil::app::drafts::{ComposerMode, DraftStore};
use aicouncil::domain::{
    Attachment, BatchId, Conversation, ConversationId, DispatchPhase, Generation, GenerationId,
    GenerationStatus, Message, MessageId, ProviderId, Role, UsageRecord,
};
use aicouncil::storage::{open, DraftConsumption, DraftConsumptionRequest, SendBatchRecord};

fn db_at(path: &std::path::Path) -> Arc<aicouncil::storage::Db> {
    Arc::new(open(path).unwrap())
}

fn new_conversation(db: &aicouncil::storage::Db) -> ConversationId {
    let id = ConversationId::new();
    db.upsert_conversation(&Conversation::new(id, chrono::Utc::now()))
        .unwrap();
    id
}

fn store_for(db: &Arc<aicouncil::storage::Db>) -> DraftStore {
    let mut store = DraftStore::new(db.clone());
    for row in db.conversations().unwrap() {
        store.sync_from_row(&row);
    }
    store
}

fn attachment(name: &str, body: &str) -> Attachment {
    Attachment {
        name: name.into(),
        content: body.into(),
    }
}

/// 模拟一次发送落库(与 UI 走同一条 `save_send_batch` 事务)。
fn save_send_batch(
    db: &aicouncil::storage::Db,
    conversation: ConversationId,
    draft_consumption: Option<DraftConsumptionRequest>,
) {
    let batch = BatchId::new();
    let request = GenerationId::new();
    let user = MessageId::new();
    let assistant = MessageId::new();
    let provider = ProviderId::new();
    let now = chrono::Utc::now();
    db.save_send_batch(&SendBatchRecord {
        batch_id: batch,
        conversation_id: conversation,
        primary_request_id: request,
        user_messages: vec![Message {
            id: user,
            conversation_id: conversation,
            parent_id: None,
            role: Role::User,
            content: "question".into(),
            attachments: Vec::new(),
            created_at: now,
        }],
        assistant_messages: vec![Message {
            id: assistant,
            conversation_id: conversation,
            parent_id: Some(user),
            role: Role::Assistant,
            content: String::new(),
            attachments: Vec::new(),
            created_at: now,
        }],
        generations: vec![Generation {
            request_id: request,
            batch_id: batch,
            user_message_id: user,
            assistant_message_id: assistant,
            provider_id: provider,
            model_id: "test-model".into(),
            status: GenerationStatus::Queued,
            params: None,
            system_prompt: None,
            context_snapshot: Vec::new(),
            price_snapshot: None,
            remote_generation_id: None,
            error: None,
            retry_of: None,
            retry_root_id: Some(request),
            failure: None,
            request_snapshot: None,
            retry_not_before_ms: None,
            dispatch_phase: DispatchPhase::NotDispatched,
            started_at: None,
            finished_at: None,
        }],
        usage: vec![UsageRecord::pending(request)],
        intended_main_leaf: None,
        replacement: None,
        draft_consumption,
    })
    .unwrap();
}

#[test]
fn draft_survives_close_and_reopen_with_exact_text() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let text = "  第一行 \n\n第二行\t带制表符   \n尾随空格  ";
    let conversation;
    {
        let db = db_at(&path);
        conversation = new_conversation(&db);
        let mut store = store_for(&db);
        store.save_normal(conversation, text, &[], true).unwrap();
    } // 关闭(释放连接)

    let db = db_at(&path);
    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft, text, "原文必须精确恢复(空白与换行不丢)");
}

#[test]
fn each_conversation_keeps_its_own_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let a = new_conversation(&db);
    let b = new_conversation(&db);
    let mut store = store_for(&db);

    store
        .save_normal(a, "A 的正文", &[attachment("a.txt", "A")], true)
        .unwrap();
    store.save_normal(b, "B 的正文", &[], true).unwrap();

    // 反复切换(A → B → A → B)后各自仍是最新内容。
    assert_eq!(store.normal_text(a), "A 的正文");
    assert_eq!(store.normal_text(b), "B 的正文");
    let attachments = store.normal_attachments(a);
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].name, "a.txt");

    // 权威行同样分离。
    let row_a = db.conversation_of(a).unwrap().unwrap();
    let row_b = db.conversation_of(b).unwrap().unwrap();
    assert_eq!(row_a.draft, "A 的正文");
    assert_eq!(row_b.draft, "B 的正文");
    assert_eq!(row_a.draft_attachments.len(), 1);
    assert!(row_b.draft_attachments.is_empty());
}

#[test]
fn deleting_all_content_does_not_revive_the_old_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let conversation;
    {
        let db = db_at(&path);
        conversation = new_conversation(&db);
        let mut store = store_for(&db);
        store
            .save_normal(conversation, "旧内容", &[], true)
            .unwrap();
        // 用户把内容删空:必须持久化清空。
        store.save_normal(conversation, "", &[], true).unwrap();
    }
    let db = db_at(&path);
    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft, "", "旧内容不得复活");
    assert!(row.draft_attachments.is_empty());
}

#[test]
fn attachments_keep_name_and_order_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let conversation;
    {
        let db = db_at(&path);
        conversation = new_conversation(&db);
        let mut store = store_for(&db);
        store
            .save_normal(
                conversation,
                "带附件的草稿",
                &[
                    attachment("first.txt", "1"),
                    attachment("second.txt", "2"),
                    attachment("third.txt", "3"),
                ],
                true,
            )
            .unwrap();
    }
    let db = db_at(&path);
    let row = db.conversation_of(conversation).unwrap().unwrap();
    let names: Vec<&str> = row
        .draft_attachments
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    assert_eq!(names, ["first.txt", "second.txt", "third.txt"]);
    assert_eq!(row.draft_attachments[1].content, "2");
}

#[test]
fn new_conversation_slot_is_reused_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let slot;
    {
        let db = db_at(&path);
        slot = new_conversation(&db);
        let mut store = store_for(&db);
        store.set_pending_new(slot).unwrap();
        store.save_normal(slot, "未发送的正文", &[], true).unwrap();
    }

    // 重开:指针仍在,槽位有效(该会话没有任何消息)。
    let db = db_at(&path);
    let conversations = db.conversations().unwrap();
    let mut store = DraftStore::new(db.clone());
    store.load_on_start(&conversations, &|_| true);
    assert_eq!(store.pending_new(), Some(slot));
    assert_eq!(store.normal_text(slot), "未发送的正文");
}

#[test]
fn startup_repairs_a_dangling_pending_new_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let ghost = ConversationId::new();
    {
        let db = db_at(&path);
        db.set_pending_new_conversation(ghost).unwrap();
    }
    let db = db_at(&path);
    let conversations = db.conversations().unwrap();
    let mut store = DraftStore::new(db.clone());
    store.load_on_start(&conversations, &|_| true);
    assert_eq!(store.pending_new(), None, "悬空指针必须安全修复");
    assert_eq!(db.pending_new_conversation().unwrap(), None);
}

#[test]
fn first_send_releases_only_the_matching_new_slot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let slot = new_conversation(&db);
    let other = new_conversation(&db);
    let mut store = store_for(&db);
    store.set_pending_new(slot).unwrap();

    // 别的会话首发:不得释放该槽位。
    save_send_batch(&db, other, None);
    assert_eq!(store.pending_new(), Some(slot));

    // 槽位会话首发:释放指针。
    save_send_batch(&db, slot, None);
    assert_eq!(db.pending_new_conversation().unwrap(), None);
}

#[test]
fn deleting_the_slot_clears_the_pointer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let slot = new_conversation(&db);
    let mut store = store_for(&db);
    store.set_pending_new(slot).unwrap();

    db.delete_conversation(slot).unwrap();
    store.forget(slot);
    store.clear_pending_new(Some(slot)).unwrap();

    assert_eq!(store.pending_new(), None);
    assert_eq!(db.pending_new_conversation().unwrap(), None);
}

#[test]
fn send_transaction_consumes_the_submitted_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let conversation = new_conversation(&db);
    let mut store = store_for(&db);
    store
        .save_normal(conversation, "要发送的正文", &[], true)
        .unwrap();

    let consumption = store
        .consumption(conversation, ComposerMode::Normal)
        .expect("消费指令");
    save_send_batch(&db, conversation, Some(consumption));

    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft, "", "已提交版本的草稿被消费");
}

#[test]
fn late_receipt_keeps_input_typed_after_the_send() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let conversation = new_conversation(&db);
    let mut store = store_for(&db);
    store
        .save_normal(conversation, "第一段", &[], true)
        .unwrap();

    // 提交冻结版本。
    let consumption = store
        .consumption(conversation, ComposerMode::Normal)
        .expect("消费指令");
    // 回执迟到之前用户又输入了下一段。
    store
        .save_normal(conversation, "第二段", &[], true)
        .unwrap();

    save_send_batch(&db, conversation, Some(consumption));

    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft, "第二段", "迟到回执不得抹掉提交后的新输入");
}

#[test]
fn retyping_the_same_text_after_send_is_not_treated_as_the_old_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let conversation = new_conversation(&db);
    let mut store = store_for(&db);
    store
        .save_normal(conversation, "同样的正文", &[], true)
        .unwrap();

    let consumption = store
        .consumption(conversation, ComposerMode::Normal)
        .expect("消费指令");
    // 用户重新输入了完全相同的正文:版本必须前进。
    store
        .save_normal(conversation, "同样的正文", &[], true)
        .unwrap();

    save_send_batch(&db, conversation, Some(consumption));

    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft, "同样的正文", "重新输入的内容按新版本保留");
}

#[test]
fn cursor_only_changes_do_not_write_or_bump_the_revision() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let conversation = new_conversation(&db);
    let mut store = store_for(&db);
    let revision = store.save_normal(conversation, "正文", &[], true).unwrap();

    // 冗余 flush(光标移动/选区/滚动):内容一致 → 不写库、不涨版本。
    let same = store.save_normal(conversation, "正文", &[], false).unwrap();
    assert_eq!(same, revision);

    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft_revision, revision);
}

#[test]
fn latest_edit_buffer_is_independent_from_the_normal_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let conversation = new_conversation(&db);
    let mut store = store_for(&db);
    let target = MessageId::new();

    store
        .save_normal(conversation, "普通草稿", &[], true)
        .unwrap();
    store
        .begin_edit(conversation, target, "编辑后的提问", &[])
        .unwrap();

    // 两个缓冲互不覆盖。
    assert_eq!(store.normal_text(conversation), "普通草稿");
    assert_eq!(
        store.edit_buffer(conversation).map(|(t, _, _)| t),
        Some("编辑后的提问")
    );

    // 提交编辑:消费编辑缓冲,普通草稿保留。
    let consumption = store
        .consumption(conversation, ComposerMode::Edit(target))
        .expect("消费指令");
    save_send_batch(&db, conversation, Some(consumption));

    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft, "普通草稿", "编辑提交不得清掉普通草稿");
    assert_eq!(row.edit_draft, "");
    assert_eq!(row.edit_target_id, None);
}

#[test]
fn cancelling_the_edit_restores_the_normal_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let conversation = new_conversation(&db);
    let mut store = store_for(&db);
    let target = MessageId::new();

    store
        .save_normal(conversation, "普通草稿", &[], true)
        .unwrap();
    store
        .begin_edit(conversation, target, "编辑后的提问", &[])
        .unwrap();
    store.clear_edit(conversation).unwrap();

    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft, "普通草稿");
    assert_eq!(row.edit_draft, "");
    assert_eq!(store.edit_buffer(conversation), None);
}

#[test]
fn edited_question_draft_survives_reopen_without_auto_submitting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let conversation;
    let target = MessageId::new();
    {
        let db = db_at(&path);
        conversation = new_conversation(&db);
        let mut store = store_for(&db);
        store
            .save_normal(conversation, "普通草稿", &[], true)
            .unwrap();
        store
            .begin_edit(conversation, target, "编辑中的问题", &[])
            .unwrap();
    }
    let db = db_at(&path);
    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.edit_draft, "编辑中的问题");
    assert_eq!(row.edit_target_id, Some(target));
    assert_eq!(row.draft, "普通草稿");
    // 没有任何消息落库(编辑缓冲不会自动发送)。
    assert!(db.messages_of(conversation).unwrap().is_empty());
}

#[test]
fn title_and_leaf_updates_do_not_overwrite_the_draft() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let conversation = new_conversation(&db);
    let mut store = store_for(&db);
    store
        .save_normal(conversation, "草稿正文", &[], true)
        .unwrap();

    db.rename_conversation_title(conversation, "新标题", 0)
        .unwrap()
        .unwrap();
    db.set_conversation_main_leaf(conversation, None).unwrap();
    db.set_conversation_group(conversation, None).unwrap();

    let row = db.conversation_of(conversation).unwrap().unwrap();
    assert_eq!(row.draft, "草稿正文", "标题/主分支更新不得覆盖草稿");
    assert_eq!(row.title, "新标题");
}

#[test]
fn consumption_request_targets_the_same_conversation_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aicouncil.db");
    let db = db_at(&path);
    let a = new_conversation(&db);
    let b = new_conversation(&db);
    let mut store = store_for(&db);
    store.save_normal(a, "A 草稿", &[], true).unwrap();
    store.save_normal(b, "B 草稿", &[], true).unwrap();

    let consumption = store.consumption(a, ComposerMode::Normal).unwrap();
    // 消费指令指向别的会话时不得影响本会话。
    let foreign = DraftConsumptionRequest {
        conversation_id: b,
        expected_revision: consumption.expected_revision,
        mode: DraftConsumption::Normal,
    };
    save_send_batch(&db, a, Some(foreign));

    let row_a = db.conversation_of(a).unwrap().unwrap();
    let row_b = db.conversation_of(b).unwrap().unwrap();
    assert_eq!(row_a.draft, "A 草稿");
    assert_eq!(row_b.draft, "B 草稿");
}
