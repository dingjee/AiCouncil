//! IPC DTO:领域类型与前端线格式之间的显式映射。
//!
//! 线约定(docs/TAURI_IPC.md):snake_case 字段;ID 为 UUID 字符串;
//! 时间为 epoch 毫秒;金额为十进制字符串或 null。任何 DTO 不得携带
//! API Key 或凭据材料。

use serde::{Deserialize, Serialize};

use crate::domain::model_identity;
use crate::domain::{Conversation, Generation, Group, Message, UsageRecord};

// ---------- 通用 ----------

/// 统一错误负载(前端按 code 分支)。
#[derive(Debug, Clone, Serialize)]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            detail: None,
        }
    }

    pub fn with_detail(code: &str, message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            detail: Some(detail.into()),
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }

    pub fn not_found(what: &str) -> Self {
        Self::new("not_found", format!("{what} not found"))
    }
}

impl From<crate::storage::StorageError> for IpcError {
    fn from(e: crate::storage::StorageError) -> Self {
        match &e {
            crate::storage::StorageError::NotFound(m) => {
                Self::with_detail("not_found", "not found", m.clone())
            }
            _ => Self::with_detail("storage", "Storage error", e.to_string()),
        }
    }
}

impl From<crate::services::SubmitError> for IpcError {
    fn from(e: crate::services::SubmitError) -> Self {
        use crate::services::AdmissionError as A;
        match e {
            crate::services::SubmitError::Admission(a) => match a {
                A::EgressBlocked { country, detail } => Self::with_detail(
                    "admission.egress_blocked",
                    "Egress blocked by region policy",
                    format!("country={country:?} {detail}"),
                ),
                A::EgressUnknown { detail } => {
                    Self::with_detail("admission.egress_unknown", "Egress unverifiable", detail)
                }
                A::Duplicate { detail } => Self::with_detail(
                    "admission.duplicate",
                    "An attempt is already in flight",
                    detail,
                ),
                A::Cooldown { until_ms } => Self::with_detail(
                    "admission.cooldown",
                    "Retry cooldown active",
                    until_ms.to_string(),
                ),
                A::NeedsAction { remedy, detail } => Self::with_detail(
                    "admission.needs_action",
                    "Action required before retry",
                    format!("{remedy}: {detail}"),
                ),
                A::InvalidBranch { detail } => {
                    Self::with_detail("admission.invalid_branch", "Branch is not eligible", detail)
                }
            },
            crate::services::SubmitError::Storage(s) => s.into(),
        }
    }
}

impl From<std::io::Error> for IpcError {
    fn from(e: std::io::Error) -> Self {
        Self::with_detail("storage", "IO error", e.to_string())
    }
}

/// 解析 UUID 字符串(ID 解析失败一律 invalid_input,不静默 nil)。
macro_rules! parse_id {
    ($ty:ty, $s:expr) => {{
        match uuid::Uuid::parse_str($s) {
            Ok(u) => Ok(<$ty>::from_uuid_str(&u.to_string())),
            Err(e) => {
                return Err($crate::desktop::dto::IpcError::invalid_input(format!(
                    "bad id `{}`: {e}",
                    $s
                )));
            }
        }
    }};
}

pub(crate) use parse_id;

fn decimal_string(d: Option<rust_decimal::Decimal>) -> Option<String> {
    d.map(|d| d.to_string())
}

// ---------- 会话与分组 ----------

#[derive(Debug, Clone, Serialize)]
pub struct GroupDto {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
}

