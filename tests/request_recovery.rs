//! B 系列验收:请求失败恢复、上下文完整性与重试防重
//! (本地计数 mock;无网络、无真实费用)。断言 DB/POST 数量。

mod common;

use std::sync::Arc;
use std::time::Duration;

use aicouncil::domain::plan::{ContinuationSelection, Planner};
use aicouncil::domain::{
    Conversation, ConversationId, DispatchPhase, FailureKind, GenerationStatus, MessageTree,
    ProviderId, Role,
};
use aicouncil::providers::openrouter::OpenRouter;
use aicouncil::services::chat::{AdmissionError, SubmitError};
use aicouncil::services::egress::{EgressObservation, EgressVerdict};
use aicouncil::services::ChatService;
use aicouncil::storage::{open_in_memory, Db};
use tokio::sync::broadcast;

fn ok_response() -> Vec<u8> {
    common::sse_ok(&format!(
        "{}{}data: [DONE]\n\n",
        common::delta("回答内容"),
        common::finish_stop("gen-ok")
    ))
}

struct Env {
    db: Arc<Db>,
    svc: Arc<ChatService<OpenRouter>>,
    mock: common::CountingMock,
}

async fn env_with(respond: impl Fn(&str, &[u8]) -> Vec<u8> + Send + Sync + 'static) -> Env {
    let db = Arc::new(open_in_memory().unwrap());
    let mock = common::spawn_counting_mock(respond).await;
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        format!("http://127.0.0.1:{}/v1", mock.port),
        "test-key".into(),
    ));
    let (tx, _rx) = broadcast::channel(4096);
    let svc = Arc::new(
        ChatService::new(provider, db.clone(), tx, 4).with_egress_guard(Arc::new(
            aicouncil::services::egress::StaticGuard::new(EgressVerdict::Allowed(
                EgressObservation {
                    ip: Some("203.0.113.10".into()),
                    country_code: Some("US".into()),
                    source: "test",
                    checked_at: chrono::Utc::now(),
                },
            )),
        )),
    );
    Env { db, svc, mock }
}

/// submit 后立即取消:轮询重发 cancel 直到终态(token 注册存在窗口)。
async fn ensure_cancelled(
    svc: &ChatService<OpenRouter>,
    db: &Db,
    conv: ConversationId,
    req: aicouncil::domain::GenerationId,
) {
    for _ in 0..50 {
        svc.cancel(req).await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let done = db
            .generations_of(conv)
            .unwrap()
            .iter()
            .any(|g| g.request_id == req && g.status.is_terminal());
        if done {
            return;
        }
    }
}

fn plan_single(
    db: &Db,
    conv: ConversationId,
    tree: &MessageTree,
    leaf: Option<aicouncil::domain::MessageId>,
    model: &str,
    prompt: &str,
) -> aicouncil::domain::SendPlan {
    db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    Planner::prepare(
        conv,
        &ContinuationSelection::Single {
            leaf,
            provider_id: ProviderId::new(),
            model_id: model.to_string(),
        },
        prompt,
        &[],
        None,
        tree,
        chrono::Utc::now(),
    )
    .unwrap()
}

fn tree_of(db: &Db, conv: ConversationId) -> MessageTree {
    let (tree, orphans) = MessageTree::from_messages(db.messages_of(conv).unwrap());
    assert_eq!(orphans, 0);
    tree
}

async fn wait_terminal(db: &Db, conv: ConversationId) -> Vec<aicouncil::domain::Generation> {
    assert!(
        common::wait_for(
            || db
                .generations_of(conv)
                .unwrap()
                .iter()
                .all(|g| g.status.is_terminal()),
            5000
        )
        .await,
        "应全部到终态"
    );
    db.generations_of(conv).unwrap()
}

