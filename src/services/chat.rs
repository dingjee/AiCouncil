//! 发送调度服务(P2.4)。
//!
//! - admission:Anthropic 全模型出口门禁(submit/retry/dispatch 三层,
//!   直接服务调用与排队后出口变化都被覆盖);预检拒绝零落库零 POST。
//! - 单事务保存 batch/user/assistant/generation/usage/snapshot +
//!   intended main_leaf(CAS);保存成功才发请求。
//! - 流式文本节流落库;终态 CAS 只落一次;usage/费用事件允许迟到。
//! - 已发出的生成不自动重试;EOF 无成功终态判失败而非保守成功。
//! - retry:同 user 下 assistant 兄弟;lineage 唯一活动约束(索引兜底)
//!   + 冷却 + 最新持久数据计算资格(连点只产生一个有效尝试)。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, Semaphore};

use crate::domain::plan::{RequestSnapshot, SendPlan};
use crate::domain::recovery::{
    apply_backoff, retry_decision, DispatchPhase, FailureInfo, FailureKind, Remedy, RetryDecision,
};
use crate::domain::{
    is_anthropic, BatchId, ConversationId, Generation, GenerationId, GenerationStatus, Message,
    MessageId, UsageRecord,
};
use crate::providers::cancel::CancellationToken;
use crate::providers::{GenEvent, ImmutableRequest, Provider, ProviderError};
use crate::services::egress::{EgressGuard, EgressPermit, EgressVerdict, GeoIpGuard};
use crate::storage::{CasExpectation, Db, SendBatchRecord, StorageError};

/// 发给 UI 的事件(带定位四元组)。
pub type ChatEvent = crate::providers::LocatedEvent;

/// 调度可注入的时钟/节流参数(测试可调小)。
#[derive(Clone, Copy)]
pub struct Throttle {
    pub flush_interval: Duration,
    pub flush_bytes: usize,
}

impl Default for Throttle {
    fn default() -> Self {
        Self {
            flush_interval: Duration::from_millis(300),
            flush_bytes: 4 * 1024,
        }
    }
}

/// admission 拒绝(submit/retry 共用;不伪装成 StorageError 或 Nop)。
#[derive(Debug, Clone)]
pub enum AdmissionError {
    /// 出口门禁拦截(明确 CN/HK)。
    EgressBlocked {
        country: Option<String>,
        detail: String,
    },
    /// 出口状态无法确认。
    EgressUnknown { detail: String },
    /// 同 lineage 已有活动尝试/原请求未终态。
    Duplicate { detail: String },
    /// 冷却中(绝对 epoch 毫秒)。
    Cooldown { until_ms: i64 },
    /// 需要先补救(凭据/余额/输入/模型/换 IP)。
    NeedsAction {
        remedy: &'static str,
        detail: String,
    },
    /// 分支状态不满足。
    InvalidBranch { detail: String },
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EgressBlocked { country, detail } => {
                write!(f, "出口受限(国家 {country:?}):{detail}")
            }
            Self::EgressUnknown { detail } => write!(f, "无法确认出口:{detail}"),
            Self::Duplicate { detail } => write!(f, "已有进行中的尝试:{detail}"),
            Self::Cooldown { until_ms } => write!(f, "重试冷却中(至 {until_ms})"),
            Self::NeedsAction { remedy, detail } => write!(f, "需要先处理({remedy}):{detail}"),
            Self::InvalidBranch { detail } => write!(f, "分支不可用:{detail}"),
        }
    }
}

/// submit/retry 错误。
#[derive(Debug)]
pub enum SubmitError {
    Admission(AdmissionError),
    Storage(StorageError),
}

impl std::fmt::Display for SubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission(e) => write!(f, "{e}"),
            Self::Storage(e) => write!(f, "{e}"),
        }
    }
}

impl From<StorageError> for SubmitError {
    fn from(e: StorageError) -> Self {
        Self::Storage(e)
    }
}

/// 发送调度器。泛型 P 以便注入 mock 提供方。
pub struct ChatService<P: Provider> {
    provider: Arc<P>,
    db: Arc<Db>,
    semaphore: Arc<Semaphore>,
    throttle: Throttle,
    cancels: Arc<tokio::sync::Mutex<HashMap<GenerationId, CancellationToken>>>,
    usage_repairs: Arc<tokio::sync::Mutex<HashSet<GenerationId>>>,
    events: broadcast::Sender<ChatEvent>,
    system_prompt: Option<String>,
    guard: Arc<dyn EgressGuard>,
}

pub const DEFAULT_CONCURRENCY: usize = 4;

/// submit/retry 返回的回执:UI 依此对账内存占位与主分支指向。
#[derive(Debug, Clone)]
pub struct SubmitReceipt {
    pub conversation_id: ConversationId,
    pub batch_id: BatchId,
    /// 主请求(完成后 main_leaf 指向其 assistant)。
    pub primary_request_id: GenerationId,
    /// 请求 -> assistant 占位消息。
    pub assistant_of: HashMap<GenerationId, MessageId>,
    /// 回执主张的 main_leaf(UI 用 DB 权威状态对账,不直接覆盖)。
    pub adopted_main_leaf: Option<MessageId>,
}

