//! Tauri 桌面装配:状态、服务构建、事件转发、窗口/菜单。
//!
//! 生命周期:setup 时构建 [`Desktop`] 并 `manage`;服务在整个应用存活期
//! 有效(切会话不重建);`reload_credentials` 在无活动请求时重建提供方
//! 状态(`.env` 为唯一凭据源,见 `env_keys`)。
//!
//! 事件通道为进程级单例(`OnceLock`):服务重建(reload)前后
//! 发送端不变,转发任务只需订阅一次,重建后事件不断流。

pub mod commands;
pub mod dto;
pub mod env_keys;
pub mod smoke;

use std::sync::{Arc, Mutex, OnceLock};

use tauri::{AppHandle, Emitter, Manager};

use crate::composer::DraftStore;
use crate::domain::{ConversationId, GenerationId, MessageId, MessageTree, ProviderId};
use crate::providers::catalog::{CatalogCache, Preferences};
use crate::providers::openrouter::OpenRouter;
use crate::providers::Provider;
use crate::services::chat::ChatService;
use crate::services::egress::{EgressPermit, GeoIpGuard};
use crate::services::network::NetworkConfig;
use crate::services::title::{TitleEvent, TitleService};
use crate::services::ChatEvent;
use crate::storage::repo::{self, AppConfig};
use crate::storage::Db;

/// 内置 OpenRouter 提供方 ID(与既有数据兼容)。
pub const PROVIDER_ID: &str = "00000000-0000-4000-8000-0000000000a1";

pub fn provider_id() -> ProviderId {
    ProviderId::from_uuid_str(PROVIDER_ID)
}

fn chat_events_sender() -> tokio::sync::broadcast::Sender<ChatEvent> {
    static SENDER: OnceLock<tokio::sync::broadcast::Sender<ChatEvent>> = OnceLock::new();
    SENDER
        .get_or_init(|| tokio::sync::broadcast::channel::<ChatEvent>(1024).0)
        .clone()
}

fn title_events_sender() -> tokio::sync::broadcast::Sender<TitleEvent> {
    static SENDER: OnceLock<tokio::sync::broadcast::Sender<TitleEvent>> = OnceLock::new();
    SENDER
        .get_or_init(|| tokio::sync::broadcast::channel::<TitleEvent>(64).0)
        .clone()
}

/// 冒烟结果回报(debug-only 落盘;release 为空操作)。
#[tauri::command(rename_all = "snake_case")]
async fn smoke_report(payload: serde_json::Value) -> Result<(), String> {
    smoke::write_report(&payload);
    Ok(())
}

/// 桌面端共享状态(Tauri managed state)。
pub struct Desktop {
    pub db: Arc<Db>,
    pub config: Mutex<AppConfig>,
    pub drafts: Mutex<DraftStore>,
    pub catalog: Arc<CatalogCache>,
    pub prefs: Mutex<Preferences>,
    pub chat: Mutex<Arc<ChatService<OpenRouter>>>,
    pub title: Mutex<Arc<TitleService>>,
    pub egress: Arc<GeoIpGuard>,
    /// 近期 egress 许可(仅批内 TTL 有效;send 时服务侧仍校验)。
    pub permit: Mutex<Option<EgressPermit>>,
    pub app: AppHandle,
}

impl Desktop {
    /// 构建提供方客户端(测试/mock 可用 AICOUNCIL_BASE_URL 覆盖)。
    pub(crate) fn build_provider(key: &str) -> Arc<OpenRouter> {
        let pid = provider_id();
        match std::env::var("AICOUNCIL_BASE_URL") {
            Ok(url) if !url.trim().is_empty() => {
                Arc::new(OpenRouter::with_base_url(pid, url, key.to_string()))
            }
            _ => Arc::new(OpenRouter::new(pid, key.to_string())),
        }
    }

    fn build_chat(
        db: &Arc<Db>,
        key: &str,
        max_concurrent: usize,
        guard: &Arc<GeoIpGuard>,
    ) -> Arc<ChatService<OpenRouter>> {
        let provider = Self::build_provider(key);
        Arc::new(
            ChatService::new(provider, db.clone(), chat_events_sender(), max_concurrent)
                .with_egress_guard(guard.clone()),
        )
    }

    fn build_title(db: &Arc<Db>) -> Arc<TitleService> {
        let client = crate::providers::deepseek::DeepSeekTitleClient::from_env();
        TitleService::new(db.clone(), client, title_events_sender())
    }