/// B01:首问全部失败后输入不同新问 —— 发送成功;旧失败卡保留;
/// 无需原问重试;新 user 仅一条。
#[tokio::test]
async fn b01_new_question_after_total_failure() {
    let fail = std::sync::atomic::AtomicBool::new(true);
    let e = env_with(move |_b, _h| {
        if fail.swap(false, std::sync::atomic::Ordering::SeqCst) {
            common::http_err(500, r#"{"error":{"message":"server boom"}}"#)
        } else {
            ok_response()
        }
    })
    .await;
    let conv = ConversationId::new();
    // 首问(失败)
    let p1 = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "第一问");
    e.svc.submit(p1).await.unwrap();
    let gs1 = wait_terminal(&e.db, conv).await;
    assert!(
        gs1.iter().all(|g| g.status == GenerationStatus::Failed),
        "{gs1:?}"
    );
    let failed_assistant = gs1[0].assistant_message_id;

    // 新问(不重试原问):失败卡是 main_leaf,新轮次应被允许(服务不因失败阻塞)
    let tree = tree_of(&e.db, conv);
    let p2 = plan_single(
        &e.db,
        conv,
        &tree,
        Some(failed_assistant),
        "mock/b",
        "第二问",
    );
    e.svc.submit(p2).await.unwrap();
    let gs2 = wait_terminal(&e.db, conv).await;
    assert_eq!(gs2.len(), 2);
    assert!(
        gs2.iter().any(|g| g.status == GenerationStatus::Completed),
        "{gs2:?}"
    );

    // 旧失败卡保留;新 user 仅一条
    let users =
        e.db.messages_of(conv)
            .unwrap()
            .iter()
            .filter(|m| m.role == Role::User)
            .count();
    assert_eq!(users, 2, "两次提问两个 user");
    let posts = e.mock.count();
    assert_eq!(posts, 2, "两次发送各一个 POST");
    e.mock.handle.abort();
}

/// B03:U1 + 空失败 A1,随后 U2 —— 捕获 body 含 U1/U2 且顺序正确,
/// 无空 assistant/错误文案进上下文。
#[tokio::test]
async fn b03_context_keeps_users_drops_empty_failure() {
    let fail = std::sync::atomic::AtomicBool::new(true);
    let e = env_with(move |_b, _h| {
        if fail.swap(false, std::sync::atomic::Ordering::SeqCst) {
            common::http_err(401, r#"{"error":{"message":"bad key"}}"#)
        } else {
            ok_response()
        }
    })
    .await;
    let conv = ConversationId::new();
    let p1 = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "U1");
    e.svc.submit(p1).await.unwrap();
    let gs1 = wait_terminal(&e.db, conv).await;
    assert_eq!(
        gs1[0].failure.as_ref().unwrap().kind(),
        FailureKind::Unauthorized
    );

    let tree = tree_of(&e.db, conv);
    // 空失败 assistant 保留在树中(供展示/审计),leaf 指向它
    let leaf = gs1[0].assistant_message_id;
    assert!(tree.get(leaf).unwrap().content.is_empty());
    let p2 = plan_single(&e.db, conv, &tree, Some(leaf), "mock/a", "U2");
    e.svc.submit(p2).await.unwrap();
    wait_terminal(&e.db, conv).await;

    let bodies = e.mock.all_bodies().await;
    let second = bodies.last().unwrap();
    // 顺序:history[U1] 之后 prompt[U2]
    let pos_u1 = second.find("U1").expect("含 U1");
    let pos_u2 = second.rfind("U2").expect("含 U2");
    assert!(pos_u1 < pos_u2, "U1 在 U2 前:{second}");
    // 空失败 assistant 与错误文案不进上下文
    assert!(!second.contains("bad key"), "错误说明不进模型上下文");
    assert!(!second.contains("失败"), "{second}");
    e.mock.handle.abort();
}