impl<P: Provider + Send + Sync + 'static> ChatService<P> {
    pub fn new(
        provider: Arc<P>,
        db: Arc<Db>,
        events: broadcast::Sender<ChatEvent>,
        max_concurrent: usize,
    ) -> Self {
        Self {
            provider,
            db,
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            throttle: Throttle::default(),
            cancels: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            usage_repairs: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            events,
            system_prompt: None,
            // 生产默认 GeoIP 门禁;无"关闭地域检查"的通用开关
            guard: Arc::new(GeoIpGuard::default()),
        }
    }

    /// 注入门禁(测试注入 mock;生产仅换实现,不降级策略)。
    pub fn with_egress_guard(mut self, guard: Arc<dyn EgressGuard>) -> Self {
        self.guard = guard;
        self
    }

    /// 共享门禁实例(UI 预检与服务校验同一 revision 语义)。
    pub fn egress_guard(&self) -> Arc<dyn EgressGuard> {
        self.guard.clone()
    }

    pub fn with_throttle(mut self, t: Throttle) -> Self {
        self.throttle = t;
        self
    }

    pub fn set_system_prompt(&mut self, sp: Option<String>) {
        self.system_prompt = sp;
    }

    /// Repair terminal pending usage records without replaying chat generations.
    /// Call this at startup and after switching conversations; each call is bounded.
    pub async fn repair_pending_usage(&self) -> Result<usize, StorageError> {
        let db = self.db.clone();
        let candidates = tokio::task::spawn_blocking(move || db.terminal_pending_usage(20))
            .await
            .map_err(|e| StorageError::Migration(e.to_string()))??;
        let mut known = 0;
        for mut rec in candidates {
            let Some(remote_id) = rec.remote_generation_id.clone() else {
                rec.mark_unavailable(chrono::Utc::now());
                db_usage(&self.db, &rec).await;
                continue;
            };
            if !self.usage_repairs.lock().await.insert(rec.request_id) {
                continue;
            }
            let request_id = rec.request_id;
            let result = query_and_record_usage(
                self.provider.clone(),
                self.db.clone(),
                rec,
                remote_id,
                None,
            )
            .await;
            self.usage_repairs.lock().await.remove(&request_id);
            if result.0 {
                known += 1;
            }
        }
        Ok(known)
    }

    /// admission:目标含 Anthropic 模型时校验出口许可。
    /// 返回有效许可(None = 目标不含 Anthropic,无需门禁)。
    async fn admission(
        &self,
        models: &[String],
        permit: Option<EgressPermit>,
    ) -> Result<Option<EgressPermit>, AdmissionError> {
        if !models.iter().any(|m| is_anthropic(m)) {
            return Ok(None); // A05:纯非 Anthropic 无 GeoIP 依赖
        }
        if let Some(p) = permit {
            if p.still_valid(self.guard.network_revision()) {
                return Ok(Some(p));
            }
        }
        // 每个动作新检测;不读持久缓存,不复用历史 Allowed(A12)
        let cancel = CancellationToken::new();
        match self.guard.check(cancel).await {
            EgressVerdict::Allowed(_) => {
                Ok(Some(EgressPermit::fresh(self.guard.network_revision())))
            }
            EgressVerdict::Blocked(o) => Err(AdmissionError::EgressBlocked {
                country: o.country_code,
                detail: "当前网络出口位于受限地区(中国大陆/香港),Anthropic 模型请求被拦截".into(),
            }),
            EgressVerdict::Unknown(r) => Err(AdmissionError::EgressUnknown {
                detail: format!("无法确认当前网络出口:{r}"),
            }),
        }
    }

    /// 提交计划:admission → 单事务落库 → 派发。预检拒绝零落库零 POST。
    pub async fn submit(&self, plan: SendPlan) -> Result<SubmitReceipt, SubmitError> {
        self.submit_with_permit(plan, None).await
    }

    /// UI 预检后的提交:permit 由同 revision 近期检测签发;
    /// 服务侧仍验证有效性(直接 submit 走完整 admission)。
    pub async fn submit_with_permit(
        &self,
        plan: SendPlan,
        permit: Option<EgressPermit>,
    ) -> Result<SubmitReceipt, SubmitError> {
        self.submit_with_permit_replacing(plan, permit, None, None)
            .await
    }

    /// 提交(含“替换旧轮回答”与草稿消费)。
    ///
    /// `draft_consumption` 与批次落在同一事务:草稿版本仍等于冻结值时
    /// 才清空;用户提交后又输入的新内容被保留(迟到回执不抹草稿)。
    #[allow(clippy::too_many_arguments)]
    pub async fn submit_with_permit_replacing(
        &self,
        plan: SendPlan,
        permit: Option<EgressPermit>,
        replacement: Option<(MessageId, Option<MessageId>)>,
        draft_consumption: Option<crate::storage::DraftConsumptionRequest>,
    ) -> Result<SubmitReceipt, SubmitError> {
        let models: Vec<String> = plan
            .branches
            .iter()
            .flat_map(|b| b.requests.iter().map(|r| r.model_id.clone()))
            .collect();
        let permit = self
            .admission(&models, permit)
            .await
            .map_err(SubmitError::Admission)?;

        let batch_id = BatchId::new();
        let now = chrono::Utc::now();

        let mut users = Vec::new();
        let mut assistants = Vec::new();
        let mut generations = Vec::new();
        let mut usage = Vec::new();

        for b in &plan.branches {
            users.push(Message {
                id: b.user_message_id,
                conversation_id: plan.conversation_id,
                parent_id: b.parent_leaf,
                role: crate::domain::Role::User,
                content: plan.prompt.clone(),
                attachments: plan.attachments.clone(),
                created_at: now,
            });
            for req in &b.requests {
                let assistant_id = MessageId::new();
                assistants.push(Message {
                    id: assistant_id,
                    conversation_id: plan.conversation_id,
                    parent_id: Some(b.user_message_id),
                    role: crate::domain::Role::Assistant,
                    content: String::new(),
                    attachments: Vec::new(),
                    created_at: now,
                });
                // 完整可重放快照(身份字段与内容字段分离;不含凭据)
                let snapshot = RequestSnapshot {
                    provider_id: self.provider.id().as_str().to_string(),
                    model_id: req.model_id.clone(),
                    params: req.params.clone(),
                    system_prompt: plan.system_prompt.clone(),
                    history: req.history.clone(),
                    prompt: crate::domain::compose_prompt(&plan.prompt, &plan.attachments),
                    context_ids: req.context_ids.iter().map(|i| i.as_str()).collect(),
                };
                generations.push(Generation {
                    request_id: req.request_id,
                    batch_id,
                    user_message_id: b.user_message_id,
                    assistant_message_id: assistant_id,
                    provider_id: self.provider.id(),
                    model_id: req.model_id.clone(),
                    status: GenerationStatus::Queued,
                    params: req.params.clone(),
                    system_prompt: plan.system_prompt.clone(),
                    context_snapshot: req.context_ids.clone(),
                    price_snapshot: req.price_snapshot,
                    remote_generation_id: None,
                    error: None,
                    retry_of: None,
                    retry_root_id: Some(req.request_id),
                    failure: None,
                    request_snapshot: Some(snapshot),
                    retry_not_before_ms: None,
                    dispatch_phase: DispatchPhase::NotDispatched,
                    started_at: None,
                    finished_at: None,
                });
                usage.push(UsageRecord::pending(req.request_id));
            }
        }

        let primary_req = &plan.branches[plan.primary.0].requests[plan.primary.1];
        // request_id -> assistant 占位映射(派发时定位)
        let assistant_of: HashMap<GenerationId, MessageId> = generations
            .iter()
            .map(|g| (g.request_id, g.assistant_message_id))
            .collect();

        let rec_primary = primary_req.request_id;
        let primary_assistant = assistant_of[&rec_primary];
        let expected_leaf = plan.branches[plan.primary.0].parent_leaf;
        let rec = SendBatchRecord {
            batch_id,
            conversation_id: plan.conversation_id,
            primary_request_id: rec_primary,
            user_messages: users,
            assistant_messages: assistants,
            generations,
            usage,
            // 事务内一并保存 intended main_leaf(CAS;用户后续选择不被覆盖)
            intended_main_leaf: Some((expected_leaf, primary_assistant)),
            replacement,
            // 事务内一并消费草稿(按冻结版本 CAS;版本已变则保留新内容)
            draft_consumption,
        };
        // spawn_blocking:rusqlite 连接非 async
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || db.save_send_batch(&rec))
            .await
            .map_err(|e| SubmitError::Storage(StorageError::Migration(e.to_string())))??;

        // 落库成功,派发请求
        for (bi, b) in plan.branches.iter().enumerate() {
            for (ri, req) in b.requests.iter().enumerate() {
                let assistant_id = assistant_of[&req.request_id];
                let immutable = ImmutableRequest::from_planned(
                    req,
                    plan.conversation_id,
                    batch_id,
                    assistant_id,
                    &plan.prompt,
                    &plan.attachments,
                    plan.system_prompt.clone(),
                );
                self.dispatch(
                    plan.conversation_id,
                    batch_id,
                    req.request_id,
                    assistant_id,
                    b.user_message_id,
                    immutable,
                    (bi, ri) == plan.primary,
                    permit,
                );
            }
        }
        Ok(SubmitReceipt {
            conversation_id: plan.conversation_id,
            batch_id,
            primary_request_id: rec_primary,
            assistant_of,
            adopted_main_leaf: Some(primary_assistant),
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch(
        &self,
        conversation_id: ConversationId,
        batch_id: BatchId,
        request_id: GenerationId,
        assistant_message_id: MessageId,
        user_message_id: MessageId,
        immutable: ImmutableRequest,
        is_primary: bool,
        permit: Option<EgressPermit>,
    ) {
        let provider = self.provider.clone();
        let db = self.db.clone();
        let sem = self.semaphore.clone();
        let events = self.events.clone();
        let cancels = self.cancels.clone();
        let usage_repairs = self.usage_repairs.clone();
        let throttle = self.throttle;
        let guard = self.guard.clone();
        let anthropic = is_anthropic(&immutable.model);

        tokio::spawn(async move {
            // 取消令牌先注册:submit 后立即 Stop 也能找到 token(B15)
            let cancel = CancellationToken::new();
            cancels.lock().await.insert(request_id, cancel.clone());

            // 排队可取消:等许可期间也能取消
            let slot = tokio::select! {
                p = sem.acquire() => p.map_err(|_| ()),
                _ = cancel.cancelled() => Err(()),
            };
            if slot.is_err() {
                finish(
                    &db,
                    &events,
                    conversation_id,
                    batch_id,
                    request_id,
                    assistant_message_id,
                    GenerationStatus::Cancelled,
                    &FailureInfo::new(
                        FailureKind::Cancelled,
                        "排队中取消",
                        DispatchPhase::NotDispatched,
                    ),
                )
                .await;
                // 排队取消未发出:不留"费用查询中"
                {
                    let dbc = db.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        dbc.mark_usage_unavailable(request_id, "排队中取消")
                    })
                    .await;
                }
                emit(
                    &events,
                    conversation_id,
                    batch_id,
                    request_id,
                    assistant_message_id,
                    GenEvent::Cancelled,
                );
                cancels.lock().await.remove(&request_id);
                return;
            }
            let _slot = slot.unwrap();

            // dispatch 前门禁重检(A08:排队超许可有效期/网络 revision 变化)
            if anthropic {
                let stale = !matches!(&permit, Some(p) if p.still_valid(guard.network_revision()));
                if stale {
                    match guard.check(cancel.clone()).await {
                        EgressVerdict::Allowed(_) => {}
                        EgressVerdict::Blocked(o) => {
                            reject_late(
                                &db,
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                FailureKind::EgressBlocked,
                                &format!(
                                    "出口受限(国家 {}):dispatch 前重检未通过,未发送",
                                    o.country_code.as_deref().unwrap_or("?")
                                ),
                            )
                            .await;
                            cancels.lock().await.remove(&request_id);
                            return;
                        }
                        EgressVerdict::Unknown(r) => {
                            reject_late(
                                &db,
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                FailureKind::EgressUnknown,
                                &format!("无法确认出口({r}):dispatch 前重检,未发送"),
                            )
                            .await;
                            cancels.lock().await.remove(&request_id);
                            return;
                        }
                    }
                }
            }

            // 进入 .send() 前记录 DispatchAttempted(断在这里也可能到达服务器)
            {
                let dbp = db.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    dbp.set_dispatch_phase(request_id, DispatchPhase::DispatchAttempted)
                })
                .await;
            }

            let mut stream = provider.stream(immutable, cancel);
            use futures_util::StreamExt;
            let mut buffered = String::new();
            let mut unflushed = 0usize;
            let mut last_flush = tokio::time::Instant::now();
            let mut final_state_written = false;
            let mut remote_accepted = false;
            let mut remote_generation_id: Option<String> = None;

            while let Some(item) = stream.next().await {
                let event = match item {
                    Ok(e) => e,
                    Err(e) => {
                        // 部分内容先落库;取消与失败分别落终态
                        if !buffered.is_empty() {
                            db_content(&db, assistant_message_id, &buffered).await;
                        }
                        final_state_written = true;
                        if matches!(e, ProviderError::Cancelled) {
                            finish(
                                &db,
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                GenerationStatus::Cancelled,
                                &FailureInfo::new(
                                    FailureKind::Cancelled,
                                    "用户停止",
                                    phase_of(remote_accepted),
                                ),
                            )
                            .await;
                            emit(
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                GenEvent::Cancelled,
                            );
                        } else {
                            fail_with_error(
                                &db,
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                &e,
                                remote_accepted,
                            )
                            .await;
                            emit(
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                GenEvent::Failed { error: e },
                            );
                        }
                        break;
                    }
                };
                match event {
                    GenEvent::Started => {
                        db_status(&db, request_id, GenerationStatus::Connecting).await;
                    }
                    GenEvent::RemoteGenerationId {
                        remote_generation_id: id,
                    } => {
                        if remote_generation_id.is_none() {
                            remote_generation_id = Some(id.clone());
                            let dbp = db.clone();
                            let persist_id = id.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                dbp.record_remote_generation_id(request_id, &persist_id)
                            })
                            .await;
                        }
                        remote_accepted = true;
                        mark_remote_accepted(&db, request_id);
                        emit(
                            &events,
                            conversation_id,
                            batch_id,
                            request_id,
                            assistant_message_id,
                            GenEvent::RemoteGenerationId {
                                remote_generation_id: id,
                            },
                        );
                    }
                    GenEvent::TextDelta(text) => {
                        if !remote_accepted {
                            remote_accepted = true;
                            mark_remote_accepted(&db, request_id);
                        }
                        buffered.push_str(&text);
                        unflushed += text.len();
                        if unflushed >= throttle.flush_bytes
                            || last_flush.elapsed() >= throttle.flush_interval
                        {
                            db_content(&db, assistant_message_id, &buffered).await;
                            unflushed = 0;
                            last_flush = tokio::time::Instant::now();
                        }
                        emit(
                            &events,
                            conversation_id,
                            batch_id,
                            request_id,
                            assistant_message_id,
                            GenEvent::TextDelta(text),
                        );
                    }
                    GenEvent::UsageUpdated {
                        prompt_tokens,
                        completion_tokens,
                        cost,
                    } => {
                        // 允许在终态后到达;入账幂等;同时转发 UI(费用标签更新)
                        let mut rec = UsageRecord::pending(request_id);
                        rec.merge_known(
                            prompt_tokens,
                            completion_tokens,
                            cost,
                            crate::domain::CostSource::ApiReported,
                            None,
                            chrono::Utc::now(),
                        );
                        db_usage(&db, &rec).await;
                        emit(
                            &events,
                            conversation_id,
                            batch_id,
                            request_id,
                            assistant_message_id,
                            GenEvent::UsageUpdated {
                                prompt_tokens,
                                completion_tokens,
                                cost,
                            },
                        );
                    }
                    GenEvent::Finished {
                        remote_generation_id: remote_id,
                        finish_state,
                    } => {
                        let final_remote_id = remote_id.or_else(|| remote_generation_id.clone());
                        if let Some(id) = final_remote_id.as_deref() {
                            let dbp = db.clone();
                            let id = id.to_string();
                            let _ = tokio::task::spawn_blocking(move || {
                                dbp.record_remote_generation_id(request_id, &id)
                            })
                            .await;
                        }
                        if !remote_accepted {
                            remote_accepted = true;
                            mark_remote_accepted(&db, request_id);
                        }
                        if !final_state_written {
                            if !buffered.is_empty() {
                                db_content(&db, assistant_message_id, &buffered).await;
                            }
                            // 非停止原因(length/content_filter)保留可用正文:
                            // Completed + 结构化截断信息(空内容时给补救建议)
                            let failure = if finish_state != crate::providers::FinishState::Stop {
                                Some(FailureInfo::new(
                                    FailureKind::Truncated,
                                    format!("输出结束原因:{finish_state:?}(正文保留)"),
                                    DispatchPhase::RemoteAccepted,
                                ))
                            } else {
                                None
                            };
                            finish_ex(
                                &db,
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                GenerationStatus::Completed,
                                failure,
                                final_remote_id.as_deref(),
                            )
                            .await;
                            final_state_written = true;
                        }
                        // 主请求完成:leaf 只在仍位于本轮(user/自身)时移动
                        if is_primary {
                            let dbp = db.clone();
                            let expected = [user_message_id, assistant_message_id];
                            let _ = tokio::task::spawn_blocking(move || {
                                dbp.cas_main_leaf(
                                    conversation_id,
                                    CasExpectation::AnyOf(&expected),
                                    assistant_message_id,
                                )
                            })
                            .await;
                        }
                        emit(
                            &events,
                            conversation_id,
                            batch_id,
                            request_id,
                            assistant_message_id,
                            GenEvent::Finished {
                                remote_generation_id: final_remote_id.clone(),
                                finish_state,
                            },
                        );
                    }
                    GenEvent::Failed { error } => {
                        if !final_state_written {
                            if !buffered.is_empty() {
                                db_content(&db, assistant_message_id, &buffered).await;
                            }
                            fail_with_error(
                                &db,
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                &error,
                                remote_accepted,
                            )
                            .await;
                            final_state_written = true;
                        }
                        emit(
                            &events,
                            conversation_id,
                            batch_id,
                            request_id,
                            assistant_message_id,
                            GenEvent::Failed { error },
                        );
                    }
                    GenEvent::Cancelled => {
                        if !final_state_written {
                            if !buffered.is_empty() {
                                db_content(&db, assistant_message_id, &buffered).await;
                            }
                            finish(
                                &db,
                                &events,
                                conversation_id,
                                batch_id,
                                request_id,
                                assistant_message_id,
                                GenerationStatus::Cancelled,
                                &FailureInfo::new(
                                    FailureKind::Cancelled,
                                    "用户停止",
                                    phase_of(remote_accepted),
                                ),
                            )
                            .await;
                            final_state_written = true;
                        }
                        emit(
                            &events,
                            conversation_id,
                            batch_id,
                            request_id,
                            assistant_message_id,
                            GenEvent::Cancelled,
                        );
                    }
                    GenEvent::Rejected { detail } => {
                        emit(
                            &events,
                            conversation_id,
                            batch_id,
                            request_id,
                            assistant_message_id,
                            GenEvent::Rejected { detail },
                        );
                    }
                    GenEvent::PersistError { error } => {
                        emit(
                            &events,
                            conversation_id,
                            batch_id,
                            request_id,
                            assistant_message_id,
                            GenEvent::PersistError { error },
                        );
                    }
                }
            }
            if !final_state_written {
                // 流自然结束但无成功终态:不再保守标 Completed(C02/C03)。
                // 部分内容保留并失败(部分中断,不当完整回答)。
                if !buffered.is_empty() {
                    db_content(&db, assistant_message_id, &buffered).await;
                }
                let info = FailureInfo::new(
                    FailureKind::StreamAborted,
                    "流在成功终态前结束",
                    phase_of(remote_accepted),
                );
                finish(
                    &db,
                    &events,
                    conversation_id,
                    batch_id,
                    request_id,
                    assistant_message_id,
                    GenerationStatus::Failed,
                    &info,
                )
                .await;
                emit(
                    &events,
                    conversation_id,
                    batch_id,
                    request_id,
                    assistant_message_id,
                    GenEvent::Failed {
                        error: ProviderError::StreamAborted("流在成功终态前结束".into()),
                    },
                );
            }
            settle_usage(
                &provider,
                &db,
                &usage_repairs,
                &events,
                conversation_id,
                batch_id,
                assistant_message_id,
                request_id,
                remote_generation_id.clone(),
            )
            .await;
            cancels.lock().await.remove(&request_id);
        });
    }

    /// 取消单个请求(排队或流式中);不影响其他请求。
    pub async fn cancel(&self, request_id: GenerationId) {
        if let Some(token) = self.cancels.lock().await.get(&request_id) {
            token.cancel();
        }
    }

    /// 原样重试:同 user 节点下新 assistant 兄弟,关联 retry_of,
    /// lineage 沿用原请求 root;资格用最新持久数据计算。
    /// 默认沿用原快照(模型/参数/上下文);缺旧快照时用调用方重建的
    /// history/prompt(按现有历史重新生成,不声称原样重试)。
    pub async fn retry(
        &self,
        conversation_id: ConversationId,
        original: &Generation,
        prompt: &str,
        history: Vec<crate::domain::plan::ContextMessage>,
    ) -> Result<SubmitReceipt, SubmitError> {
        // 门禁:A09 直接服务 retry 不能绕过
        let permit = self
            .admission(std::slice::from_ref(&original.model_id), None)
            .await
            .map_err(SubmitError::Admission)?;

        // ---- 资格检查(最新持久数据;B07/B10) ----
        if !original.status.is_terminal() {
            return Err(SubmitError::Admission(AdmissionError::Duplicate {
                detail: "原请求仍在进行中".into(),
            }));
        }
        let root = original.retry_root_id.unwrap_or(original.request_id);
        if self.db.lineage_has_active(root).unwrap_or(true) {
            return Err(SubmitError::Admission(AdmissionError::Duplicate {
                detail: "该请求 lineage 已有活动尝试".into(),
            }));
        }
        if let Some(f) = self.db.lineage_latest_failure(root).unwrap_or(None) {
            match retry_decision(
                original.status,
                Some(&f),
                crate::domain::recovery::now_ms(),
                false,
            ) {
                RetryDecision::Cooldown(until) => {
                    return Err(SubmitError::Admission(AdmissionError::Cooldown {
                        until_ms: until,
                    }))
                }
                RetryDecision::NeedsAction(remedy) => {
                    return Err(SubmitError::Admission(AdmissionError::NeedsAction {
                        remedy: match remedy {
                            Remedy::SwitchIp => "switch_ip",
                            Remedy::Credentials => "credentials",
                            Remedy::Balance => "balance",
                            Remedy::EditInput => "edit_input",
                            Remedy::PickModel => "pick_model",
                            Remedy::None => "none",
                        },
                        detail: f.message.clone(),
                    }))
                }
                RetryDecision::InFlight => {
                    return Err(SubmitError::Admission(AdmissionError::Duplicate {
                        detail: "已有活动尝试".into(),
                    }))
                }
                RetryDecision::Allowed => {}
            }
        }

        // ---- 记录构建(原快照优先) ----
        let (model, params, system, hist, prompt_text) = match &original.request_snapshot {
            Some(s) => (
                s.model_id.clone(),
                s.params.clone(),
                s.system_prompt.clone(),
                s.history.clone(),
                s.prompt.clone(),
            ),
            None => (
                // 旧数据无快照:按现有历史重新生成(不声称原样)
                original.model_id.clone(),
                original.params.clone(),
                original.system_prompt.clone(),
                history,
                prompt.to_string(),
            ),
        };

        let new_request = GenerationId::new();
        let sibling = MessageId::new();
        let batch = BatchId::new();
        let now = chrono::Utc::now();

        let snapshot = RequestSnapshot {
            provider_id: original.provider_id.as_str().to_string(),
            model_id: model.clone(),
            params: params.clone(),
            system_prompt: system.clone(),
            history: hist.clone(),
            prompt: prompt_text.clone(),
            context_ids: original
                .context_snapshot
                .iter()
                .map(|i| i.as_str())
                .collect(),
        };
        let rec = SendBatchRecord {
            batch_id: batch,
            conversation_id,
            primary_request_id: new_request,
            user_messages: vec![],
            assistant_messages: vec![Message {
                id: sibling,
                conversation_id,
                parent_id: Some(original.user_message_id),
                role: crate::domain::Role::Assistant,
                content: String::new(),
                attachments: Vec::new(),
                created_at: now,
            }],
            generations: vec![Generation {
                request_id: new_request,
                batch_id: batch,
                user_message_id: original.user_message_id,
                assistant_message_id: sibling,
                provider_id: original.provider_id,
                model_id: model.clone(),
                status: GenerationStatus::Queued,
                params: params.clone(),
                system_prompt: system.clone(),
                context_snapshot: original.context_snapshot.clone(),
                price_snapshot: original.price_snapshot,
                remote_generation_id: None,
                error: None,
                retry_of: Some(original.request_id),
                retry_root_id: Some(root),
                failure: None,
                request_snapshot: Some(snapshot),
                retry_not_before_ms: None,
                dispatch_phase: DispatchPhase::NotDispatched,
                started_at: None,
                finished_at: None,
            }],
            usage: vec![UsageRecord::pending(new_request)],
            intended_main_leaf: None,
            replacement: None,
            // 重试不消费 composer 草稿(草稿属于新一轮输入)
            draft_consumption: None,
        };
        let db = self.db.clone();
        let old_card = original.assistant_message_id;
        let insert = tokio::task::spawn_blocking(move || {
            db.save_send_batch(&rec)?;
            // retry 回执 CAS:leaf 从仍选中的旧卡改为新尝试;
            // 当前 leaf 已是后续轮次/别的分支时不移动(B13)
            db.cas_main_leaf(conversation_id, CasExpectation::AnyOf(&[old_card]), sibling)?;
            Ok::<(), StorageError>(())
        })
        .await
        .map_err(|e| SubmitError::Storage(StorageError::Migration(e.to_string())))?;
        // 并发连点兜底:lineage 唯一活动索引冲突 → Duplicate(不是存储故障)
        if let Err(StorageError::Sqlite(rusqlite::Error::SqliteFailure(_, ref msg))) = insert {
            if msg
                .as_deref()
                .is_some_and(|m| m.contains("generations.retry_root_id"))
            {
                return Err(SubmitError::Admission(AdmissionError::Duplicate {
                    detail: "lineage 已有活动尝试(唯一约束)".into(),
                }));
            }
        }
        insert?;

        // 派发(单请求,沿用快照内容)
        let immutable = ImmutableRequest {
            conversation_id,
            batch_id: batch,
            request_id: new_request,
            assistant_message_id: sibling,
            model,
            params,
            system_prompt: system,
            history: hist,
            prompt: prompt_text,
        };
        let mut assistant_of = HashMap::new();
        assistant_of.insert(new_request, sibling);
        self.dispatch(
            conversation_id,
            batch,
            new_request,
            sibling,
            original.user_message_id,
            immutable,
            true,
            permit,
        );
        Ok(SubmitReceipt {
            conversation_id,
            batch_id: batch,
            primary_request_id: new_request,
            assistant_of,
            adopted_main_leaf: Some(sibling),
        })
    }
}

