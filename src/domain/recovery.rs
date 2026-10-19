//! 失败恢复域:结构化失败信息、重试资格与退避(REQUEST_RECOVERY_MODEL_PICKER_PLAN 4.2/4.3/5.1)。
//!
//! - `FailureInfo` 持久化为可版本化 JSON;未知枚举/旧字符串安全降级,
//!   不能因读不了错误而读不了会话。
//! - 重试资格用最新持久数据计算;冷却持久化绝对时间(epoch 毫秒),
//!   本次运行倒计时由 UI 用单调时钟换算。

use serde::{Deserialize, Serialize};

/// 失败类别(stringly 序列化;未知值降级 Unknown)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 出口门禁拦截(明确 CN/HK)。
    EgressBlocked,
    /// 出口状态无法确认(超时/断网/无效数据/冲突)。
    EgressUnknown,
    /// 401:凭据无效。
    Unauthorized,
    /// 403:权限/策略拒绝(不统一解释成 Key 错误)。
    Forbidden,
    /// 402:余额或额度不足。
    PaymentRequired,
    /// 429/503/明确短时预算限制。
    RateLimited,
    /// 上下文超限。
    ContextLength,
    /// 请求参数被拒绝。
    InvalidRequest,
    /// 模型不可用/不存在。
    ModelUnavailable,
    /// 网络错误(未发出或发出后断流)。
    Network,
    /// 超时。
    Timeout,
    /// 服务端 5xx。
    Server,
    /// 连接在终态前关闭(有部分内容)。
    StreamAborted,
    /// 流正常结束但无任何内容。
    EmptyResponse,
    /// 协议失败([DONE] 无终态等)。
    ProtocolFailure,
    /// 响应解析失败。
    ParseFailure,
    /// 非停止原因截断(length/content_filter 等,有可用正文)。
    Truncated,
    /// 程序退出时未完成。
    Interrupted,
    /// 用户取消。
    Cancelled,
    /// 未知/旧数据。
    Unknown,
}

impl FailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EgressBlocked => "egress_blocked",
            Self::EgressUnknown => "egress_unknown",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::PaymentRequired => "payment_required",
            Self::RateLimited => "rate_limited",
            Self::ContextLength => "context_length",
            Self::InvalidRequest => "invalid_request",
            Self::ModelUnavailable => "model_unavailable",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Server => "server",
            Self::StreamAborted => "stream_aborted",
            Self::EmptyResponse => "empty_response",
            Self::ProtocolFailure => "protocol_failure",
            Self::ParseFailure => "parse_failure",
            Self::Truncated => "truncated",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }

    /// 未知字符串安全降级 Unknown。
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "egress_blocked" => Self::EgressBlocked,
            "egress_unknown" => Self::EgressUnknown,
            "unauthorized" => Self::Unauthorized,
            "forbidden" => Self::Forbidden,
            "payment_required" => Self::PaymentRequired,
            "rate_limited" => Self::RateLimited,
            "context_length" => Self::ContextLength,
            "invalid_request" => Self::InvalidRequest,
            "model_unavailable" => Self::ModelUnavailable,
            "network" => Self::Network,
            "timeout" => Self::Timeout,
            "server" => Self::Server,
            "stream_aborted" => Self::StreamAborted,
            "empty_response" => Self::EmptyResponse,
            "protocol_failure" => Self::ProtocolFailure,
            "parse_failure" => Self::ParseFailure,
            "truncated" => Self::Truncated,
            "interrupted" => Self::Interrupted,
            "cancelled" => Self::Cancelled,
            "unknown" => Self::Unknown,
            _ => Self::Unknown,
        }
    }

    /// 展示用 i18n key(卡片短原因)。
    pub fn i18n_key(self) -> &'static str {
        match self {
            Self::EgressBlocked => "failure.egress_blocked",
            Self::EgressUnknown => "failure.egress_unknown",
            Self::Unauthorized => "failure.unauthorized",
            Self::Forbidden => "failure.forbidden",
            Self::PaymentRequired => "failure.payment_required",
            Self::RateLimited => "failure.rate_limited",
            Self::ContextLength => "failure.context_length",
            Self::InvalidRequest => "failure.invalid_request",
            Self::ModelUnavailable => "failure.model_unavailable",
            Self::Network => "failure.network",
            Self::Timeout => "failure.timeout",
            Self::Server => "failure.server",
            Self::StreamAborted => "failure.stream_aborted",
            Self::EmptyResponse => "failure.empty_response",
            Self::ProtocolFailure => "failure.protocol_failure",
            Self::ParseFailure => "failure.parse_failure",
            Self::Truncated => "failure.truncated",
            Self::Interrupted => "failure.interrupted",
            Self::Cancelled => "failure.cancelled",
            Self::Unknown => "failure.unknown",
        }
    }
}

