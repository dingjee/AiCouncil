//! 提供方层:OpenRouter / OpenAI 兼容适配器。
//!
//! UI 与服务层只依赖 [`Provider`] trait;不拼接 API 请求的细节
//! 不外泄。所有异步生成事件携带四元组定位
//! (conversation_id + batch_id + request_id + assistant_message_id),
//! UI 按 ID 更新,不按窗口/模型路由(PLAN 3.2)。

pub mod catalog;
pub mod deepseek;
pub mod openrouter;
pub mod sse;

use std::future::Future;
use std::pin::Pin;

use futures_util::Stream;
use rust_decimal::Decimal;

use crate::domain::plan::{ContextMessage, PlannedRequest};
use crate::domain::recovery::{DispatchPhase, FailureInfo, FailureKind};
use crate::domain::Attachment;
use crate::domain::{BatchId, ConversationId, GenerationId, MessageId, Pricing, ProviderId};

/// 生成事件(服务层转发给 UI)。
#[derive(Debug, Clone)]
pub enum GenEvent {
    Started,
    TextDelta(String),
    /// API 内联 usage/cost(可能在文本终态之后到达)。
    UsageUpdated {
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        cost: Option<Decimal>,
    },
    /// 远端 generation id 在任意首帧出现时尽早持久化,便于中断后补查。
    RemoteGenerationId {
        remote_generation_id: String,
    },
    Finished {
        remote_generation_id: Option<String>,
        /// 成功终态的结束原因(stop 之外仍有可用正文,但含义不同)。
        finish_state: FinishState,
    },
    Failed {
        error: ProviderError,
    },
    Cancelled,
    /// admission/dispatch 门禁拒绝(未调用生成接口;卡片显示原因)。
    Rejected {
        detail: String,
    },
    /// 终态/内容落库失败(可见存储故障;不向 UI 宣称可靠保存成功)。
    PersistError {
        error: String,
    },
}

/// 成功终态的结束原因(5.4:合法非 stop 原因区别处理)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishState {
    Stop,
    /// length:输出被截断,正文可用。
    Length,
    /// content_filter:内容被过滤,保留可用正文。
    ContentFilter,
    /// 其他非 stop 原因(如 tool_calls)。
    Other(String),
}

impl FinishState {
    /// 组内短说明 i18n key(None = 无需提示)。
    pub fn i18n_key(&self) -> Option<&'static str> {
        match self {
            Self::Stop => None,
            Self::Length => Some("message.truncated"),
            Self::ContentFilter => Some("message.content_filtered"),
            Self::Other(_) => Some("message.finish_other"),
        }
    }
}

/// 四元组定位 + 事件。
#[derive(Debug, Clone)]
pub struct LocatedEvent {
    pub conversation_id: ConversationId,
    pub batch_id: BatchId,
    pub request_id: GenerationId,
    pub assistant_message_id: MessageId,
    pub event: GenEvent,
}

/// 错误分类(DESIGN 4:区分鉴权/权限/余额/限流/网络/模型不可用)。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProviderError {
    #[error("鉴权失败(401):{0}")]
    Unauthorized(String),
    #[error("权限或策略拒绝(403):{0}")]
    Forbidden(String),
    #[error("余额或额度不足(402):{0}")]
    PaymentRequired(String),
    #[error("限流({retry_after_secs:?}):{message}")]
    RateLimited {
        message: String,
        retry_after_secs: Option<u64>,
    },
    #[error("模型不可用或不存在:{0}")]
    ModelUnavailable(String),
    #[error("上下文超限:{0}")]
    ContextLength(String),
    #[error("请求参数被拒绝:{0}")]
    InvalidRequest(String),
    #[error("网络错误:{0}")]
    Network(String),
    #[error("超时:{0}")]
    Timeout(String),
    #[error("服务端错误({0}):{1}")]
    Server(u16, String),
    #[error("流中断:{0}")]
    StreamAborted(String),
    #[error("空响应:{0}")]
    EmptyResponse(String),
    #[error("响应解析失败:{0}")]
    Parse(String),
    #[error("协议失败:{0}")]
    Protocol(String),
    #[error("已取消")]
    Cancelled,
}

impl ProviderError {
    /// HTTP 状态分类(OpenRouter 与 OpenAI 兼容端点通用)。
    pub fn from_status(status: u16, body: String) -> Self {
        Self::from_status_with_retry(status, body, None)
    }

    /// HTTP 状态分类 + Retry-After 头秒数。
    pub fn from_status_with_retry(
        status: u16,
        body: String,
        retry_after_secs: Option<u64>,
    ) -> Self {
        match status {
            401 => Self::Unauthorized(body),
            // 403 单列权限错误:不统一解释成 Key 错误
            403 => Self::Forbidden(body),
            402 => Self::PaymentRequired(body),
            408 => Self::Timeout(body),
            429 => Self::RateLimited {
                message: body,
                retry_after_secs,
            },
            400 => {
                if body.contains("context length") || body.contains("maximum context") {
                    Self::ContextLength(body)
                } else if body.contains("model")
                    && (body.contains("not found") || body.contains("decommissioned"))
                {
                    Self::ModelUnavailable(body)
                } else {
                    Self::InvalidRequest(body)
                }
            }
            404 => Self::ModelUnavailable(body),
            s if (500..600).contains(&s) => rate_limited_server(s, body, retry_after_secs),
            s => Self::Server(s, body),
        }
    }

