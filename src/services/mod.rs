//! 服务层:发送调度(ChatService)、出口门禁、网络配置、上下文、
//! 目录、费用、导出。
//!
//! 职责约束(PLAN 2):UI 不直接拼 API 请求或写 SQL;异步生成事件
//! 经 channel 回 UI;SQLite 单写队列;请求先落库再发出。

pub mod chat;
pub mod egress;
pub mod export;
pub mod network;
pub mod title;

pub use chat::{AdmissionError, ChatEvent, ChatService, SubmitError, SubmitReceipt, Throttle};
