//! A 系列验收:Anthropic 全模型出口门禁(mock guard + 可计数本地 mock;
//! 无公网、无真实费用)。断言 DB 增量与生成 POST 数量,不 snapshot 文案。

mod common;

use std::sync::Arc;
use std::time::Duration;

use aicouncil::domain::plan::{ContinuationSelection, ModelTarget, Planner};
use aicouncil::domain::{Conversation, ConversationId, MessageTree, ProviderId};
use aicouncil::providers::openrouter::OpenRouter;
use aicouncil::services::chat::{AdmissionError, SubmitError};
use aicouncil::services::egress::{
    EgressGuard, EgressObservation, EgressPermit, EgressVerdict, StaticGuard, PERMIT_TTL,
};
use aicouncil::services::ChatService;
use aicouncil::storage::{open_in_memory, Db};
use tokio::sync::broadcast;

fn obs(cc: &str) -> EgressObservation {
    EgressObservation {
        ip: Some("203.0.113.10".into()),
        country_code: Some(cc.into()),
        source: "test",
        checked_at: chrono::Utc::now(),
    }
}

/// 可翻转判定 guard(模拟出口变化 US→CN)。
struct FlipGuard {
    verdict: tokio::sync::Mutex<EgressVerdict>,
    checks: std::sync::atomic::AtomicU64,
    revision: std::sync::atomic::AtomicU64,
}

impl FlipGuard {
    fn new(v: EgressVerdict) -> Self {
        Self {
            verdict: tokio::sync::Mutex::new(v),
            checks: std::sync::atomic::AtomicU64::new(0),
            revision: std::sync::atomic::AtomicU64::new(0),
        }
    }

    async fn set(&self, v: EgressVerdict) {
        *self.verdict.lock().await = v;
    }
}

impl aicouncil::services::egress::EgressGuard for FlipGuard {
    fn check(
        &self,
        _cancel: aicouncil::providers::cancel::CancellationToken,
    ) -> futures_util::future::BoxFuture<'static, EgressVerdict> {
        self.checks
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let v = self
            .verdict
            .try_lock()
            .map(|g| g.clone())
            .unwrap_or(EgressVerdict::Unknown("busy".into()));
        Box::pin(async move { v })
    }

    fn network_revision(&self) -> u64 {
        self.revision.load(std::sync::atomic::Ordering::SeqCst)
    }
}

fn anthropic_target(model: &str) -> ModelTarget {
    ModelTarget {
        provider_id: ProviderId::new(),
        model_id: model.to_string(),
        params: None,
        price: None,
    }
}

fn make_plan(
    conv: ConversationId,
    targets: Vec<ModelTarget>,
    primary: usize,
) -> aicouncil::domain::SendPlan {
    Planner::prepare(
        conv,
        &ContinuationSelection::SameQuestionMulti {
            leaf: None,
            targets,
            primary,
        },
        "你好",
        &[],
        None,
        &MessageTree::new(),
        chrono::Utc::now(),
    )
    .unwrap()
}

struct Env {
    db: Arc<Db>,
    svc: ChatService<OpenRouter>,
    #[allow(dead_code)]
    rx: broadcast::Receiver<aicouncil::services::ChatEvent>,
    guard: Arc<FlipGuard>,
    mock: common::CountingMock,
}

async fn env(verdict: EgressVerdict, response: Vec<u8>) -> Env {
    let db = Arc::new(open_in_memory().unwrap());
    let mock = common::spawn_counting_mock(move |_body, _head| response.clone()).await;
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        format!("http://127.0.0.1:{}/v1", mock.port),
        "test-key".into(),
    ));
    let (tx, rx) = broadcast::channel(4096);
    let guard = Arc::new(FlipGuard::new(verdict));
    let svc = ChatService::new(provider, db.clone(), tx, 4).with_egress_guard(guard.clone());
    Env {
        db,
        svc,
        rx,
        guard,
        mock,
    }
}

