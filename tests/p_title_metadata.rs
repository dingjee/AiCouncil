//! 标题元数据迁移 + 字段级 CAS + 辅助请求台账 + 用量排名(T06/T07/T08/T05)。

use aicouncil::domain::{
    BatchId, Conversation, ConversationId, Generation, GenerationId, GenerationStatus, Message,
    MessageId, ProviderId, Role, TitleSource, TitleStatus,
};
use aicouncil::storage::{open, open_in_memory, Db, SendBatchRecord};

const C1: &str = "00000000-0000-4000-8000-0000000000e1";
const C2: &str = "00000000-0000-4000-8000-0000000000e2";

fn conv(id: &str) -> Conversation {
    Conversation::new(ConversationId::from_uuid_str(id), chrono::Utc::now())
}

#[test]
fn migration_0003_protects_existing_titles() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    {
        // 模拟 0.1.2 库(0001+0002 已登记)
        let conn = rusqlite::Connection::open(&path).unwrap();
        for (name, sql) in [
            ("0001_init", include_str!("../migrations/0001_init.sql")),
            (
                "0002_attachments",
                include_str!("../migrations/0002_attachments.sql"),
            ),
        ] {
            conn.execute_batch(sql).unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    name TEXT PRIMARY KEY, applied_at INTEGER NOT NULL);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO schema_migrations(name, applied_at) VALUES (?1, 0)",
                [name],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO conversations(id, title, system_prompt, draft, created_at, updated_at)
             VALUES (?1, '手动标题', '', '', 1, 1)",
            [C1],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO conversations(id, title, system_prompt, draft, created_at, updated_at)
             VALUES (?1, '', '', '', 1, 1)",
            [C2],
        )
        .unwrap();
    }
    let db = open(&path).unwrap();
    let convs = db.conversations().unwrap();
    assert_eq!(convs.len(), 2, "旧会话应全部保留");
    let titled = convs.iter().find(|c| c.id.as_str() == C1).unwrap();
    let empty = convs.iter().find(|c| c.id.as_str() == C2).unwrap();
    // T08:非空旧标题保护为 manual;空标题进入自动流程
    assert_eq!(titled.title_source, TitleSource::Manual);
    assert_eq!(titled.title_status, TitleStatus::Ready);
    assert_eq!(empty.title_source, TitleSource::Empty);
    assert_eq!(empty.title_status, TitleStatus::NeedsGeneration);
}

#[test]
fn draft_update_never_clobbers_title() {
    let db = open_in_memory().unwrap();
    let mut c = conv(C1);
    c.title = "自动生成的标题".into();
    c.title_source = TitleSource::Generated;
    c.title_status = TitleStatus::Ready;
    c.title_revision = 3;
    db.upsert_conversation(&c).unwrap();
    // 旧对象携带过期标题全量 upsert(T07)
    let mut stale = c.clone();
    stale.title = String::new();
    stale.title_source = TitleSource::Empty;
    stale.title_revision = 0;
    db.upsert_conversation(&stale).unwrap();
    let back = &db.conversations().unwrap()[0];
    assert_eq!(back.title, "自动生成的标题", "草稿更新不得覆盖标题");
    assert_eq!(back.title_source, TitleSource::Generated);
    assert_eq!(back.title_revision, 3);
}

#[test]
fn rename_cas_rejects_stale_revision_and_manual_is_final() {
    let db = open_in_memory().unwrap();
    let id = ConversationId::from_uuid_str(C1);
    db.upsert_conversation(&conv(C1)).unwrap();
    // 手动改名 rev0 → 成功,rev+1
    let renamed = db
        .rename_conversation_title(id, "我的会话", 0)
        .unwrap()
        .unwrap();
    assert_eq!(renamed.title, "我的会话");
    assert_eq!(renamed.title_source, TitleSource::Manual);
    assert_eq!(renamed.title_revision, 1);
    // 旧版本重试 → CAS 失败(T07)
    assert!(db
        .rename_conversation_title(id, "旧结果", 0)
        .unwrap()
        .is_none());
    // manual 不可被自动覆盖
    assert!(db
        .apply_generated_title(id, 1, "自动标题")
        .unwrap()
        .is_none());
}

