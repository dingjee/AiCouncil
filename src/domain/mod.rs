//! 领域模型:ID、消息树、发送计划、状态、金额与错误。
//!
//! 设计约束(PLAN 3.1/4.x):
//! - 逻辑字段以此为准;类型可调,不得丢失关系与审计信息。
//! - 时间一律 UTC(epoch 毫秒存储);金额用十进制定点,禁止浮点累计。
//! - 密钥与 Authorization 不进入本层任何结构。

pub mod ids;
pub mod money;
pub mod plan;
pub mod tree;

pub use ids::*;
pub use money::{CostSource, Pricing, UsageRecord, UsageStatus};
pub use plan::*;
pub use tree::*;

/// 领域错误。
#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("消息 {0} 不存在")]
    MessageNotFound(MessageId),
    #[error("会话 {0} 不存在")]
    ConversationNotFound(ConversationId),
    #[error("父子消息必须属于同一会话")]
    CrossConversationParent,
    #[error("消息树出现环:父链回到 {0}")]
    Cycle(MessageId),
    #[error("只有已完成的回答可用于继续")]
    NotCompleted,
    #[error("无效参数: {0}")]
    Invalid(String),
}

/// 生成状态机(PLAN 4.2):
/// Queued -> Connecting -> Streaming -> Completed;
/// 未终态 -> Cancelled / Failed;程序重启时遗留未终态 -> Interrupted。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationStatus {
    Queued,
    Connecting,
    Streaming,
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

impl GenerationStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Connecting => "connecting",
            Self::Streaming => "streaming",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "connecting" => Self::Connecting,
            "streaming" => Self::Streaming,
            "completed" => Self::Completed,
            "cancelled" => Self::Cancelled,
            "failed" => Self::Failed,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }

    /// 终态(重启后不再迁移,除非显式重试产生新请求)。
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Failed | Self::Interrupted
        )
    }
}

/// 消息角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            _ => return None,
        })
    }
}