async fn env_static(verdict: EgressVerdict, response: Vec<u8>) -> Env2 {
    let db = Arc::new(open_in_memory().unwrap());
    let mock = common::spawn_counting_mock(move |_body, _head| response.clone()).await;
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        format!("http://127.0.0.1:{}/v1", mock.port),
        "test-key".into(),
    ));
    let (tx, rx) = broadcast::channel(4096);
    let guard = Arc::new(StaticGuard::new(verdict));
    let svc = ChatService::new(provider, db.clone(), tx, 4).with_egress_guard(guard.clone());
    Env2 {
        db,
        svc,
        rx,
        guard,
        mock,
    }
}

struct Env2 {
    db: Arc<Db>,
    svc: ChatService<OpenRouter>,
    #[allow(dead_code)]
    rx: broadcast::Receiver<aicouncil::services::ChatEvent>,
    guard: Arc<StaticGuard>,
    mock: common::CountingMock,
}

fn counts(db: &Db, conv: ConversationId) -> (usize, usize, usize) {
    let gens = db.generations_of(conv).unwrap().len();
    let msgs = db.messages_of(conv).unwrap().len();
    let batches: i64 = db
        .lock()
        .query_row("SELECT COUNT(*) FROM send_batches", [], |r| r.get(0))
        .unwrap();
    (batches as usize, gens, msgs)
}

/// A01:Anthropic 任意模型(Claude 与非 Claude fixture)+ CN/HK:
/// 生成 POST=0,batch/generation/assistant 增量=0。
#[tokio::test]
async fn a01_anthropic_blocked_cn_hk_zero_side_effects() {
    for cc in ["CN", "HK"] {
        for model in [
            "anthropic/claude-opus-5.5",
            "anthropic/claude-3.5-sonnet",
            "anthropic/test-non-claude-family", // A13 fixture:名称无 Claude
        ] {
            let e = env_static(
                EgressVerdict::Blocked(obs(cc)),
                common::sse_ok(&format!(
                    "{}{}data: [DONE]\n\n",
                    common::delta("x"),
                    common::finish_stop("g")
                )),
            )
            .await;
            let conv = ConversationId::new();
            e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
                .unwrap();
            let plan = make_plan(conv, vec![anthropic_target(model)], 0);
            let err = e.svc.submit(plan).await.unwrap_err();
            match &err {
                SubmitError::Admission(AdmissionError::EgressBlocked { country, .. }) => {
                    assert_eq!(country.as_deref(), Some(cc));
                }
                other => panic!("应拒绝:{other:?}"),
            }
            assert_eq!(e.mock.count(), 0, "生成 POST=0");
            assert_eq!(counts(&e.db, conv), (0, 0, 0), "零落库");
            e.mock.handle.abort();
        }
    }
}

/// A02:明确有效非 CN/HK 放行(原模型 ID);MO/TW 不扩大封禁。
#[tokio::test]
async fn a02_valid_countries_allowed_with_raw_ids() {
    for cc in ["US", "JP", "SG", "MO", "TW"] {
        let e = env_static(
            EgressVerdict::Allowed(obs(cc)),
            common::sse_ok(&format!(
                "{}{}data: [DONE]\n\n",
                common::delta("ok"),
                common::finish_stop("g1")
            )),
        )
        .await;
        let conv = ConversationId::new();
        e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
            .unwrap();
        let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")], 0);
        let receipt = e.svc.submit(plan).await.expect("明确有效结果可发送");
        assert_eq!(receipt.conversation_id, conv);
        assert!(
            common::wait_for(|| e.mock.count() == 1, 3000).await,
            "POST=1"
        );
        let bodies = e.mock.all_bodies().await;
        assert!(
            bodies[0].contains("anthropic/claude-opus-5.5"),
            "原模型 ID 不变"
        );
        e.mock.handle.abort();
    }
}