/// B04:U1 + 部分失败 A1(有部分文本),随后 U2 —— body 带 A1 实际部分文本。
#[tokio::test]
async fn b04_partial_failure_content_carried_forward() {
    let fail = std::sync::atomic::AtomicBool::new(true);
    let e = env_with(move |_b, _h| {
        if fail.swap(false, std::sync::atomic::Ordering::SeqCst) {
            // partial + error(连接在终态前以 error 帧结束)
            common::sse_ok(&format!(
                "{}data: {{\"error\":{{\"message\":\"mid fail\",\"code\":500}}}}\n\ndata: [DONE]\n\n",
                common::delta("部分回答"),
            ))
        } else {
            ok_response()
        }
    })
    .await;
    let conv = ConversationId::new();
    let p1 = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "U1");
    e.svc.submit(p1).await.unwrap();
    let gs1 = wait_terminal(&e.db, conv).await;
    assert_eq!(gs1[0].status, GenerationStatus::Failed);
    // 部分内容保留
    let msgs = e.db.messages_of(conv).unwrap();
    let a1 = msgs.iter().find(|m| m.role == Role::Assistant).unwrap();
    assert!(
        a1.content.contains("部分回答"),
        "部分输出保留:{}",
        a1.content
    );

    let tree = tree_of(&e.db, conv);
    let p2 = plan_single(
        &e.db,
        conv,
        &tree,
        Some(gs1[0].assistant_message_id),
        "mock/a",
        "U2",
    );
    e.svc.submit(p2).await.unwrap();
    wait_terminal(&e.db, conv).await;
    let bodies = e.mock.all_bodies().await;
    assert!(
        bodies.last().unwrap().contains("部分回答"),
        "B04:携带已收到文本"
    );
    e.mock.handle.abort();
}

/// B07:10 次同卡 retry 连点 + 直接并发服务调用 —— 一个活动 lineage、
/// 一个新 sibling、至多一个新 POST;旧 user 不复制。
#[tokio::test]
async fn b07_retry_dedup_ten_clicks() {
    // 先制造一条可重试的失败(网络错误无 RA → 退避;用取消制造 Allowed 资格)
    let e = env_with(|_b, _h| ok_response()).await;
    let conv = ConversationId::new();
    let p1 = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "Q1");
    let receipt = e.svc.submit(p1).await.unwrap();
    // submit 后立即取消(B15:token 已注册,可停止)
    ensure_cancelled(&e.svc, &e.db, conv, receipt.primary_request_id).await;
    let gs1 = wait_terminal(&e.db, conv).await;
    assert_eq!(gs1[0].status, GenerationStatus::Cancelled, "{gs1:?}");
    let before_posts = e.mock.count();
    let before_users =
        e.db.messages_of(conv)
            .unwrap()
            .iter()
            .filter(|m| m.role == Role::User)
            .count();

    // 10 次并发 retry(同一张卡)
    let original = gs1[0].clone();
    let mut handles = Vec::new();
    for _ in 0..10 {
        let svc = e.svc.clone();
        let orig = original.clone();
        handles.push(tokio::spawn(async move {
            svc.retry(conv, &orig, "Q1", vec![]).await
        }));
    }
    let mut ok = 0;
    let mut rejected = 0;
    for h in handles {
        match h.await.unwrap() {
            Ok(_) => ok += 1,
            Err(SubmitError::Admission(AdmissionError::Duplicate { .. })) => rejected += 1,
            Err(other) => panic!("意外错误:{other}"),
        }
    }
    assert_eq!(ok, 1, "只允许一个有效尝试(其余 {rejected} 拒绝)");
    assert!(rejected >= 9, "其余全部被拒:{rejected}");
    wait_terminal(&e.db, conv).await;

    let gs2 = e.db.generations_of(conv).unwrap();
    assert_eq!(gs2.len(), 2, "一个新 sibling");
    // 旧 user 不复制
    let users =
        e.db.messages_of(conv)
            .unwrap()
            .iter()
            .filter(|m| m.role == Role::User)
            .count();
    assert_eq!(users, before_users, "retry 不追加重复 user");
    // 至多一个新 POST
    let new_posts = e.mock.count() - before_posts;
    assert_eq!(new_posts, 1, "POST 增量=1");
    e.mock.handle.abort();
}