    /// 结构化 HTTP 错误:解析 body 中 OpenRouter/OpenAI 兼容的
    /// `{error:{message,code,metadata.error_type}}`;保留原 body 语义。
    pub fn from_http_error(status: u16, body: &str, retry_after_secs: Option<u64>) -> Self {
        let parsed = parse_error_payload(body);
        let message = parsed
            .as_ref()
            .map(|p| p.render(status))
            .unwrap_or_else(|| truncate(body, 400));
        Self::from_status_with_retry(
            status,
            message,
            retry_after_secs.or_else(|| parsed.as_ref().and_then(|p| p.retry_after)),
        )
    }

    /// 只读请求(目录/用量)是否可有限重试;已发出的生成不自动重试。
    pub fn retryable_for_readonly(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::Timeout(_) | Self::RateLimited { .. } | Self::Server(..)
        )
    }

    pub fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Unauthorized(_) => FailureKind::Unauthorized,
            Self::Forbidden(_) => FailureKind::Forbidden,
            Self::PaymentRequired(_) => FailureKind::PaymentRequired,
            Self::RateLimited { .. } => FailureKind::RateLimited,
            Self::ModelUnavailable(_) => FailureKind::ModelUnavailable,
            Self::ContextLength(_) => FailureKind::ContextLength,
            Self::InvalidRequest(_) => FailureKind::InvalidRequest,
            Self::Network(_) => FailureKind::Network,
            Self::Timeout(_) => FailureKind::Timeout,
            Self::Server(s, _) => {
                if *s == 503 || *s == 429 {
                    FailureKind::RateLimited
                } else {
                    FailureKind::Server
                }
            }
            Self::StreamAborted(_) => FailureKind::StreamAborted,
            Self::EmptyResponse(_) => FailureKind::EmptyResponse,
            Self::Parse(_) => FailureKind::ParseFailure,
            Self::Protocol(_) => FailureKind::ProtocolFailure,
            Self::Cancelled => FailureKind::Cancelled,
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Unauthorized(_) => Some(401),
            Self::Forbidden(_) => Some(403),
            Self::PaymentRequired(_) => Some(402),
            Self::RateLimited { .. } => Some(429),
            Self::Server(s, _) => Some(*s),
            _ => None,
        }
    }

    pub fn retry_after_secs(&self) -> Option<u64> {
        match self {
            Self::RateLimited {
                retry_after_secs, ..
            } => *retry_after_secs,
            _ => None,
        }
    }

    /// 转结构化失败信息(chat 服务持久化用)。
    pub fn to_failure_info(&self, phase: DispatchPhase) -> FailureInfo {
        let mut f = FailureInfo::new(self.failure_kind(), self.to_string(), phase);
        f.http_status = self.http_status();
        f.billing_uncertain = phase.may_be_billed();
        if let Some(ra) = self.retry_after_secs() {
            f.provider_code = Some(crate::domain::recovery::encode_retry_after(ra));
        }
        f
    }
}

/// 503 视作短时可重试的限流类别(其余 5xx 是普通服务端错误)。
fn rate_limited_server(status: u16, body: String, retry_after_secs: Option<u64>) -> ProviderError {
    if status == 503 {
        ProviderError::RateLimited {
            message: format!("服务端错误(503):{body}"),
            retry_after_secs,
        }
    } else {
        ProviderError::Server(status, body)
    }
}

/// 解析后的错误载荷(OpenRouter 结构优先)。
struct ParsedError {
    message: String,
    code: Option<String>,
    error_type: Option<String>,
    retry_after: Option<u64>,
}

impl ParsedError {
    fn render(&self, status: u16) -> String {
        let mut s = String::new();
        if let Some(et) = &self.error_type {
            s.push_str(&format!("[{et}] "));
        }
        s.push_str(&truncate(&self.message, 360));
        if let Some(c) = &self.code {
            s.push_str(&format!(" (code {c}, HTTP {status})"));
        } else {
            s.push_str(&format!(" (HTTP {status})"));
        }
        s
    }
}