/// 请求阶段:NotDispatched 才能确定未调用生成接口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchPhase {
    NotDispatched,
    DispatchAttempted,
    RemoteAccepted,
}

impl DispatchPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotDispatched => "not_dispatched",
            Self::DispatchAttempted => "dispatch_attempted",
            Self::RemoteAccepted => "remote_accepted",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "dispatch_attempted" => Self::DispatchAttempted,
            "remote_accepted" => Self::RemoteAccepted,
            _ => Self::NotDispatched,
        }
    }

    /// 已发出(可能已计费):DispatchAttempted 起。
    pub fn may_be_billed(self) -> bool {
        matches!(self, Self::DispatchAttempted | Self::RemoteAccepted)
    }
}

/// 结构化失败信息(持久化 JSON;版本化以便演进)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FailureInfo {
    pub version: u8,
    /// 类别(序列化为 kind 字符串;未知降级 Unknown)。
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_not_before_ms: Option<i64>,
    pub dispatch_phase: String,
    #[serde(default)]
    pub billing_uncertain: bool,
}

impl FailureInfo {
    pub fn new(kind: FailureKind, message: impl Into<String>, phase: DispatchPhase) -> Self {
        Self {
            version: 1,
            kind: kind.as_str().to_string(),
            http_status: None,
            provider_code: None,
            message: message.into(),
            retry_not_before_ms: None,
            dispatch_phase: phase.as_str().to_string(),
            billing_uncertain: false,
        }
    }

    pub fn kind(&self) -> FailureKind {
        FailureKind::from_str(&self.kind)
    }

    pub fn phase(&self) -> DispatchPhase {
        DispatchPhase::from_str(&self.dispatch_phase)
    }

    /// 解析失败/未知 JSON 的安全降级:保留原始字符串,不阻止会话读取。
    pub fn from_json(s: &str) -> Option<Self> {
        serde_json::from_str(s).ok()
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }
}

/// 补救动作(卡片首要动作)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// 切换 IP / 重新检测。
    SwitchIp,
    /// 打开凭据设置。
    Credentials,
    /// 检查余额/额度。
    Balance,
    /// 修改输入或参数。
    EditInput,
    /// 选择其他模型。
    PickModel,
    /// 无需补救(可重试)。
    None,
}

/// 重试资格决策。
#[derive(Debug, Clone, PartialEq)]
pub enum RetryDecision {
    /// 允许显式重试/再生成。
    Allowed,
    /// 同 lineage 已有活动尝试。
    InFlight,
    /// 冷却中(绝对 epoch 毫秒;UI 用单调时钟换算倒计时)。
    Cooldown(i64),
    /// 需要先执行补救动作;原样重试禁用。
    NeedsAction(Remedy),
}

/// 无 Retry-After 的临时失败退避:2、4、8…秒,上限 30 秒。
/// `attempt` 从 1 起(第 1 次失败后的等待)。
pub fn backoff_delay_ms(attempt: u32) -> i64 {
    let base = 2u64.saturating_pow(attempt.clamp(1, 16));
    (base.min(30) * 1000) as i64
}