/// B08:同模型两个不同 user/分支均重试 —— 各自可创建一个,互不锁死。
#[tokio::test]
async fn b08_same_model_different_lineages_independent() {
    let e = env_with(|_b, _h| ok_response()).await;
    let conv = ConversationId::new();
    let p1 = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "Q1");
    let r1 = e.svc.submit(p1).await.unwrap();
    ensure_cancelled(&e.svc, &e.db, conv, r1.primary_request_id).await;
    let gs1 = wait_terminal(&e.db, conv).await;
    let g1 = gs1[0].clone();

    let tree = tree_of(&e.db, conv);
    let p2 = plan_single(
        &e.db,
        conv,
        &tree,
        Some(g1.assistant_message_id),
        "mock/a",
        "Q2",
    );
    let r2 = e.svc.submit(p2).await.unwrap();
    ensure_cancelled(&e.svc, &e.db, conv, r2.primary_request_id).await;
    let gs2 = wait_terminal(&e.db, conv).await;
    let g2 = gs2
        .iter()
        .find(|g| g.request_id != g1.request_id)
        .unwrap()
        .clone();

    // 两张卡都允许 retry(不同 lineage,不因同模型互相锁死)
    let o1 = e.svc.retry(conv, &g1, "Q1", vec![]).await;
    let o2 = e.svc.retry(conv, &g2, "Q2", vec![]).await;
    assert!(o1.is_ok(), "{:?}", o1.unwrap_err());
    assert!(o2.is_ok(), "{:?}", o2.unwrap_err());
    wait_terminal(&e.db, conv).await;
    assert_eq!(e.db.generations_of(conv).unwrap().len(), 4);
    e.mock.handle.abort();
}

/// B09/B10:错误分类、Retry-After 冷却持久化;到期前原样重试被拒,
/// 到期只解锁(不自动发送)。
#[tokio::test]
async fn b09_b10_classification_and_cooldown() {
    let e = env_with(|_b, _h| common::http_err(403, r#"{"error":{"message":"no policy"}}"#)).await;
    let conv = ConversationId::new();
    let p = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "Q");
    e.svc.submit(p).await.unwrap();
    let gs = wait_terminal(&e.db, conv).await;
    let g = &gs[0];
    // 403 → Forbidden(不统一解释成 Key 错误)
    assert_eq!(g.failure.as_ref().unwrap().kind(), FailureKind::Forbidden);
    assert_eq!(g.failure.as_ref().unwrap().http_status, Some(403));

    // Retry-After 冷却:手工写一条带冷却的失败,验证服务端拒绝逻辑
    let mut info = aicouncil::domain::FailureInfo::new(
        FailureKind::RateLimited,
        "429",
        DispatchPhase::DispatchAttempted,
    );
    info.provider_code = Some(aicouncil::domain::recovery::encode_retry_after(120));
    aicouncil::domain::recovery::apply_backoff(&mut info, 1);
    let until = info.retry_not_before_ms.unwrap();
    assert!(until > aicouncil::domain::recovery::now_ms(), "冷却在未来");
    e.db.update_generation_status(g.request_id, GenerationStatus::Failed, Some("429"), None)
        .unwrap();
    // 直接 SQL 写 failure_json(服务路径已由 429 mock 覆盖;此处验证读取与判定)
    e.db
        .lock()
        .execute(
            "UPDATE generations SET failure_json = ?2, retry_not_before_ms = ?3 WHERE request_id = ?1",
            rusqlite::params![g.request_id.as_str(), info.to_json(), until],
        )
        .unwrap();

    // 到期前重试被拒(Cooldown)
    let err = e.svc.retry(conv, g, "Q", vec![]).await.unwrap_err();
    assert!(
        matches!(err, SubmitError::Admission(AdmissionError::Cooldown { until_ms }) if until_ms == until),
        "{err:?}"
    );
    assert_eq!(e.mock.count(), 1, "冷却期零额外 POST(不自动发送)");
    e.mock.handle.abort();
}

/// B13:新问题已发送/用户换支 —— 旧请求晚完成只更新原卡,
/// 不抢回 main_leaf。
#[tokio::test]
async fn b13_late_completion_does_not_steal_main_leaf() {
    let first_slow = std::sync::atomic::AtomicBool::new(true);
    let e = env_with(move |_b, _h| {
        if first_slow.swap(false, std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(600));
            ok_response()
        } else {
            ok_response()
        }
    })
    .await;
    let conv = ConversationId::new();
    // 第一问:主请求(慢)
    let p1 = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "Q1");
    let r1 = e.svc.submit(p1).await.unwrap();
    let slow_assistant = *r1.assistant_of.get(&r1.primary_request_id).unwrap();

    // 用户在慢请求完成前把 leaf 移到别的位置(模拟用户选择)
    let other = aicouncil::domain::MessageId::new();
    let _ = e.db.set_conversation_main_leaf(conv, Some(other));

    wait_terminal(&e.db, conv).await;
    // 晚完成不抢回 main_leaf(CAS 期望 [user, 自身] 不含 other)
    let leaf = e.db.main_leaf_of(conv).unwrap();
    assert_eq!(leaf, Some(other), "main_leaf 保持用户后来的选择");
    let _ = slow_assistant;
    e.mock.handle.abort();
}