#[test]
fn title_attempt_flow_with_aux_ledger() {
    let db = open_in_memory().unwrap();
    let id = ConversationId::from_uuid_str(C1);
    db.upsert_conversation(&conv(C1)).unwrap();
    assert!(db
        .begin_title_attempt(id, Some("turn-1"), "tok-1", "deepseek-flash", 0)
        .unwrap());
    let convs = db.conversations().unwrap();
    assert_eq!(convs[0].title_status, TitleStatus::InFlight);
    assert_eq!(convs[0].title_attempt_count, 1);
    // 同会话已有 in_flight(needs_generation 不满足)→ 拒绝二次
    assert!(!db
        .begin_title_attempt(id, Some("turn-1"), "tok-2", "deepseek-flash", 0)
        .unwrap());
    // 失败 → needs_generation + 台账 failed(下次提问再试,T06)
    db.fail_title_attempt(id, "tok-1", "timeout").unwrap();
    let convs = db.conversations().unwrap();
    assert_eq!(convs[0].title_status, TitleStatus::NeedsGeneration);
    let aux = db.aux_usage_of(id).unwrap();
    assert_eq!(aux.len(), 1);
    assert_eq!(aux[0].status, "failed");
    assert_eq!(aux[0].error_kind.as_deref(), Some("timeout"));
    assert_eq!(aux[0].cost_status, "unknown");
    // 成功路径:apply_generated_title 落库 + 台账 completed
    assert!(db
        .begin_title_attempt(id, Some("turn-2"), "tok-3", "deepseek-flash", 0)
        .unwrap());
    let applied = db
        .apply_generated_title(id, 0, "第一次提问的标题")
        .unwrap()
        .unwrap();
    assert_eq!(applied.title, "第一次提问的标题");
    assert_eq!(applied.title_source, TitleSource::Generated);
    assert_eq!(applied.title_status, TitleStatus::Ready);
    // 成功后再次 apply(CAS 旧 rev)→ None
    assert!(db
        .apply_generated_title(id, 0, "迟到的结果")
        .unwrap()
        .is_none());
    db.complete_aux_request("tok-3", Some(120), Some(8), Some("0.000001".into()))
        .unwrap();
    let aux = db.aux_usage_of(id).unwrap();
    let done = aux.iter().find(|a| a.request_token == "tok-3").unwrap();
    assert_eq!(done.status, "completed");
    assert_eq!(done.cost_status, "known");
}

#[test]
fn recover_in_flight_on_restart() {
    let db = open_in_memory().unwrap();
    let id = ConversationId::from_uuid_str(C1);
    db.upsert_conversation(&conv(C1)).unwrap();
    db.begin_title_attempt(id, Some("t"), "tok", "m", 0)
        .unwrap();
    let n = db.recover_title_in_flight().unwrap();
    assert_eq!(n, 1);
    let convs = db.conversations().unwrap();
    assert_eq!(convs[0].title_status, TitleStatus::NeedsGeneration);
}

fn mk_generation(
    db: &Db,
    conv_id: ConversationId,
    model: &str,
    provider: ProviderId,
    started_at: Option<i64>,
) {
    let user = Message {
        id: MessageId::new(),
        conversation_id: conv_id,
        parent_id: None,
        role: Role::User,
        content: "q".into(),
        attachments: vec![],
        created_at: chrono::Utc::now(),
    };
    let assistant = Message {
        id: MessageId::new(),
        conversation_id: conv_id,
        parent_id: Some(user.id),
        role: Role::Assistant,
        content: String::new(),
        attachments: vec![],
        created_at: chrono::Utc::now(),
    };
    let request_id = GenerationId::new();
    let gen = Generation {
        request_id,
        batch_id: BatchId::new(),
        user_message_id: user.id,
        assistant_message_id: assistant.id,
        provider_id: provider,
        model_id: model.to_string(),
        status: if started_at.is_some() {
            GenerationStatus::Completed
        } else {
            GenerationStatus::Queued
        },
        params: None,
        system_prompt: None,
        context_snapshot: vec![],
        price_snapshot: None,
        remote_generation_id: None,
        error: None,
        retry_of: None,
        retry_root_id: Some(request_id),
        failure: None,
        request_snapshot: None,
        retry_not_before_ms: None,
        dispatch_phase: aicouncil::domain::DispatchPhase::NotDispatched,
        started_at: started_at.and_then(chrono::DateTime::from_timestamp_millis),
        finished_at: None,
    };
    db.save_send_batch(&SendBatchRecord {
        batch_id: gen.batch_id,
        conversation_id: conv_id,
        primary_request_id: gen.request_id,
        user_messages: vec![user],
        assistant_messages: vec![assistant],
        generations: vec![gen],
        usage: vec![],
        intended_main_leaf: None,
    })
    .unwrap();
}

#[test]
fn usage_rank_orders_by_count_then_recency_stable() {
    // T05:排名确定性(不依赖 HashMap 顺序);未 Started 不计
    let db = open_in_memory().unwrap();
    let id = ConversationId::from_uuid_str(C1);
    db.upsert_conversation(&conv(C1)).unwrap();
    let provider = ProviderId::new();
    mk_generation(&db, id, "m/a", provider, Some(100));
    mk_generation(&db, id, "m/b", provider, Some(200));
    mk_generation(&db, id, "m/b", provider, Some(300));
    mk_generation(&db, id, "m/c", provider, None); // 未出队取消:不计
    let rank = db.model_usage_rank(provider).unwrap();
    let ids: Vec<&str> = rank.iter().map(|(m, _, _)| m.as_str()).collect();
    assert_eq!(ids, vec!["m/b", "m/a"], "m/b 2 次居首;未 Started 不参与");
}