/// A03/A14:Unknown 拦截;非 Anthropic 显示名含 Claude 不误拦;
/// 受控 claude-* 别名归属 Anthropic 受门禁。
#[tokio::test]
async fn a03_a14_unknown_blocks_and_display_name_does_not() {
    // Unknown 拦截(检测服务断网等价:guard 返回 Unknown)
    let e = env_static(
        EgressVerdict::Unknown("检测超时".into()),
        common::sse_ok(&format!("{}data: [DONE]\n\n", common::finish_stop("g"))),
    )
    .await;
    let conv = ConversationId::new();
    e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    let plan = make_plan(
        conv,
        vec![anthropic_target("anthropic/claude-fable-5.1")],
        0,
    );
    assert!(matches!(
        e.svc.submit(plan).await.unwrap_err(),
        SubmitError::Admission(AdmissionError::EgressUnknown { .. })
    ));
    assert_eq!(e.mock.count(), 0);
    assert_eq!(counts(&e.db, conv), (0, 0, 0));
    e.mock.handle.abort();

    // 非 Anthropic 模型仅 ID 无 Anthropic 归属:显示名含 Claude 也不拦
    let e = env_static(
        EgressVerdict::Blocked(obs("CN")),
        common::sse_ok(&format!("{}data: [DONE]\n\n", common::finish_stop("g"))),
    )
    .await;
    let conv = ConversationId::new();
    e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    let plan = make_plan(
        conv,
        vec![anthropic_target("mock/claude-lookalike")], // 显示名可含 Claude
        0,
    );
    e.svc.submit(plan).await.expect("ID 非 Anthropic 归属不拦");
    assert!(common::wait_for(|| e.mock.count() == 1, 3000).await);
    e.mock.handle.abort();

    // 受控别名 claude-*:归属 Anthropic,同样被拦
    let e = env_static(
        EgressVerdict::Blocked(obs("CN")),
        common::sse_ok(&format!("{}data: [DONE]\n\n", common::finish_stop("g"))),
    )
    .await;
    let conv = ConversationId::new();
    e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    let plan = make_plan(
        conv,
        vec![anthropic_target("claude-3-5-sonnet-20241022")],
        0,
    );
    assert!(e.svc.submit(plan).await.is_err(), "claude-* 别名受门禁");
    assert_eq!(e.mock.count(), 0);
    e.mock.handle.abort();
}

/// A05:纯非 Anthropic 模型 + CN:无 GeoIP 依赖,正常发送。
#[tokio::test]
async fn a05_non_anthropic_has_no_geoip_dependency() {
    let e = env_static(
        EgressVerdict::Blocked(obs("CN")),
        common::sse_ok(&format!(
            "{}{}data: [DONE]\n\n",
            common::delta("好"),
            common::finish_stop("g2")
        )),
    )
    .await;
    let conv = ConversationId::new();
    e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    let plan = make_plan(
        conv,
        vec![
            anthropic_target("openai/gpt-4o"),
            anthropic_target("google/gemini-2.0-flash"),
        ],
        0,
    );
    e.svc.submit(plan).await.expect("非 Anthropic 不检测出口");
    assert_eq!(e.guard.check_count(), 0, "guard 零调用");
    assert!(common::wait_for(|| e.mock.count() == 2, 3000).await);
    e.mock.handle.abort();
}

/// A06:混合目标被整批拒绝(不能静默只发非 Anthropic);
/// 显式移除全部 Anthropic 模型后(新计划)剩余目标正常发送。
#[tokio::test]
async fn a06_mixed_batch_all_or_nothing() {
    let e = env_static(
        EgressVerdict::Blocked(obs("CN")),
        common::sse_ok(&format!("{}data: [DONE]\n\n", common::finish_stop("g"))),
    )
    .await;
    let conv = ConversationId::new();
    e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    let plan = make_plan(
        conv,
        vec![
            anthropic_target("anthropic/claude-opus-5.5"),
            anthropic_target("openai/gpt-4o"),
            anthropic_target("google/gemini-2.5-pro"),
        ],
        1, // 主模型是非 Anthropic:仍整批拒绝,不悄悄换主
    );
    assert!(e.svc.submit(plan).await.is_err(), "整批在落库前停止");
    assert_eq!(e.mock.count(), 0);
    assert_eq!(counts(&e.db, conv), (0, 0, 0));

    // "移除 Anthropic 模型后发送" = 用户显式新动作(新计划)
    let plan2 = make_plan(
        conv,
        vec![
            anthropic_target("openai/gpt-4o"),
            anthropic_target("google/gemini-2.5-pro"),
        ],
        0,
    );
    e.svc.submit(plan2).await.expect("剩余目标可发送");
    assert!(common::wait_for(|| e.mock.count() == 2, 3000).await);
    e.mock.handle.abort();
}