/// B15:submit 后立刻 Stop —— token 可见;请求被取消;
/// 排队 Stop/检测中取消由同一 cancel 通道覆盖。
#[tokio::test]
async fn b15_stop_immediately_after_submit() {
    let e = env_with(|_b, _h| {
        std::thread::sleep(Duration::from_millis(300));
        ok_response()
    })
    .await;
    let conv = ConversationId::new();
    let p = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "Q");
    let r = e.svc.submit(p).await.unwrap();
    ensure_cancelled(&e.svc, &e.db, conv, r.primary_request_id).await;
    let gs = wait_terminal(&e.db, conv).await;
    assert_eq!(gs[0].status, GenerationStatus::Cancelled, "{gs:?}");
    e.mock.handle.abort();
}

/// B16:重试前用户改参数/系统提示词/当前模型 —— 新尝试使用原快照
/// 和原模型。(用取消制造无冷却的失败,重试资格 Allowed)
#[tokio::test]
async fn b16_retry_uses_original_snapshot() {
    let e = env_with(|_b, _h| {
        std::thread::sleep(Duration::from_millis(300));
        ok_response()
    })
    .await;
    let conv = ConversationId::new();
    // 原请求:mock/a + 系统提示词 SYS + 参数
    let mut plan = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "原始问题");
    plan.system_prompt = Some("SYS".into());
    plan.branches[0].requests[0].params = Some(serde_json::json!({"temperature": 0.9}));
    let r = e.svc.submit(plan).await.unwrap();
    ensure_cancelled(&e.svc, &e.db, conv, r.primary_request_id).await;
    let gs = wait_terminal(&e.db, conv).await;
    assert_eq!(gs[0].status, GenerationStatus::Cancelled);
    let original = gs[0].clone();

    // 重试(即使"当前"目录已没有该模型/参数已变):应沿用快照
    let receipt = e
        .svc
        .retry(conv, &original, "调用方重建的提示词", vec![])
        .await
        .unwrap();
    wait_terminal(&e.db, conv).await;
    let gs2 = e.db.generations_of(conv).unwrap();
    let retried = gs2
        .iter()
        .find(|g| g.request_id == receipt.primary_request_id)
        .unwrap();
    assert_eq!(retried.model_id, "mock/a", "沿用原模型");
    assert_eq!(
        retried.system_prompt.as_deref(),
        Some("SYS"),
        "沿用原系统提示词"
    );
    assert_eq!(
        retried.params,
        Some(serde_json::json!({"temperature": 0.9})),
        "沿用原参数"
    );
    // 快照存在且完整
    let snap = retried.request_snapshot.as_ref().unwrap();
    assert_eq!(snap.model_id, "mock/a");
    assert_eq!(snap.prompt, "原始问题");
    // body 用原模型
    let bodies = e.mock.all_bodies().await;
    assert!(bodies.last().unwrap().contains("mock/a"));
    assert!(
        bodies.last().unwrap().contains("原始问题"),
        "用原快照 prompt"
    );
    assert!(bodies.last().unwrap().contains("SYS"));
    e.mock.handle.abort();
}