impl From<&Group> for GroupDto {
    fn from(g: &Group) -> Self {
        Self {
            id: g.id.as_str(),
            name: g.name.clone(),
            sort_order: g.sort_order,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationSummaryDto {
    pub id: String,
    pub title: String,
    pub group_id: Option<String>,
    pub system_prompt: String,
    pub title_source: String,
    pub title_status: String,
    pub title_revision: i64,
    /// 标题自动生成尝试次数(0=尚未尝试;失败后保留 fallback 且 >0)。
    pub title_attempt_count: i64,
    pub main_leaf_id: Option<String>,
    pub draft_revision: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl From<&Conversation> for ConversationSummaryDto {
    fn from(c: &Conversation) -> Self {
        Self {
            id: c.id.as_str(),
            title: c.title.clone(),
            group_id: c.group_id.map(|g| g.as_str()),
            system_prompt: c.system_prompt.clone(),
            title_source: c.title_source.as_str().to_string(),
            title_status: c.title_status.as_str().to_string(),
            title_revision: c.title_revision,
            title_attempt_count: c.title_attempt_count,
            main_leaf_id: c.main_leaf_id.map(|m| m.as_str()),
            draft_revision: c.draft_revision,
            created_at_ms: c.created_at.timestamp_millis(),
            updated_at_ms: c.updated_at.timestamp_millis(),
        }
    }
}

// ---------- 消息 / 生成 / 用量 ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentDto {
    pub name: String,
    pub content: String,
}

impl From<&crate::domain::Attachment> for AttachmentDto {
    fn from(a: &crate::domain::Attachment) -> Self {
        Self {
            name: a.name.clone(),
            content: a.content.clone(),
        }
    }
}

impl From<AttachmentDto> for crate::domain::Attachment {
    fn from(a: AttachmentDto) -> Self {
        Self {
            name: a.name,
            content: a.content,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MessageDto {
    pub id: String,
    pub conversation_id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub content: String,
    pub created_at_ms: i64,
    pub attachments: Vec<AttachmentDto>,
}

impl From<&Message> for MessageDto {
    fn from(m: &Message) -> Self {
        Self {
            id: m.id.as_str(),
            conversation_id: m.conversation_id.as_str(),
            parent_id: m.parent_id.map(|p| p.as_str()),
            role: m.role.as_str().to_string(),
            content: m.content.clone(),
            created_at_ms: m.created_at.timestamp_millis(),
            attachments: m.attachments.iter().map(AttachmentDto::from).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GenerationDto {
    pub request_id: String,
    pub batch_id: String,
    pub user_message_id: String,
    pub assistant_message_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub status: String,
    pub system_prompt: Option<String>,
    pub price_prompt: Option<String>,
    pub price_completion: Option<String>,
    pub remote_generation_id: Option<String>,
    pub error: Option<String>,
    pub retry_of: Option<String>,
    pub retry_root_id: Option<String>,
    pub retry_not_before_ms: Option<i64>,
    pub dispatch_phase: String,
    pub failure_kind: Option<String>,
    pub failure_message: Option<String>,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

impl From<&Generation> for GenerationDto {
    fn from(g: &Generation) -> Self {
        Self {
            request_id: g.request_id.as_str(),
            batch_id: g.batch_id.as_str(),
            user_message_id: g.user_message_id.as_str(),
            assistant_message_id: g.assistant_message_id.as_str(),
            provider_id: g.provider_id.as_str(),
            model_id: g.model_id.clone(),
            status: g.status.as_str().to_string(),
            system_prompt: g.system_prompt.clone(),
            price_prompt: decimal_string(g.price_snapshot.and_then(|p| p.prompt_per_mtok)),
            price_completion: decimal_string(g.price_snapshot.and_then(|p| p.completion_per_mtok)),
            remote_generation_id: g.remote_generation_id.clone(),
            error: g.error.clone(),
            retry_of: g.retry_of.map(|r| r.as_str()),
            retry_root_id: g.retry_root_id.map(|r| r.as_str()),
            retry_not_before_ms: g.retry_not_before_ms,
            dispatch_phase: g.dispatch_phase.as_str().to_string(),
            failure_kind: g.failure.as_ref().map(|f| f.kind().as_str().to_string()),
            failure_message: g.failure.as_ref().map(|f| f.message.clone()),
            started_at_ms: g.started_at.map(|t| t.timestamp_millis()),
            finished_at_ms: g.finished_at.map(|t| t.timestamp_millis()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageRecordDto {
    pub request_id: String,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cost_text: Option<String>,
    pub cost_source: Option<String>,
    pub status: String,
}

impl From<&UsageRecord> for UsageRecordDto {
    fn from(u: &UsageRecord) -> Self {
        Self {
            request_id: u.request_id.as_str(),
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
            cost_text: decimal_string(u.cost),
            cost_source: u.cost_source.map(|c| c.as_str().to_string()),
            status: u.status.as_str().to_string(),
        }
    }
}

/// load_conversation 的 edit_latest 字段。
#[derive(Debug, Clone, Serialize)]
pub struct EditLatestDto {
    pub target_id: String,
    pub content: String,
    pub attachments: Vec<AttachmentDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConversationDetailDto {
    pub conversation: ConversationSummaryDto,
    pub messages: Vec<MessageDto>,
    pub generations: Vec<GenerationDto>,
    pub usage: Vec<UsageRecordDto>,
    pub edit_latest: Option<EditLatestDto>,
}

// ---------- 草稿 ----------

#[derive(Debug, Clone, Serialize)]
pub struct DraftDto {
    pub text: String,
    pub attachments: Vec<AttachmentDto>,
    pub edit_text: String,
    pub edit_attachments: Vec<AttachmentDto>,
    pub edit_target_id: Option<String>,
    pub revision: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RevisionDto {
    pub revision: i64,
}

// ---------- 设置 ----------

#[derive(Debug, Clone, Serialize)]
pub struct SettingsDto {
    pub max_concurrent_requests: u32,
    pub assumed_output_tokens: u64,
    pub connect_timeout_secs: u64,
    pub read_idle_timeout_secs: u64,
    pub body_text_size: Option<f32>,
    pub locale: String,
    pub theme_id: String,
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SettingsPatchDto {
    pub max_concurrent_requests: Option<u32>,
    pub assumed_output_tokens: Option<u64>,
    pub connect_timeout_secs: Option<u64>,
    pub read_idle_timeout_secs: Option<u64>,
    pub body_text_size: Option<f32>,
    pub locale: Option<String>,
    pub theme_id: Option<String>,
    pub sidebar_width: Option<f32>,
    pub sidebar_collapsed: Option<bool>,
}

// ---------- 目录与偏好 ----------

#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntryDto {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub family: String,
    pub context_length: Option<u64>,
    pub prompt_price: Option<String>,
    pub completion_price: Option<String>,
    pub supports_streaming: bool,
    pub supports_temperature: bool,
    pub is_anthropic: bool,
}

impl CatalogEntryDto {
    pub fn from_entry(
        provider_id: &crate::domain::ProviderId,
        e: &crate::providers::CatalogEntry,
    ) -> Self {
        let presentation = model_identity::presentation(&e.model_id, &e.name);
        Self {
            id: e.model_id.clone(),
            provider_id: provider_id.as_str(),
            name: e.name.clone(),
            family: presentation.group_label,
            context_length: e.context_window,
            prompt_price: decimal_string(e.pricing.prompt_per_mtok),
            completion_price: decimal_string(e.pricing.completion_per_mtok),
            supports_streaming: e.capabilities.iter().any(|c| c == "stream"),
            supports_temperature: e.capabilities.is_empty()
                || e.capabilities.iter().any(|c| c == "temperature"),
            is_anthropic: model_identity::is_anthropic(&e.model_id),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CatalogStateDto {
    pub updated_at_ms: Option<i64>,
    pub entries: Vec<CatalogEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPreferenceDto {
    pub enabled: bool,
    pub sort_order: i64,
    pub is_default: bool,
    pub overrides: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelPreferenceEntryDto {
    pub provider_id: String,
    pub model_id: String,
    #[serde(flatten)]
    pub preference: ModelPreferenceDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyStatusDto {
    pub status: String,
    pub label: Option<String>,
    pub usage: Option<String>,
    pub limit: Option<String>,
    pub limit_remaining: Option<String>,
    pub error: Option<String>,
}

// ---------- 出口 ----------

#[derive(Debug, Clone, Serialize)]
pub struct EgressStateDto {
    pub verdict: String,
    pub country_code: Option<String>,
    pub detail: String,
}

// ---------- 发送 ----------

#[derive(Debug, Clone, Deserialize)]
pub struct ModelTargetRefDto {
    pub provider_id: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MultiBranchItemDto {
    pub leaf: String,
    pub target: ModelTargetRefDto,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectionDto {
    Single {
        leaf: Option<String>,
        targets: Vec<ModelTargetRefDto>,
        primary: usize,
    },
    SameQuestionMulti {
        leaf: Option<String>,
        targets: Vec<ModelTargetRefDto>,
        primary: usize,
    },
    MultiBranch {
        branches: Vec<MultiBranchItemDto>,
        primary: usize,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct SendMessageRequestDto {
    pub conversation_id: String,
    pub mode: String,
    pub prompt: String,
    pub attachments: Vec<AttachmentDto>,
    pub expected_draft_revision: i64,
    pub selection: SelectionDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct SendReceiptDto {
    pub conversation_id: String,
    pub batch_id: String,
    pub primary_request_id: String,
    pub assistant_of: std::collections::HashMap<String, String>,
    pub adopted_main_leaf: Option<String>,
}

impl From<crate::services::SubmitReceipt> for SendReceiptDto {
    fn from(r: crate::services::SubmitReceipt) -> Self {
        Self {
            conversation_id: r.conversation_id.as_str(),
            batch_id: r.batch_id.as_str(),
            primary_request_id: r.primary_request_id.as_str(),
            assistant_of: r
                .assistant_of
                .into_iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
            adopted_main_leaf: r.adopted_main_leaf.map(|m| m.as_str()),
        }
    }
}

// ---------- 导出 ----------

#[derive(Debug, Clone, Serialize)]
pub struct ExportResultDto {
    pub filename: String,
    pub content: String,
}

// ---------- 凭据 ----------

#[derive(Debug, Clone, Serialize)]
pub struct CredentialsStatusDto {
    pub openrouter: bool,
    pub deepseek: bool,
}

// ---------- 事件 ----------

/// chat-event 的 event 载荷(GenEvent 线格式)。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GenEventDto {
    Started,
    TextDelta {
        delta: String,
    },
    UsageUpdated {
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        cost: Option<String>,
    },
    RemoteGenerationId {
        id: String,
    },
    Finished {
        remote_generation_id: Option<String>,
        finish_state: String,
    },
    Failed {
        error: ProviderErrorDto,
    },
    Cancelled,
    Rejected {
        detail: String,
    },
    PersistError {
        error: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderErrorDto {
    pub kind: String,
    pub message: String,
    pub retry_after_secs: Option<u64>,
}

impl From<&crate::providers::ProviderError> for ProviderErrorDto {
    fn from(e: &crate::providers::ProviderError) -> Self {
        use crate::providers::ProviderError as E;
        let (kind, retry_after_secs) = match e {
            E::Unauthorized(_) => ("unauthorized", None),
            E::Forbidden(_) => ("forbidden", None),
            E::PaymentRequired(_) => ("payment_required", None),
            E::RateLimited {
                retry_after_secs, ..
            } => ("rate_limited", *retry_after_secs),
            E::ModelUnavailable(_) => ("model_unavailable", None),
            E::ContextLength(_) => ("context_length", None),
            E::InvalidRequest(_) => ("invalid_request", None),
            E::Network(_) => ("network", None),
            E::Timeout(_) => ("timeout", None),
            E::Server(..) => ("server", None),
            E::StreamAborted(_) => ("stream_aborted", None),
            E::EmptyResponse(_) => ("empty_response", None),
            E::Parse(_) => ("parse", None),
            E::Protocol(_) => ("protocol", None),
            E::Cancelled => ("cancelled", None),
        };
        Self {
            kind: kind.to_string(),
            message: e.to_string(),
            retry_after_secs,
        }
    }
}

impl From<crate::providers::LocatedEvent> for ChatEventDto {
    fn from(ev: crate::providers::LocatedEvent) -> Self {
        use crate::providers::GenEvent as G;
        let event = match ev.event {
            G::Started => GenEventDto::Started,
            G::TextDelta(delta) => GenEventDto::TextDelta { delta },
            G::UsageUpdated {
                prompt_tokens,
                completion_tokens,
                cost,
            } => GenEventDto::UsageUpdated {
                prompt_tokens,
                completion_tokens,
                cost: cost.map(|c| c.to_string()),
            },
            G::RemoteGenerationId {
                remote_generation_id,
            } => GenEventDto::RemoteGenerationId {
                id: remote_generation_id,
            },
            G::Finished {
                remote_generation_id,
                finish_state,
            } => {
                use crate::providers::FinishState as F;
                let fs = match finish_state {
                    F::Stop => "stop",
                    F::Length => "length",
                    F::ContentFilter => "content_filter",
                    F::Other(_) => "other",
                };
                GenEventDto::Finished {
                    remote_generation_id,
                    finish_state: fs.to_string(),
                }
            }
            G::Failed { error } => GenEventDto::Failed {
                error: ProviderErrorDto::from(&error),
            },
            G::Cancelled => GenEventDto::Cancelled,
            G::Rejected { detail } => GenEventDto::Rejected { detail },
            G::PersistError { error } => GenEventDto::PersistError { error },
        };
        Self {
            conversation_id: ev.conversation_id.as_str(),
            batch_id: ev.batch_id.as_str(),
            request_id: ev.request_id.as_str(),
            assistant_message_id: ev.assistant_message_id.as_str(),
            event,
        }
    }
}

/// 线格式回归:Failed 必须序列化为嵌套 `error`(前端
/// `event.error.message` 依赖此形态;0.2.2 曾 flatten 导致
/// Connecting 卡死)。夹具与前端测试共用同一文件,两侧不得各自
/// 发明事件形态。
#[test]
fn failed_event_serializes_nested_error() {
    use crate::providers::{GenEvent, LocatedEvent};
    const FIXTURE: &str = include_str!("../../frontend/tests/fixtures/native_failed_event.json");
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("夹具应为合法 JSON");

    let ev = LocatedEvent {
        conversation_id: crate::domain::ConversationId::new(),
        batch_id: crate::domain::BatchId::new(),
        request_id: crate::domain::GenerationId::new(),
        assistant_message_id: crate::domain::MessageId::new(),
        event: GenEvent::Failed {
            // DTO 层渲染完整展示文案(含类别前缀);夹具存完整消息。
            error: crate::providers::ProviderError::Unauthorized("User not found.".into()),
        },
    };
    let dto = ChatEventDto::from(ev);
    let json = serde_json::to_value(&dto.event).unwrap();
    assert_eq!(json, fixture["event"], "线格式必须与共享夹具逐字段一致");
    let err = json
        .get("error")
        .and_then(|e| e.as_object())
        .expect("error 必须为嵌套对象(不得 flatten 到顶层)");
    assert_eq!(err["kind"], "unauthorized");
    assert!(json.get("kind").is_none() && json.get("retry_after_secs").is_none());
}

/// chat-event 顶层载荷(LocatedEvent 线格式)。
#[derive(Debug, Clone, Serialize)]
pub struct ChatEventDto {
    pub conversation_id: String,
    pub batch_id: String,
    pub request_id: String,
    pub assistant_message_id: String,
    pub event: GenEventDto,
}

/// title-event 载荷。
#[derive(Debug, Clone, Serialize)]
pub struct TitleEventDto {
    pub conversation_id: String,
    pub request_token: String,
    pub outcome: String,
    pub error_kind: Option<String>,
}

// ---------- Bootstrap ----------

#[derive(Debug, Clone, Serialize)]
pub struct BootstrapStateDto {
    pub app_version: String,
    pub settings: SettingsDto,
    pub conversations: Vec<ConversationSummaryDto>,
    pub groups: Vec<GroupDto>,
    pub pending_new_conversation: Option<String>,
    pub catalog: CatalogStateDto,
    pub preferences: Vec<ModelPreferenceEntryDto>,
    pub credentials: CredentialsStatusDto,
    pub egress: Option<EgressStateDto>,
}