/// 失败类别是否计入自动退避(无 Retry-After 的临时错误)。
fn backoff_kind(kind: FailureKind) -> bool {
    matches!(
        kind,
        FailureKind::Network
            | FailureKind::Timeout
            | FailureKind::Server
            | FailureKind::StreamAborted
            | FailureKind::ProtocolFailure
    )
}

/// 重试资格(4.3 行为表)。
///
/// - `status`:该卡最新 Generation 状态;活动态 → InFlight。
/// - `failure`:该 lineage 最新失败的 FailureInfo(无则视为成功/取消)。
/// - `now_ms`:当前 epoch 毫秒(与持久化的 retry_not_before 比较)。
/// - `lineage_active`:同 lineage 是否已有活动尝试(服务/DB 判定)。
pub fn retry_decision(
    status: crate::domain::GenerationStatus,
    failure: Option<&FailureInfo>,
    now_ms: i64,
    lineage_active: bool,
) -> RetryDecision {
    use crate::domain::GenerationStatus as S;
    use FailureKind as K;
    if matches!(status, S::Queued | S::Connecting | S::Streaming) || lineage_active {
        return RetryDecision::InFlight;
    }
    let Some(f) = failure else {
        // 无失败记录:完成(再生成)/取消(重新生成)
        return RetryDecision::Allowed;
    };
    // 冷却检查对所有带 retry_not_before 的类别生效(Retry-After 或退避)
    if let Some(until) = f.retry_not_before_ms {
        if now_ms < until {
            return RetryDecision::Cooldown(until);
        }
    }
    match f.kind() {
        K::EgressBlocked | K::EgressUnknown => RetryDecision::NeedsAction(Remedy::SwitchIp),
        K::Unauthorized => RetryDecision::NeedsAction(Remedy::Credentials),
        K::PaymentRequired => RetryDecision::NeedsAction(Remedy::Balance),
        K::ContextLength | K::InvalidRequest | K::EmptyResponse | K::ParseFailure => {
            RetryDecision::NeedsAction(Remedy::EditInput)
        }
        K::ModelUnavailable => RetryDecision::NeedsAction(Remedy::PickModel),
        // 403 未知权限错误可显式再试;RateLimited 到期只恢复按钮;
        // 网络类退避到期允许;Cancelled/Truncated/Unknown 允许显式重试。
        _ => RetryDecision::Allowed,
    }
}

/// 给失败信息计算 retry_not_before(写入时调用)。
/// `attempt` 为该 lineage 已失败次数(含本次)。
pub fn apply_backoff(f: &mut FailureInfo, attempt: u32) {
    if let Some(ra) = f
        .provider_code
        .as_deref()
        .and_then(parse_retry_after_seconds)
    {
        f.retry_not_before_ms = Some(now_ms() + (ra as i64) * 1000);
        return;
    }
    if backoff_kind(f.kind()) {
        f.retry_not_before_ms = Some(now_ms() + backoff_delay_ms(attempt));
    }
}

/// Retry-After 秒数(provider_code 字段携带 "retry-after:N" 形态)。
pub fn encode_retry_after(secs: u64) -> String {
    format!("retry-after:{secs}")
}