/// B17:重试最新卡成功 —— lineage 聚合为多次尝试,账目按请求独立。
#[tokio::test]
async fn b17_lineage_aggregation_counts() {
    let e = env_with(|_b, _h| {
        std::thread::sleep(Duration::from_millis(300));
        ok_response()
    })
    .await;
    let conv = ConversationId::new();
    let p = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "Q");
    let r = e.svc.submit(p).await.unwrap();
    ensure_cancelled(&e.svc, &e.db, conv, r.primary_request_id).await;
    let gs = wait_terminal(&e.db, conv).await;
    assert_eq!(gs[0].status, GenerationStatus::Cancelled);
    let original = gs[0].clone();
    let root = original.retry_root_id.unwrap_or(original.request_id);

    let receipt = e.svc.retry(conv, &original, "Q", vec![]).await.unwrap();
    wait_terminal(&e.db, conv).await;

    // lineage:2 次尝试,同一 root
    let lineage = e.db.generations_of_lineage(root).unwrap();
    assert_eq!(lineage.len(), 2, "尝试数=2");
    assert!(lineage
        .iter()
        .any(|(_, s)| *s == GenerationStatus::Completed));
    // 新尝试挂同一 user 下(兄弟,不追加 user)
    let users =
        e.db.messages_of(conv)
            .unwrap()
            .iter()
            .filter(|m| m.role == Role::User)
            .count();
    assert_eq!(users, 1);
    // 账目按请求独立
    let usage = e.db.usage_of(conv).unwrap();
    assert_eq!(usage.len(), 2);
    let _ = receipt;
    e.mock.handle.abort();
}

/// B11(服务侧):submit 存储 mocked 失败 —— 无 POST、无假消息;
/// 草稿安全由 App 层保障(此处验证服务返回错误而非假成功)。
#[tokio::test]
async fn b11_storage_failure_no_post_no_fake_rows() {
    // 计划指向未落库的会话:外键违反 → 单事务回滚
    let e = env_with(|_b, _h| ok_response()).await;
    let conv = ConversationId::new(); // 故意不 upsert_conversation
    let plan = Planner::prepare(
        conv,
        &ContinuationSelection::Single {
            leaf: None,
            provider_id: ProviderId::new(),
            model_id: "mock/a".into(),
        },
        "Q",
        &[],
        None,
        &MessageTree::new(),
        chrono::Utc::now(),
    )
    .unwrap();
    let err = e.svc.submit(plan).await.unwrap_err();
    assert!(matches!(err, SubmitError::Storage(_)), "{err:?}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(e.mock.count(), 0, "无 POST");
    let n: i64 =
        e.db.lock()
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
            .unwrap();
    assert_eq!(n, 0, "无假消息");
    e.mock.handle.abort();
}

/// B02(部分):重启后的失败/中断状态来自 DB;
/// recover_interrupted 不自动新增任何生成。
#[tokio::test]
async fn b02_restart_recovery_from_db_only() {
    let e = env_with(|_b, _h| ok_response()).await;
    let conv = ConversationId::new();
    let p = plan_single(&e.db, conv, &MessageTree::new(), None, "mock/a", "Q");
    e.svc.submit(p).await.unwrap();
    // 不等终态,直接"重启"
    let n = e.db.recover_interrupted().unwrap();
    assert_eq!(n, 1, "活动请求 → Interrupted");
    let gs = e.db.generations_of(conv).unwrap();
    assert_eq!(gs.len(), 1, "不自动新增生成");
    assert_eq!(gs[0].status, GenerationStatus::Interrupted);
    assert!(gs[0].error.is_some());
    // Interrupted 是终态:retry 资格 = Allowed(显式)
    let original = gs[0].clone();
    let ok = e.svc.retry(conv, &original, "Q", vec![]).await;
    assert!(ok.is_ok(), "中断卡可显式重试:{:?}", ok.unwrap_err());
    wait_terminal(&e.db, conv).await;
    // 首请求可能仍在流中;此时 lineage 唯一活动约束会把 Interrupted 视为终态,
    // 新重试是唯一活动
    e.mock.handle.abort();
}