    /// 当前活动(非终态)generation 数(reload 重建前必须为零)。
    fn active_generation_count(db: &Db) -> usize {
        let Ok(convs) = db.conversations() else {
            return usize::MAX; // 保守:拒绝重建
        };
        let mut n = 0;
        for c in convs {
            if let Ok(gs) = db.generations_of(c.id) {
                n += gs.iter().filter(|g| !g.status.is_terminal()).count();
            }
        }
        n
    }

    /// 应用启动装配。失败返回错误字符串(setup 失败,应用不启动)。
    pub fn bootstrap(app: AppHandle) -> Result<Desktop, String> {
        let _data_dir = repo::data_dir().map_err(|e| format!("data dir: {e}"))?;
        let db = Arc::new(
            crate::storage::open(&repo::db_path().map_err(|e| format!("db path: {e}"))?)
                .map_err(|e| format!("open db: {e}"))?,
        );
        // 启动恢复:中断的 generation / 在途标题。
        let _ = db.recover_interrupted();
        let _ = db.recover_title_in_flight();

        let config = AppConfig::load_or_default().map_err(|e| format!("config: {e}"))?;

        // 事件通道(进程级单例;先取发送端并订阅,再构建服务)。
        let chat_tx = chat_events_sender();
        let chat_rx = chat_tx.subscribe();
        let title_tx = title_events_sender();
        let title_rx = title_tx.subscribe();

        // 目录缓存:先从持久层恢复,再后台刷新。
        let catalog = Arc::new(CatalogCache::new(provider_id()));
        let stored = db.catalog(provider_id()).unwrap_or_default();
        if !stored.is_empty() {
            let at = db
                .catalog_updated_at(provider_id())
                .ok()
                .flatten()
                .unwrap_or_else(crate::storage::now_dt);
            catalog.replace(
                crate::providers::CatalogSnapshot {
                    provider_id: provider_id(),
                    entries: stored,
                    fetched_at: at,
                },
                at,
            );
        }

        // 偏好。
        let mut prefs = Preferences::default();
        for (model, pref) in db.preferences(provider_id()).unwrap_or_default() {
            prefs.set(provider_id(), &model, pref);
        }

        let network = NetworkConfig {
            proxy: crate::services::network::explicit_proxy_from_env(),
            ..Default::default()
        };
        let guard = Arc::new(GeoIpGuard::new(&network).map_err(|e| format!("egress guard: {e}"))?);

        let openrouter_key =
            env_keys::load_named_key(env_keys::OPENROUTER_ENV_NAME).unwrap_or_default();
        let max_concurrent = config.general.max_concurrent_requests.max(1) as usize;
        let chat = Self::build_chat(&db, &openrouter_key, max_concurrent, &guard);
        let title = Self::build_title(&db);

        // 后台补查:重启前中断请求的 usage 收尾(不阻塞启动)。
        let repair_chat = chat.clone();
        tauri::async_runtime::spawn(async move {
            let _ = repair_chat.repair_pending_usage().await;
        });

        // 草稿镜像 + pending-new 槽位校验。
        let mut drafts = DraftStore::new(db.clone());
        let conversations = db.conversations().unwrap_or_default();
        let db_for_unsent = db.clone();
        drafts.load_on_start(&conversations, &move |id| {
            db_for_unsent
                .messages_of(id)
                .map(|m| m.is_empty())
                .unwrap_or(false)
        });

        let desktop = Desktop {
            db,
            config: Mutex::new(config),
            drafts: Mutex::new(drafts),
            catalog: catalog.clone(),
            prefs: Mutex::new(prefs),
            chat: Mutex::new(chat),
            title: Mutex::new(title),
            egress: guard,
            permit: Mutex::new(None),
            app: app.clone(),
        };

        spawn_forwarders(&app, chat_rx, title_rx);
        spawn_catalog_refresh(&app, catalog);

        Ok(desktop)
    }