fn phase_of(remote_accepted: bool) -> DispatchPhase {
    if remote_accepted {
        DispatchPhase::RemoteAccepted
    } else {
        DispatchPhase::DispatchAttempted
    }
}

fn mark_remote_accepted(db: &Arc<Db>, request: GenerationId) {
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        db.set_dispatch_phase(request, DispatchPhase::RemoteAccepted)
    });
}

/// 排队取消/流内取消等已知 FailureInfo 的终态落库(CAS)。
#[allow(clippy::too_many_arguments)]
async fn finish(
    db: &Arc<Db>,
    events: &broadcast::Sender<ChatEvent>,
    conversation_id: ConversationId,
    batch_id: BatchId,
    request: GenerationId,
    assistant: MessageId,
    status: GenerationStatus,
    failure: &FailureInfo,
) {
    let error_text = status_summary(status, failure);
    let failure_json = failure.to_json();
    let retry_not_before = failure.retry_not_before_ms;
    let db = db.clone();
    let result = tokio::task::spawn_blocking(move || {
        db.finish_generation(
            request,
            status,
            Some(error_text).as_deref(),
            Some(failure_json).as_deref(),
            retry_not_before,
            None,
        )
    })
    .await
    .unwrap_or_else(|e| Err(StorageError::Migration(e.to_string())));
    if let Err(e) = result {
        persist_error(events, conversation_id, batch_id, request, assistant, e).await;
    }
}