/// A07:同批三个 Anthropic 模型(含不同家族)共用该批预检:
/// allowed 后各目标只一个 POST。
#[tokio::test]
async fn a07_batch_shares_single_check() {
    let e = env_static(
        EgressVerdict::Allowed(obs("US")),
        common::sse_ok(&format!("{}data: [DONE]\n\n", common::finish_stop("g3"))),
    )
    .await;
    let conv = ConversationId::new();
    e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    let plan = make_plan(
        conv,
        vec![
            anthropic_target("anthropic/claude-opus-5.5"),
            anthropic_target("anthropic/claude-fable-5.1"),
            anthropic_target("anthropic/test-non-claude-family"),
        ],
        0,
    );
    e.svc.submit(plan).await.unwrap();
    assert_eq!(e.guard.check_count(), 1, "批内合并检测(一次)");
    assert!(
        common::wait_for(|| e.mock.count() == 3, 4000).await,
        "各目标一个 POST"
    );
    e.mock.handle.abort();
}

/// A08/A12:permit 过期/revision 变动时重检;每个新动作新检测,
/// 不长期复用 Allowed。
#[tokio::test]
async fn a08_stale_permit_rechecks_and_rejects() {
    let e = env(
        EgressVerdict::Allowed(obs("US")),
        common::sse_ok(&format!("{}data: [DONE]\n\n", common::finish_stop("g"))),
    )
    .await;
    let conv = ConversationId::new();
    e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();

    // 过期 permit + 出口已翻转为 CN:重检拒绝,未发请求标记 NotDispatched
    let stale = EgressPermit {
        checked_at_mono: tokio::time::Instant::now() - PERMIT_TTL - Duration::from_secs(1),
        revision: e.guard.network_revision(),
    };
    e.guard.set(EgressVerdict::Blocked(obs("CN"))).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")], 0);
    assert!(matches!(
        e.svc
            .submit_with_permit(plan, Some(stale))
            .await
            .unwrap_err(),
        SubmitError::Admission(AdmissionError::EgressBlocked { .. })
    ));
    assert_eq!(e.mock.count(), 0);

    // revision 变化的 permit 同样失效
    let wrong_rev = EgressPermit::fresh(e.guard.network_revision() + 1);
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")], 0);
    assert!(e
        .svc
        .submit_with_permit(plan, Some(wrong_rev))
        .await
        .is_err());
    assert!(
        e.guard.checks.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "新动作新检测"
    );
    e.mock.handle.abort();
}

