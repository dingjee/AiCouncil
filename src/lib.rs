//! AiCouncil:极简原生 AI 聊天(多模型并行分支)。
//!
//! 模块结构见 PLAN.md 第 2 节:
//!   [`render`] — Markdown/公式解析与原生排版(P0)
//!   `domain`   — ID、消息树、发送计划、状态、金额(P1)
//!   `providers` — OpenRouter / OpenAI 兼容适配、SSE(P2)
//!   `services` — 发送调度、上下文、目录、费用(P1-P5)
//!   `storage`  — SQLite、配置、凭据(P1)
//!   `ui`       — 聊天、右侧栏、分组、模型管理、设置(P3+)
//!   `app`      — UI 状态与后台事件桥接(P3+)

pub mod app;
pub mod domain;
pub mod providers;
pub mod render;
pub mod services;
pub mod storage;
/// 共享 UI 令牌(render 与 app 共同依赖;禁止 render → app)。
pub mod ui_style;