/// ProviderError 失败:构造 FailureInfo(含退避/Retry-After)并落终态。
#[allow(clippy::too_many_arguments)]
async fn fail_with_error(
    db: &Arc<Db>,
    events: &broadcast::Sender<ChatEvent>,
    conversation_id: ConversationId,
    batch_id: BatchId,
    request: GenerationId,
    assistant: MessageId,
    error: &ProviderError,
    remote_accepted: bool,
) {
    let phase = phase_of(remote_accepted);
    let mut info = error.to_failure_info(phase);
    // 退避输入:lineage 已有尝试数(含本次;指数退避)
    let db1 = db.clone();
    let attempt = tokio::task::spawn_blocking(move || db1.lineage_attempt_count_of(request))
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten()
        .unwrap_or(1);
    apply_backoff(&mut info, attempt.max(1));
    finish(
        db,
        events,
        conversation_id,
        batch_id,
        request,
        assistant,
        GenerationStatus::Failed,
        &info,
    )
    .await;
}

/// 成功终态(可携带截断信息)。
#[allow(clippy::too_many_arguments)]
async fn finish_ex(
    db: &Arc<Db>,
    events: &broadcast::Sender<ChatEvent>,
    conversation_id: ConversationId,
    batch_id: BatchId,
    request: GenerationId,
    assistant: MessageId,
    status: GenerationStatus,
    failure: Option<FailureInfo>,
    remote_generation_id: Option<&str>,
) {
    let error_text = failure.as_ref().map(|f| f.message.clone());
    let failure_json = failure.as_ref().map(|f| f.to_json());
    let retry_not_before = failure.as_ref().and_then(|f| f.retry_not_before_ms);
    let remote = remote_generation_id.map(str::to_string);
    let db = db.clone();
    let result = tokio::task::spawn_blocking(move || {
        db.finish_generation(
            request,
            status,
            error_text.as_deref(),
            failure_json.as_deref(),
            retry_not_before,
            remote.as_deref(),
        )
    })
    .await
    .unwrap_or_else(|e| Err(StorageError::Migration(e.to_string())));
    if let Err(e) = result {
        persist_error(events, conversation_id, batch_id, request, assistant, e).await;
    }
}