    /// 依据当前 `.env` 重建提供方状态(凭据 reload)。
    /// 有活动请求时拒绝(前端应提示稍后重试)。
    pub fn reload_providers(&self) -> Result<(), String> {
        if Self::active_generation_count(&self.db) > 0 {
            return Err("requests are in flight; retry after they finish".to_string());
        }
        // 与新生成客户端读取同一网络策略快照:同步重建检测客户端,
        // 避免检测(旧客户端)与生成(新代理)策略分裂;
        // rebuild 成功即 bump revision → 既有许可全部失效。
        let network = NetworkConfig {
            proxy: crate::services::network::explicit_proxy_from_env(),
            ..Default::default()
        };
        self.egress
            .rebuild_clients(&network)
            .map_err(|e| format!("egress guard rebuild: {e}"))?;
        let key = env_keys::load_named_key(env_keys::OPENROUTER_ENV_NAME).unwrap_or_default();
        let max_concurrent = {
            let cfg = self.config.lock().unwrap_or_else(|p| p.into_inner());
            cfg.general.max_concurrent_requests.max(1) as usize
        };
        // 同一广播发送端:新服务替换旧服务,转发任务无感。
        let chat = Self::build_chat(&self.db, &key, max_concurrent, &self.egress);
        *self.chat.lock().unwrap_or_else(|p| p.into_inner()) = chat;
        let title = Self::build_title(&self.db);
        *self.title.lock().unwrap_or_else(|p| p.into_inner()) = title;
        // 旧许可一律不复用(策略可能已随 reload 变化)。
        *self.permit.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(())
    }

    pub fn emit_conversations_changed(&self) {
        let _ = self
            .app
            .emit("conversations-changed", serde_json::json!({}));
    }

    pub fn emit_conversation_changed(&self, conversation_id: ConversationId) {
        let _ = self.app.emit(
            "conversation-changed",
            serde_json::json!({ "conversation_id": conversation_id.as_str() }),
        );
    }

    pub fn emit_catalog_changed(&self) {
        let state = commands::catalog_state(self);
        let _ = self.app.emit("catalog-changed", state);
    }
}