fn parse_error_payload(body: &str) -> Option<ParsedError> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let e = v.get("error")?;
    let message = e
        .get("message")
        .and_then(|m| m.as_str())
        .map(str::to_string)?;
    let code = e
        .get("code")
        .map(|c| match c {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty());
    let error_type = e
        .pointer("/metadata/error_type")
        .and_then(|m| m.as_str())
        .map(String::from);
    let retry_after = e
        .pointer("/metadata/retry_after")
        .or_else(|| e.get("retry_after"))
        .and_then(|r| match r {
            serde_json::Value::Number(n) => n.as_u64(),
            serde_json::Value::String(s) => s.parse().ok(),
            _ => None,
        });
    Some(ParsedError {
        message,
        code,
        error_type,
        retry_after,
    })
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut end = n;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// 目录模型(自动同步部分;与用户偏好分表)。
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogEntry {
    pub model_id: String,
    pub name: String,
    pub context_window: Option<u64>,
    pub max_output: Option<u64>,
    pub pricing: Pricing,
    /// 能力标记(如 supported_parameters)。
    pub capabilities: Vec<String>,
    /// 服务端默认参数(Auto 模式依据;不含则省略可选字段)。
    pub server_defaults: Option<serde_json::Value>,
    pub available: bool,
}

/// 目录快照。
#[derive(Clone)]
pub struct CatalogSnapshot {
    pub provider_id: ProviderId,
    pub entries: Vec<CatalogEntry>,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
}

/// Key 状态。
#[derive(Debug, Clone)]
pub enum KeyStatus {
    Ok {
        label: Option<String>,
        usage: Option<Decimal>,
        limit: Option<Decimal>,
        limit_remaining: Option<Decimal>,
    },
    Unsupported,
    PermissionDenied,
}

/// 用量查询结果(generation 补查)。
#[derive(Debug, Clone)]
pub struct UsageResult {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cost: Option<Decimal>,
}

/// 取消令牌(独立请求粒度;watch 通道实现,无额外依赖)。
pub mod cancel {
    /// 取消令牌:克隆共享,触发后所有副本感知。
    #[derive(Clone)]
    pub struct CancellationToken {
        tx: tokio::sync::watch::Sender<bool>,
        rx: tokio::sync::watch::Receiver<bool>,
    }

    impl CancellationToken {
        pub fn new() -> Self {
            let (tx, rx) = tokio::sync::watch::channel(false);
            Self { tx, rx }
        }

        pub fn cancel(&self) {
            let _ = self.tx.send(true);
        }

        pub fn is_cancelled(&self) -> bool {
            *self.rx.borrow()
        }

        /// 异步等待取消(或已取消立即返回)。
        pub async fn cancelled(&self) {
            if self.is_cancelled() {
                return;
            }
            let mut rx = self.rx.clone();
            let _ = rx.changed().await;
        }
    }

    impl Default for CancellationToken {
        fn default() -> Self {
            Self::new()
        }
    }

    impl std::fmt::Debug for CancellationToken {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "CancellationToken(cancelled={})", self.is_cancelled())
        }
    }
}

/// 发往提供方的不可变请求(SendPlan 冻结后组装)。
#[derive(Debug, Clone)]
pub struct ImmutableRequest {
    pub conversation_id: ConversationId,
    pub batch_id: BatchId,
    pub request_id: GenerationId,
    pub assistant_message_id: MessageId,
    pub model: String,
    /// Auto 已合成:仅含服务端默认值或用户有效覆盖;可为空。
    pub params: Option<serde_json::Value>,
    pub system_prompt: Option<String>,
    pub history: Vec<ContextMessage>,
    /// 本轮新 user 内容(已按附件拼接后的最终文本)。
    pub prompt: String,
}

impl ImmutableRequest {
    pub fn from_planned(
        req: &PlannedRequest,
        conversation_id: ConversationId,
        batch_id: BatchId,
        assistant_message_id: MessageId,
        prompt: &str,
        attachments: &[Attachment],
        system_prompt: Option<String>,
    ) -> Self {
        Self {
            conversation_id,
            batch_id,
            request_id: req.request_id,
            assistant_message_id,
            model: req.model_id.clone(),
            params: req.params.clone(),
            system_prompt,
            history: req.history.clone(),
            // P7B:附件在此组合为最终 prompt,对提供方透明
            prompt: crate::domain::attachment::compose_prompt(prompt, attachments),
        }
    }
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<GenEvent, ProviderError>> + Send>>;

/// 提供方接口(概念签名见 PLAN 3.2)。
#[allow(async_fn_in_trait)]
pub trait Provider {
    fn id(&self) -> ProviderId;

    async fn list_models(&self) -> Result<CatalogSnapshot, ProviderError>;

    /// 流式聊天;`http` 便于测试注入 mock 客户端。
    fn stream(&self, req: ImmutableRequest, cancel: cancel::CancellationToken) -> EventStream;

    fn query_usage<'a>(
        &'a self,
        remote_generation_id: &'a str,
    ) -> impl Future<Output = Result<Option<UsageResult>, ProviderError>> + Send + 'a;

    async fn query_key_status(&self) -> Result<KeyStatus, ProviderError>;

    /// 连接测试(鉴权 + 目录可拉取)。
    async fn test_connection(&self) -> Result<(), ProviderError>;
}