/// dispatch 门禁晚到拒绝:Failed + NotDispatched;费用不留"查询中"。
#[allow(clippy::too_many_arguments)]
async fn reject_late(
    db: &Arc<Db>,
    events: &broadcast::Sender<ChatEvent>,
    conversation_id: ConversationId,
    batch_id: BatchId,
    request: GenerationId,
    assistant: MessageId,
    kind: FailureKind,
    detail: &str,
) {
    let info = FailureInfo::new(kind, detail, DispatchPhase::NotDispatched);
    finish(
        db,
        events,
        conversation_id,
        batch_id,
        request,
        assistant,
        GenerationStatus::Failed,
        &info,
    )
    .await;
    // 本地未发送:用量标 unavailable(不留 pending"费用查询中")
    let dbc = db.clone();
    let detail_owned = detail.to_string();
    let _ = tokio::task::spawn_blocking(move || dbc.mark_usage_unavailable(request, &detail_owned))
        .await;
    emit(
        events,
        conversation_id,
        batch_id,
        request,
        assistant,
        GenEvent::Rejected {
            detail: detail.to_string(),
        },
    );
}

fn status_summary(status: GenerationStatus, failure: &FailureInfo) -> String {
    let head = match status {
        GenerationStatus::Cancelled => "已取消",
        GenerationStatus::Completed => "已完成",
        _ => "失败",
    };
    format!("{head}:{}", failure.message)
}