pub fn parse_retry_after_seconds(s: &str) -> Option<u64> {
    s.strip_prefix("retry-after:").and_then(|v| v.parse().ok())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::GenerationStatus as S;

    fn info(kind: FailureKind) -> FailureInfo {
        FailureInfo::new(kind, "m", DispatchPhase::DispatchAttempted)
    }

    #[test]
    fn kind_roundtrip_and_fallback() {
        for k in [
            FailureKind::EgressBlocked,
            FailureKind::RateLimited,
            FailureKind::EmptyResponse,
        ] {
            assert_eq!(FailureKind::from_str(k.as_str()), k);
        }
        assert_eq!(
            FailureKind::from_str("brand-new-kind"),
            FailureKind::Unknown
        );
        // 旧 JSON 降级:未知 kind 字符串可读
        let raw =
            r#"{"version":1,"kind":"future_kind","message":"x","dispatch_phase":"not_dispatched"}"#;
        let f = FailureInfo::from_json(raw).unwrap();
        assert_eq!(f.kind(), FailureKind::Unknown);
    }

    #[test]
    fn active_status_is_inflight() {
        for s in [S::Queued, S::Connecting, S::Streaming] {
            assert_eq!(retry_decision(s, None, 0, false), RetryDecision::InFlight);
        }
        assert_eq!(
            retry_decision(S::Completed, None, 0, true),
            RetryDecision::InFlight,
            "lineage 有活动尝试也拒绝"
        );
    }

    #[test]
    fn terminal_states_allow_new_turn_but_gate_retry() {
        // B05:终态失败允许新轮次(发送路径);此处验证重试资格分层
        assert_eq!(
            retry_decision(S::Failed, None, 0, false),
            RetryDecision::Allowed
        );
        assert_eq!(
            retry_decision(S::Cancelled, None, 0, false),
            RetryDecision::Allowed
        );
        assert_eq!(
            retry_decision(S::Interrupted, None, 0, false),
            RetryDecision::Allowed
        );
    }

    #[test]
    fn remedies_by_kind() {
        let cases = [
            (FailureKind::EgressBlocked, Remedy::SwitchIp),
            (FailureKind::EgressUnknown, Remedy::SwitchIp),
            (FailureKind::Unauthorized, Remedy::Credentials),
            (FailureKind::PaymentRequired, Remedy::Balance),
            (FailureKind::ContextLength, Remedy::EditInput),
            (FailureKind::EmptyResponse, Remedy::EditInput),
            (FailureKind::ModelUnavailable, Remedy::PickModel),
        ];
        for (kind, remedy) in cases {
            let d = retry_decision(S::Failed, Some(&info(kind)), 0, false);
            assert_eq!(d, RetryDecision::NeedsAction(remedy), "{kind:?}");
        }
        // 403:未知权限错误可显式再试
        assert_eq!(
            retry_decision(S::Failed, Some(&info(FailureKind::Forbidden)), 0, false),
            RetryDecision::Allowed
        );
    }

    #[test]
    fn cooldown_respects_retry_after_and_backoff() {
        // B10:429 Retry-After 到期前拒绝;到期只解锁
        let mut f = info(FailureKind::RateLimited);
        f.provider_code = Some(encode_retry_after(60));
        apply_backoff(&mut f, 1);
        let until = f.retry_not_before_ms.unwrap();
        assert_eq!(
            retry_decision(S::Failed, Some(&f), until - 1, false),
            RetryDecision::Cooldown(until)
        );
        assert_eq!(
            retry_decision(S::Failed, Some(&f), until, false),
            RetryDecision::Allowed
        );
        // 网络类无 RA:2/4/8…30
        assert_eq!(backoff_delay_ms(1), 2000);
        assert_eq!(backoff_delay_ms(2), 4000);
        assert_eq!(backoff_delay_ms(3), 8000);
        assert_eq!(backoff_delay_ms(9), 30000, "上限 30 秒");
        let mut net = info(FailureKind::Network);
        apply_backoff(&mut net, 1);
        assert!(net.retry_not_before_ms.unwrap() > now_ms());
        // 非退避类别不写 retry_not_before
        let mut inv = info(FailureKind::InvalidRequest);
        apply_backoff(&mut inv, 1);
        assert_eq!(inv.retry_not_before_ms, None);
    }

    #[test]
    fn dispatch_phase_billing() {
        assert!(!DispatchPhase::NotDispatched.may_be_billed());
        assert!(DispatchPhase::DispatchAttempted.may_be_billed());
        assert!(DispatchPhase::RemoteAccepted.may_be_billed());
        assert_eq!(
            DispatchPhase::from_str("weird"),
            DispatchPhase::NotDispatched
        );
    }
}
