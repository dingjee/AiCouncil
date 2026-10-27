//! AiCouncil:极简原生 AI 聊天(多模型并行分支)——Tauri 2 macOS 版。
//!
//! 模块结构:
//!   `domain`    — ID、消息树、发送计划、状态、金额
//!   `providers` — OpenRouter / OpenAI 兼容适配、SSE、目录、DeepSeek 标题
//!   `services`  — 发送调度、出口门禁、网络、标题、导出
//!   `storage`   — SQLite、配置、凭据
//!   `composer`  — composer 草稿协调(单一写入点;DB 列唯一写入者)
//!   `desktop`   — Tauri 命令/DTO/状态装配与事件转发

pub mod composer;
pub mod desktop;
pub mod domain;
pub mod providers;
pub mod services;
pub mod storage;

/// 原生冒烟 harness(GLM 所有;debug-only,release 不编译)。
#[cfg(debug_assertions)]
pub mod native_smoke;