async fn persist_error(
    events: &broadcast::Sender<ChatEvent>,
    conversation_id: ConversationId,
    batch_id: BatchId,
    request: GenerationId,
    assistant: MessageId,
    e: StorageError,
) {
    // 可见存储故障;不向 UI 宣称可靠保存成功(B18)
    tracing::error!(%e, %request, "终态落库失败");
    emit(
        events,
        conversation_id,
        batch_id,
        request,
        assistant,
        GenEvent::PersistError {
            error: e.to_string(),
        },
    );
}

fn emit(
    events: &broadcast::Sender<ChatEvent>,
    conversation_id: ConversationId,
    batch_id: BatchId,
    request_id: GenerationId,
    assistant_message_id: MessageId,
    event: GenEvent,
) {
    let _ = events.send(ChatEvent {
        conversation_id,
        batch_id,
        request_id,
        assistant_message_id,
        event,
    });
}

async fn db_status(db: &Arc<Db>, request: GenerationId, status: GenerationStatus) {
    let db = db.clone();
    let r = tokio::task::spawn_blocking(move || {
        db.update_generation_status(request, status, None, None)
    })
    .await
    .unwrap_or_else(|e| Err(StorageError::Migration(e.to_string())));
    if let Err(e) = r {
        tracing::error!(%e, %request, "状态推进落库失败");
    }
}