/// A09:直接服务 retry 不能绕过门禁(Anthropic 归属)。
#[tokio::test]
async fn a09_service_retry_cannot_bypass() {
    // 先用 US 放行制造一条已失败的 Anthropic generation
    let e = env(
        EgressVerdict::Allowed(obs("US")),
        common::http_err(500, r#"{"error":{"message":"boom"}}"#),
    )
    .await;
    let conv = ConversationId::new();
    e.db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")], 0);
    e.svc.submit(plan).await.unwrap();
    assert!(
        common::wait_for(
            || e.db
                .generations_of(conv)
                .unwrap()
                .first()
                .map(|g| g.status.is_terminal())
                .unwrap_or(false),
            4000
        )
        .await,
        "首请求应到达终态"
    );
    // 出口翻转 CN 后直接 retry
    e.guard.set(EgressVerdict::Blocked(obs("CN"))).await;
    let original = e.db.generations_of(conv).unwrap().remove(0);
    let err = e
        .svc
        .retry(conv, &original, "你好", vec![])
        .await
        .unwrap_err();
    assert!(matches!(
        &err,
        SubmitError::Admission(AdmissionError::EgressBlocked { .. })
    ));
    // 未新增任何 generation/POST
    assert_eq!(e.db.generations_of(conv).unwrap().len(), 1);
    let before = e.mock.count();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(e.mock.count(), before, "retry 不产生 POST");
    e.mock.handle.abort();
}

/// A08(dispatch 级):排队超许可有效期后出口 US→CN:
/// dispatch 重检拒绝未发请求;状态 Failed/NotDispatched,费用不 pending;
/// 已发其他请求保持原结果。
#[tokio::test]
async fn a08_dispatch_level_recheck_rejects_after_queue_expiry() {
    let db = Arc::new(open_in_memory().unwrap());
    let first_slow = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let mock = common::spawn_counting_mock(move |_b, _h| {
        // 第一个连接延迟 4s(占用 semaphore 超过许可 TTL=3s)
        if first_slow.swap(false, std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_secs(4));
        }
        common::sse_ok(&format!("{}data: [DONE]\n\n", common::finish_stop("g")))
    })
    .await;
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        format!("http://127.0.0.1:{}/v1", mock.port),
        "test-key".into(),
    ));
    let (tx, _rx) = broadcast::channel(64);
    let guard = Arc::new(FlipGuard::new(EgressVerdict::Allowed(obs("US"))));
    let svc = ChatService::new(provider, db.clone(), tx, 1).with_egress_guard(guard.clone());
    let conv = ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();

    // req1:占用唯一并发位(慢响应)
    let plan1 = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")], 0);
    svc.submit(plan1).await.unwrap();
    // req2:排队(admission 在排队前,permit 新鲜);
    // 排队期间出口翻转 CN
    let plan2 = make_plan(
        conv,
        vec![anthropic_target("anthropic/claude-fable-5.1")],
        0,
    );
    svc.submit(plan2).await.unwrap();
    guard.set(EgressVerdict::Blocked(obs("CN"))).await;

    // 等待两个终态
    assert!(
        common::wait_for(
            || {
                let gs = db.generations_of(conv).unwrap();
                gs.len() == 2 && gs.iter().all(|g| g.status.is_terminal())
            },
            12000
        )
        .await,
        "两请求都应到终态"
    );
    let gs = db.generations_of(conv).unwrap();
    let rejected = gs
        .iter()
        .find(|g| g.model_id == "anthropic/claude-fable-5.1")
        .expect("req2 已落库");
    assert_eq!(rejected.status, aicouncil::domain::GenerationStatus::Failed);
    let failure = rejected.failure.as_ref().expect("结构化失败信息");
    assert_eq!(
        failure.kind(),
        aicouncil::domain::FailureKind::EgressBlocked,
        "{failure:?}"
    );
    assert_eq!(
        failure.phase(),
        aicouncil::domain::DispatchPhase::NotDispatched,
        "未调用生成接口"
    );
    assert_eq!(
        rejected.dispatch_phase,
        aicouncil::domain::DispatchPhase::NotDispatched
    );
    // 费用不留 pending
    let usage = db.usage_of(conv).unwrap();
    let rej_usage = usage
        .iter()
        .find(|u| u.request_id == rejected.request_id)
        .unwrap();
    assert_eq!(
        rej_usage.status,
        aicouncil::domain::UsageStatus::Unavailable,
        "本地未发送不留费用查询中"
    );
    // 已发的 req1 保持原结果(Completed)
    let ok = gs
        .iter()
        .find(|g| g.model_id == "anthropic/claude-opus-5.5")
        .unwrap();
    assert_eq!(ok.status, aicouncil::domain::GenerationStatus::Completed);
    // POST:只有 req1(慢响应);req2 被拒未发
    assert_eq!(mock.count(), 1, "被拒请求零 POST");
    mock.handle.abort();
}