fn spawn_forwarders(
    app: &AppHandle,
    mut chat_rx: tokio::sync::broadcast::Receiver<ChatEvent>,
    mut title_rx: tokio::sync::broadcast::Receiver<TitleEvent>,
) {
    let app_chat = app.clone();
    let app_title = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match chat_rx.recv().await {
                Ok(ev) => {
                    let dto = dto::ChatEventDto::from(ev);
                    let _ = app_chat.emit("chat-event", dto);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    tauri::async_runtime::spawn(async move {
        loop {
            match title_rx.recv().await {
                Ok(ev) => {
                    let outcome_ok = ev.outcome.is_ok();
                    let error_kind = ev.outcome.err().map(|k| k.as_str().to_string());
                    let conv = ev.conversation_id.as_str();
                    let dto = dto::TitleEventDto {
                        conversation_id: conv.clone(),
                        request_token: ev.request_token,
                        outcome: if outcome_ok { "ok" } else { "failed" }.to_string(),
                        error_kind,
                    };
                    let _ = app_title.emit("title-event", dto);
                    let _ = app_title.emit(
                        "conversation-changed",
                        serde_json::json!({ "conversation_id": conv }),
                    );
                    let _ = app_title.emit("conversations-changed", serde_json::json!({}));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// 启动后台目录刷新(有 key 才刷;失败保留旧数据)。
fn spawn_catalog_refresh(app: &AppHandle, catalog: Arc<CatalogCache>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // 等 managed state 就绪(setup 完成后)。
        for _ in 0..50 {
            if app.try_state::<Desktop>().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let Some(state) = app.try_state::<Desktop>() else {
            return;
        };
        let Some(key) = env_keys::load_named_key(env_keys::OPENROUTER_ENV_NAME) else {
            return;
        };
        if !catalog.needs_refresh(crate::storage::now_dt()) {
            return;
        }
        let provider = Desktop::build_provider(&key);
        catalog
            .refresh(
                || async move { provider.list_models().await },
                false,
                crate::storage::now_dt(),
            )
            .await;
        if let Some(snap) = catalog.snapshot() {
            let _ = state
                .db
                .replace_catalog(provider_id(), &snap.entries, snap.fetched_at);
        }
        state.emit_catalog_changed();
    });
}

/// 组装并运行 Tauri 应用。
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .menu(|app| {
            use tauri::menu::{MenuBuilder, PredefinedMenuItem, SubmenuBuilder};
            // macOS 应用菜单(标准 About/Services/Hide/Quit;Cmd+Q 退出)。
            let app_menu = SubmenuBuilder::new(app, "AiCouncil")
                .items(&[
                    &PredefinedMenuItem::about(app, Some("About AiCouncil"), None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::show_all(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::quit(app, None)?,
                ])
                .build()?;
            let edit = SubmenuBuilder::new(app, "Edit")
                .items(&[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ])
                .build()?;
            let window = SubmenuBuilder::new(app, "Window")
                .items(&[
                    &PredefinedMenuItem::minimize(app, None)?,
                    &PredefinedMenuItem::maximize(app, None)?,
                    &PredefinedMenuItem::fullscreen(app, None)?,
                    &PredefinedMenuItem::close_window(app, None)?,
                ])
                .build()?;
            MenuBuilder::new(app)
                .items(&[&app_menu, &edit, &window])
                .build()
        })
        .on_page_load(smoke::on_page_load)
        .invoke_handler(tauri::generate_handler![
            commands::bootstrap,
            commands::get_settings,
            commands::update_settings,
            commands::check_egress,
            commands::list_conversations,
            commands::load_conversation,
            commands::create_conversation,
            commands::rename_conversation,
            commands::delete_conversation,
            commands::set_conversation_group,
            commands::set_system_prompt,
            commands::set_main_leaf,
            commands::create_group,
            commands::rename_group,
            commands::delete_group,
            commands::get_draft,
            commands::save_draft,
            commands::save_edit_buffer,
            commands::clear_edit_buffer,
            commands::refresh_catalog,
            commands::list_preferences,
            commands::set_model_preference,
            commands::set_primary_model,
            commands::check_key_status,
            commands::send_message,
            commands::cancel_request,
            commands::retry_request,
            commands::remove_message,
            commands::export_markdown,
            commands::export_markdown_to_file,
            commands::credential_status,
            commands::reload_credentials,
            smoke_report,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            match Desktop::bootstrap(handle.clone()) {
                Ok(state) => {
                    app.manage(state);
                    // debug-only:页面加载回调未触发时兜底重注入冒烟脚本。
                    smoke::spawn_fallback_injection(handle);
                    Ok(())
                }
                Err(e) => {
                    tracing::error!("bootstrap failed: {e}");
                    Err(Box::new(std::io::Error::other(format!(
                        "AiCouncil failed to start: {e}"
                    ))))
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------- 供命令层使用的公共助手 ----------

/// 会话消息树(展示/计划/导出共用)。
pub fn tree_of(db: &Db, conversation: ConversationId) -> MessageTree {
    let messages = db.messages_of(conversation).unwrap_or_default();
    MessageTree::from_messages(messages).0
}

/// “编辑最新问题”定位 + 资格(主分支最深可见 user 消息,其 lineage 无活动请求)。
pub fn edit_latest_for(
    conversation: &crate::domain::Conversation,
    tree: &MessageTree,
    generations: &[crate::domain::Generation],
) -> Option<dto::EditLatestDto> {
    let leaf = conversation.main_leaf_id?;
    let path = tree.path_from_root(leaf).ok()?;
    let target = path.iter().rev().find(|id| {
        tree.get(**id)
            .map(|m| m.role == crate::domain::Role::User)
            .unwrap_or(false)
    })?;
    let active = generations
        .iter()
        .any(|g| g.user_message_id == *target && !g.status.is_terminal());
    if active {
        return None;
    }
    let msg = tree.get(*target)?;
    Some(dto::EditLatestDto {
        target_id: target.as_str(),
        content: msg.content.clone(),
        attachments: msg
            .attachments
            .iter()
            .map(dto::AttachmentDto::from)
            .collect(),
    })
}

/// 由目录 + 偏好合成目标模型(参数/价格快照;不在目录则 Auto)。
pub fn model_target_with(
    cache: &CatalogCache,
    prefs: &Preferences,
    pid: ProviderId,
    model_id: &str,
) -> crate::domain::plan::ModelTarget {
    let entry = cache.entries().into_iter().find(|e| e.model_id == model_id);
    let pref = prefs.get(pid, model_id);
    let (params, _) = match &entry {
        Some(e) => crate::providers::catalog::compose_params(e, &pref),
        None => (None, Vec::new()),
    };
    crate::domain::plan::ModelTarget {
        provider_id: pid,
        model_id: model_id.to_string(),
        params,
        price: entry.map(|e| e.pricing),
    }
}

// 便捷 ID 解析(crate 内命令层共用)。
pub(crate) fn parse_conversation_id(s: &str) -> Result<ConversationId, dto::IpcError> {
    dto::parse_id!(ConversationId, s)
}

pub(crate) fn parse_message_id(s: &str) -> Result<MessageId, dto::IpcError> {
    dto::parse_id!(MessageId, s)
}

pub(crate) fn parse_generation_id(s: &str) -> Result<GenerationId, dto::IpcError> {
    dto::parse_id!(GenerationId, s)
}

pub(crate) fn parse_provider_id(s: &str) -> Result<ProviderId, dto::IpcError> {
    dto::parse_id!(ProviderId, s)
}