async fn db_content(db: &Arc<Db>, message: MessageId, content: &str) {
    let db = db.clone();
    let content = content.to_string();
    let r = tokio::task::spawn_blocking(move || db.update_message_content(message, &content)).await;
    if let Ok(Err(e)) = r {
        tracing::error!(%e, %message, "内容落库失败");
    }
}

async fn db_usage(db: &Arc<Db>, rec: &UsageRecord) {
    let db = db.clone();
    let rec = rec.clone();
    let r = tokio::task::spawn_blocking(move || db.upsert_usage(&rec)).await;
    if let Ok(Err(e)) = r {
        tracing::error!(%e, "用量入账失败");
    }
}

#[allow(clippy::too_many_arguments)]
async fn settle_usage<P: Provider + Send + Sync + 'static>(
    provider: &Arc<P>,
    db: &Arc<Db>,
    in_flight: &Arc<tokio::sync::Mutex<HashSet<GenerationId>>>,
    events: &broadcast::Sender<ChatEvent>,
    conversation: ConversationId,
    batch: BatchId,
    assistant: MessageId,
    request: GenerationId,
    remote_id: Option<String>,
) {
    let db_read = db.clone();
    let status = tokio::task::spawn_blocking(move || db_read.usage_record(request))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .map(|record| record.status);
    if !matches!(status, Some(crate::domain::UsageStatus::Pending)) {
        return;
    }
    if let Some(remote_id) = remote_id {
        schedule_usage_repair(
            provider,
            db,
            in_flight,
            Some((events.clone(), conversation, batch, assistant)),
            request,
            remote_id,
        );
    } else {
        let db_mark = db.clone();
        let _ = tokio::task::spawn_blocking(move || {
            db_mark.mark_usage_unavailable(request, "终态没有远端 generation id")
        })
        .await;
        let _ = events.send(ChatEvent {
            conversation_id: conversation,
            batch_id: batch,
            request_id: request,
            assistant_message_id: assistant,
            event: GenEvent::UsageUpdated {
                prompt_tokens: None,
                completion_tokens: None,
                cost: None,
            },
        });
    }
}

fn schedule_usage_repair<P: Provider + Send + Sync + 'static>(
    provider: &Arc<P>,
    db: &Arc<Db>,
    in_flight: &Arc<tokio::sync::Mutex<HashSet<GenerationId>>>,
    event_context: Option<(
        broadcast::Sender<ChatEvent>,
        ConversationId,
        BatchId,
        MessageId,
    )>,
    request: GenerationId,
    remote_id: String,
) {
    let provider = provider.clone();
    let db = db.clone();
    let in_flight = in_flight.clone();
    tokio::spawn(async move {
        if !in_flight.lock().await.insert(request) {
            return;
        }
        let rec = UsageRecord::pending(request);
        let _ = query_and_record_usage(provider, db, rec, remote_id, event_context).await;
        in_flight.lock().await.remove(&request);
    });
}

async fn query_and_record_usage<P: Provider + Send + Sync + 'static>(
    provider: Arc<P>,
    db: Arc<Db>,
    mut rec: UsageRecord,
    remote_id: String,
    event_context: Option<(
        broadcast::Sender<ChatEvent>,
        ConversationId,
        BatchId,
        MessageId,
    )>,
) -> (bool, GenerationId) {
    // Transient 404/401 and network failures stay pending. Short retries catch
    // generation metadata that becomes visible shortly after the SSE stream.
    let delays = [
        std::time::Duration::from_millis(250),
        std::time::Duration::from_millis(750),
        std::time::Duration::from_secs(2),
    ];
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut result = None;
    for attempt in 0..=delays.len() {
        let query = tokio::time::timeout_at(deadline, provider.query_usage(&remote_id)).await;
        match query {
            Err(_) => break,
            Ok(Ok(Some(usage))) if usage.cost.is_some() => {
                result = Some(usage);
                break;
            }
            Ok(Ok(Some(usage))) => result = Some(usage),
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                tracing::debug!(%error, request_id = %rec.request_id, "generation usage 补查稍后重试");
                if matches!(
                    error,
                    ProviderError::Unauthorized(_)
                        | ProviderError::Forbidden(_)
                        | ProviderError::PaymentRequired(_)
                        | ProviderError::RateLimited { .. }
                ) {
                    break;
                }
            }
        }
        if attempt < delays.len()
            && tokio::time::timeout_at(deadline, tokio::time::sleep(delays[attempt]))
                .await
                .is_err()
        {
            break;
        }
    }
    rec.merge_known(
        result.as_ref().and_then(|u| u.prompt_tokens),
        result.as_ref().and_then(|u| u.completion_tokens),
        result.as_ref().and_then(|u| u.cost),
        crate::domain::CostSource::GenerationQuery,
        Some(remote_id),
        chrono::Utc::now(),
    );
    if rec.cost.is_none() {
        rec.cost_source = None;
    }
    let known = rec.status == crate::domain::UsageStatus::Known;
    let request_id = rec.request_id;
    let prompt_tokens = rec.prompt_tokens;
    let completion_tokens = rec.completion_tokens;
    let cost = rec.cost;
    db_usage(&db, &rec).await;
    if let Some((events, conversation, batch, assistant)) = event_context {
        let db_read = db.clone();
        let persisted = tokio::task::spawn_blocking(move || db_read.usage_record(request_id))
            .await
            .ok()
            .and_then(Result::ok)
            .flatten();
        let prompt_tokens = persisted
            .as_ref()
            .and_then(|record| record.prompt_tokens)
            .or(prompt_tokens);
        let completion_tokens = persisted
            .as_ref()
            .and_then(|record| record.completion_tokens)
            .or(completion_tokens);
        let cost = persisted.and_then(|record| record.cost).or(cost);
        let _ = events.send(ChatEvent {
            conversation_id: conversation,
            batch_id: batch,
            request_id,
            assistant_message_id: assistant,
            event: GenEvent::UsageUpdated {
                prompt_tokens,
                completion_tokens,
                cost,
            },
        });
    }
    (known, request_id)
}

#[cfg(test)]
mod usage_repair_tests {
    use super::*;
    use crate::domain::Conversation;
    use crate::providers::{CatalogSnapshot, EventStream, KeyStatus, UsageResult};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DelayedCostProvider(AtomicUsize);

    impl Provider for DelayedCostProvider {
        fn id(&self) -> crate::domain::ProviderId {
            crate::domain::ProviderId::new()
        }

        async fn list_models(&self) -> Result<CatalogSnapshot, ProviderError> {
            Err(ProviderError::Protocol("unused".into()))
        }

        fn stream(
            &self,
            _req: ImmutableRequest,
            _cancel: CancellationToken,
        ) -> crate::providers::EventStream {
            Box::pin(futures_util::stream::empty()) as EventStream
        }

        async fn query_usage(
            &self,
            _remote_generation_id: &str,
        ) -> Result<Option<UsageResult>, ProviderError> {
            let attempt = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Some(UsageResult {
                prompt_tokens: Some(15),
                completion_tokens: Some(4),
                cost: (attempt > 0).then_some(rust_decimal::Decimal::new(491, 5)),
            }))
        }

        async fn query_key_status(&self) -> Result<KeyStatus, ProviderError> {
            Ok(KeyStatus::Unsupported)
        }

        async fn test_connection(&self) -> Result<(), ProviderError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn usage_query_retries_tokens_only_until_cost_arrives() {
        let db = Arc::new(crate::storage::open_in_memory().unwrap());
        let request = GenerationId::new();
        let conversation = ConversationId::new();
        let batch = BatchId::new();
        let user = MessageId::new();
        let assistant = MessageId::new();
        let provider_id = crate::domain::ProviderId::new();
        let now = chrono::Utc::now();
        let generation = Generation {
            request_id: request,
            batch_id: batch,
            user_message_id: user,
            assistant_message_id: assistant,
            provider_id,
            model_id: "test-model".into(),
            status: GenerationStatus::Completed,
            params: None,
            system_prompt: None,
            context_snapshot: Vec::new(),
            price_snapshot: None,
            remote_generation_id: Some("gen-delayed".into()),
            error: None,
            retry_of: None,
            retry_root_id: Some(request),
            failure: None,
            request_snapshot: None,
            retry_not_before_ms: None,
            dispatch_phase: DispatchPhase::RemoteAccepted,
            started_at: Some(now),
            finished_at: Some(now),
        };
        db.upsert_conversation(&Conversation::new(conversation, now))
            .unwrap();
        db.save_send_batch(&SendBatchRecord {
            batch_id: batch,
            conversation_id: conversation,
            primary_request_id: request,
            user_messages: vec![Message {
                id: user,
                conversation_id: conversation,
                parent_id: None,
                role: crate::domain::Role::User,
                content: "question".into(),
                attachments: Vec::new(),
                created_at: now,
            }],
            assistant_messages: vec![Message {
                id: assistant,
                conversation_id: conversation,
                parent_id: Some(user),
                role: crate::domain::Role::Assistant,
                content: String::new(),
                attachments: Vec::new(),
                created_at: now,
            }],
            generations: vec![generation],
            usage: vec![UsageRecord::pending(request)],
            intended_main_leaf: None,
            replacement: None,
            draft_consumption: None,
        })
        .unwrap();
        let provider = Arc::new(DelayedCostProvider(AtomicUsize::new(0)));
        let (events, _rx) = broadcast::channel(16);
        let service = ChatService::new(provider.clone(), db.clone(), events, 1);
        assert_eq!(service.repair_pending_usage().await.unwrap(), 1);
        assert_eq!(provider.0.load(Ordering::SeqCst), 2);
        let record = db.usage_record(request).unwrap().unwrap();
        assert_eq!(record.status, crate::domain::UsageStatus::Known);
        assert_eq!(record.prompt_tokens, Some(15));
        assert_eq!(record.cost, Some(rust_decimal::Decimal::new(491, 5)));
        settle_usage(
            &provider,
            &db,
            &service.usage_repairs,
            &service.events,
            conversation,
            batch,
            assistant,
            request,
            Some("gen-delayed".into()),
        )
        .await;
        assert_eq!(
            provider.0.load(Ordering::SeqCst),
            2,
            "Known 账目不应再次 GET"
        );
    }
}
