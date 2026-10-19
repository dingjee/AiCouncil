//! 极简原生 UI:聊天主区 + 右侧可折叠侧栏(会话/模型/设置)。
//!
//! 本轮架构(PLAN 5-8):
//! - [`design`]/[`crate::ui_style`]:Soft Sage 语义 token 与控件样式;
//! - [`i18n`]:全部 UI 文案英文资源(默认 en);
//! - [`scroll`]:基于实际视口的跟随/阅读状态机(不按 dy 猜);
//! - **统一模型选择器**:单一状态源(单选/同题多模型同一入口);
//! - **正文局部选择**:消息流级 SelectionCoordinator,纯文本投影 +
//!   公式原文复制(render::document);
//! - **自动标题**:后台 TitleService(DeepSeek;`.env` 读 key),
//!   字段级 CAS 写库,手动改名始终优先;
//! - **右栏宽度**:拖拽分隔条,config 持久化(双击复位 300);
//! - Composer:动态高度(默认三行,至多 min(220, 0.30×窗高)),
//!   稳定 `text_editor::Content`(仅切换会话/发送后重建)。
//!
//! 事件桥接:ChatService broadcast + TitleService broadcast 经
//! Subscription 进入 update;按 (conversation_id, request_id) 更新。

mod design;
pub mod i18n;
mod icons;
pub mod model_picker;
pub mod scroll;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use iced::widget::{
    button as iced_button, checkbox, column, container, mouse_area, row, scrollable, slider, text,
    text_editor, text_input, Space,
};
use iced::{Element, Length, Padding, Subscription, Task};

use crate::domain::model_identity::presentation;
use crate::domain::plan::{ContinuationSelection, ModelTarget, Planner};
use crate::domain::recovery::{retry_decision, FailureInfo, RetryDecision};
use crate::domain::{
    is_anthropic, Attachment, Conversation, ConversationId, Generation, GenerationId,
    GenerationStatus, Group, GroupId, MessageId, MessageTree, Role,
};
use crate::providers::cancel::CancellationToken;
use crate::providers::catalog::{compose_params, ModelPreference};
use crate::providers::deepseek::DeepSeekTitleClient;
use crate::providers::openrouter::OpenRouter;
use crate::providers::{CatalogEntry, GenEvent, KeyStatus};
use crate::render::document::{DocPos, PathSeg, Selection as DocSelection};
use crate::render::flow::{SelPos, SelectionHooks};
use crate::render::{self, math::MathCache, view, RenderStyle, RenderedMessage};
use crate::services::egress::{EgressGuard, EgressObservation, EgressPermit, EgressVerdict};
use crate::services::export::{self, ExportScope};
use crate::services::title::{TitleEvent, TitleService};
use crate::services::{ChatEvent, ChatService, SubmitReceipt};
use crate::storage::credentials::CredentialVault;
use crate::storage::{self, Db};

const PROVIDER_KIND: &str = "openrouter";
const OPENROUTER_ID: &str = "00000000-0000-4000-8000-0000000000a1";
/// 消息滚动区 widget id。
const MESSAGES_ID: &str = "messages";
/// Composer widget id(复制焦点判定)。
const COMPOSER_ID: &str = "composer";
/// 折叠视口高度(约 6 行 × 1.55b;b=15 时 ≈140)。
const COLLAPSED_HEIGHT: f32 = 140.0;
/// 折叠判定:字符数阈值(行数为辅;按实际布局高度判定列为已知限制)。
const COLLAPSE_CHARS: usize = 320;
const COLLAPSE_LINES: usize = 8;
/// 侧栏宽度边界(8)。
const SIDEBAR_MIN: f32 = 240.0;
const SIDEBAR_MAX: f32 = 480.0;
const SIDEBAR_DEFAULT: f32 = 300.0;
/// 主区最小有效宽度(窄窗口并行卡改纵向)。
const MAIN_MIN_WIDTH: f32 = 520.0;
/// Composer 高度边界(5.1)。
const COMPOSER_MIN: f32 = 76.0;
const COMPOSER_MAX: f32 = 220.0;

pub fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aicouncil=info".into()),
        )
        .init();

    if let Err(error) = storage::repo::data_dir() {
        let message = i18n::f("dialog.data_dir.body", &[("error", &error.to_string())]);
        let _ = rfd::MessageDialog::new()
            .set_title(i18n::t("dialog.data_dir.title"))
            .set_description(message)
            .set_level(rfd::MessageLevel::Error)
            .show();
        return Err(iced::Error::WindowCreationFailed(Box::new(error)));
    }
    iced::application(App::boot, App::update, App::view)
        .title(|_: &App| i18n::t("app.title").to_string())
        .window(iced::window::Settings {
            size: iced::Size::new(1150.0, 680.0),
            // 窄于此宽度时侧栏会挤掉输入区工具栏(发送按钮不可见)。
            min_size: Some(iced::Size::new(880.0, 520.0)),
            icon: window_icon(),
            ..Default::default()
        })
        .theme(|_: &App| design::theme())
        .subscription(App::subscription)
        .run()
}

/// 窗口图标:assets/icons/logo.png(构建期由 gen_icon 从 logo.svg 生成)。
fn window_icon() -> Option<iced::window::Icon> {
    const PNG: &[u8] = include_bytes!("../../assets/icons/logo.png");
    let decoder = png::Decoder::new(std::io::Cursor::new(PNG));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    iced::window::icon::from_rgba(buf[..info.buffer_size()].to_vec(), info.width, info.height).ok()
}

/// 文件选择结果:可读文本文件 + 逐文件错误(非 UTF-8/读取失败)。
#[derive(Debug, Clone)]
pub struct PickedAttachments {
    files: Vec<Attachment>,
    errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    // 事件桥
    Provider(ChatEvent),
    /// submit/retry 回执(成功/失败统一入口;失败保留草稿)。
    SendResult(ConversationId, Result<SubmitReceipt, String>),
    /// 重试回执(定位原卡,清理 UI 防重标记)。
    RetryOutcome(ConversationId, MessageId, Result<SubmitReceipt, String>),
    /// 出口门禁预检结果(intent 校验;过期结果不提交)。
    EgressChecked(u64, Result<EgressVerdict, String>),
    /// 弹窗"重新检测"结果(只更新弹窗,不自动发送)。
    EgressRecheckResult(Result<EgressVerdict, String>),
    /// 门禁弹窗动作(重新检测/取消/移除 Anthropic 后发送)。
    EgressModal(EgressModalAction),
    /// broadcast lag 后的 DB 对账。
    Reconcile,
    TitleFinished(TitleEvent),
    Tick,
    // 输入与发送
    InputEdited(text_editor::Action),
    Send,
    Stop(GenerationId),
    Retry(MessageId),
    Copy(MessageId),
    /// 展开/收起旧重试尝试(会话级)。
    ToggleAttempts,
    // 滚动(视口驱动;替代旧 WheelScrolled dy 判定)
    Viewport {
        content: f32,
        view: f32,
        offset: f32,
    },
    BackToBottom,
    // 正文局部选择(消息流级协调)
    SelStart(MessageId, Vec<PathSeg>, SelPos),
    SelExtend(MessageId, Vec<PathSeg>, SelPos),
    SelRange(MessageId, Vec<PathSeg>, SelPos, SelPos),
    SelFinish,
    /// Ctrl+C:先查 composer 焦点再决定写谁。
    CheckCopy,
    CopySelection {
        composer_focused: bool,
    },
    ClearSelection,
    // 模型选择器(单一状态源)
    TogglePicker,
    ToggleParallelMode,
    ToggleChecked(String),
    SetPrimary(String),
    PickSingle(String),
    PickerSearchChanged(String),
    // 会话与分组
    NewConversation,
    SelectConversation(ConversationId),
    RenameStart(ConversationId),
    RenameInput(String),
    RenameApply,
    RenameCancel,
    DeleteConversation(ConversationId),
    NewGroupInput(String),
    NewGroupApply,
    RenameGroupStart(GroupId),
    RenameGroupInput(GroupId, String),
    RenameGroupApply,
    DeleteGroup(GroupId),
    MoveToGroup(ConversationId, Option<GroupId>),
    ConvSearchChanged(String),
    // 面板与侧栏
    Panel(Panels),
    ToggleSidebar,
    SidebarPress,
    SidebarDrag(f32),
    SidebarRelease,
    SidebarReset,
    ToggleExportMenu,
    ToggleUsage,
    // 模型管理
    KeyInput(String),
    SaveKey,
    TestConnection,
    RefreshCatalog,
    CatalogLoaded(Result<Vec<CatalogEntry>, String>),
    TestConn(Result<KeyStatus, String>),
    KeyStatusDone(Result<KeyStatus, String>),
    ToggleModel(String),
    SetDefault(String),
    MoveModel(String, i64),
    ModelSearchChanged(String),
    // 导出
    Export(ExportScope),
    ExportPathChosen(Option<std::path::PathBuf>),
    ExportDone(Result<String, String>),
    // 其他
    LinkClicked(String),
    ToggleBranchSelect(MessageId),
    ContinueFrom(MessageId),
    ToggleExpand(MessageId),
    PickAttachments,
    AttachLoaded(PickedAttachments),
    RemoveAttachment(usize),
    FontSize(f32),
    WindowResized(iced::Size),
    KeyPressed(iced::keyboard::Key, iced::keyboard::Modifiers),
    ConfigSaved(Result<(), String>),
    E2eAutopilot,
    E2eStage2,
    Nop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panels {
    History,
    Models,
    Settings,
}

/// 流式请求的 UI 侧状态。
#[derive(Clone)]
struct StreamState {
    request_id: GenerationId,
    model: String,
    status: GenerationStatus,
    error: Option<String>,
}

/// 门禁弹窗动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EgressModalAction {
    /// 只重新检测,不自动发送;确认可用后回到编辑区由用户发送。
    Recheck,
    /// 取消(Esc 同效;避免 modal 堆叠)。
    Dismiss,
    /// 移除全部 Anthropic 目标后发送(用户显式发起的新动作,重新冻结计划)。
    RemoveAnthropicAndSend,
}

/// 一份发送意图:绑定其草稿 revision(编辑/换模型/切会话使旧 intent 失效)。
#[derive(Clone)]
struct PendingSend {
    conv: ConversationId,
    plan: crate::domain::SendPlan,
    /// 提交时的草稿内容(回执只清理已提交 revision,不清除新内容)。
    submitted_text: String,
    submitted_attachments: Vec<Attachment>,
    intent: u64,
}

/// Anthropic 出口门禁弹窗(单弹窗;按 intent 去重,不堆叠)。
struct EgressModal {
    unknown: bool,
    observation: Option<EgressObservation>,
    detail: String,
    /// 混合目标(含非 Anthropic)时提供"移除 Anthropic 模型后发送"。
    mixed: bool,
    checking: bool,
}

/// 统一模型选择状态(PLAN 6:单一状态源)。
#[derive(Debug, Default, Clone)]
struct ModelSelectionState {
    /// 同题多模型(并行)模式。
    parallel: bool,
    /// 选择器面板展开(单选/复选同一入口)。
    picker_open: bool,
    /// 并行勾选集(严格按勾选;primary 必属于其中)。
    checked: Vec<String>,
    /// 主模型(主分支归属)。
    primary: Option<String>,
}

/// 归一化选择快照(Send disabled 状态/费用预估/发送计划共用)。
#[derive(Debug, Clone, PartialEq)]
struct NormalizedSelection {
    /// effective targets(primary 在首位)。
    targets: Vec<String>,
    primary: Option<String>,
    /// Send disabled 原因(i18n key)。
    invalid_reason: Option<&'static str>,
}

/// 纯函数 resolver(6 节行为表;T04)。
fn resolve_selection(
    enabled: &[String],
    usage_rank: &[String],
    sel: &ModelSelectionState,
) -> NormalizedSelection {
    if enabled.is_empty() {
        return NormalizedSelection {
            targets: vec![],
            primary: None,
            invalid_reason: Some("composer.send_disabled_no_model"),
        };
    }
    if !sel.parallel {
        // 单模型:primary ∈ enabled,否则列表第一项
        let primary = sel
            .primary
            .clone()
            .filter(|p| enabled.contains(p))
            .or_else(|| enabled.first().cloned());
        NormalizedSelection {
            targets: primary.clone().into_iter().collect(),
            primary,
            invalid_reason: None,
        }
    } else {
        // 并行:严格按勾选 ∩ 有效集(勾选顺序按面板稳定顺序)
        let mut checked: Vec<String> = enabled
            .iter()
            .filter(|m| sel.checked.contains(m))
            .cloned()
            .collect();
        if checked.is_empty() {
            return NormalizedSelection {
                targets: vec![],
                primary: None,
                invalid_reason: Some("composer.send_disabled_no_targets"),
            };
        }
        // primary 必属于 checked;缺失则按“最近使用 > 面板顺序”提升
        let primary = sel
            .primary
            .clone()
            .filter(|p| checked.contains(p))
            .or_else(|| {
                usage_rank
                    .iter()
                    .find(|m| checked.contains(m))
                    .cloned()
                    .or_else(|| checked.first().cloned())
            });
        if let Some(p) = &primary {
            if let Some(pos) = checked.iter().position(|m| m == p) {
                checked.remove(pos);
                checked.insert(0, p.clone());
            }
        }
        NormalizedSelection {
            targets: checked,
            primary,
            invalid_reason: None,
        }
    }
}

/// 打开并行时的默认两项:当前模型 + 用量排名中最靠前的另一可用模型;
/// 无历史记录时按面板顺序取第一项(6 节:不使用随机顺序)。
fn default_parallel_pair(
    enabled: &[String],
    usage_rank: &[String],
    current: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut checked: Vec<String> = Vec::new();
    if let Some(c) = current {
        if enabled.iter().any(|m| m == c) {
            checked.push(c.to_string());
        }
    }
    for m in usage_rank {
        if enabled.iter().any(|e| e == m) && !checked.contains(m) {
            checked.push(m.clone());
            break;
        }
    }
    if checked.len() < 2 {
        // 第二模型兜底:面板顺序第一个未勾选项
        if let Some(m) = enabled.iter().find(|m| !checked.contains(m)) {
            checked.push(m.clone());
        }
    }
    if checked.is_empty() {
        checked = enabled.first().cloned().into_iter().collect();
    }
    let primary = checked.first().cloned();
    (checked, primary)
}

/// 长 user 消息折叠判定(字符/行启发式;按实际布局高度判定列为
/// 已知限制,见 docs/RENDERING.md)。
fn should_collapse(content: &str) -> bool {
    content.chars().count() > COLLAPSE_CHARS || content.lines().count() > COLLAPSE_LINES
}

#[derive(Clone)]
struct EventBus(Arc<tokio::sync::broadcast::Sender<ChatEvent>>);

impl std::hash::Hash for EventBus {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        0u8.hash(state);
    }
}

#[derive(Clone)]
struct TitleBus(Arc<tokio::sync::broadcast::Sender<TitleEvent>>);

impl std::hash::Hash for TitleBus {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        0u8.hash(state);
    }
}

pub struct App {
    db: Arc<Db>,
    vault: Arc<CredentialVault>,
    chat: Arc<ChatService<OpenRouter>>,
    provider: Arc<OpenRouter>,
    bus: EventBus,
    title_bus: TitleBus,
    titles: Arc<TitleService>,
    event_tx: Arc<tokio::sync::broadcast::Sender<ChatEvent>>,
    provider_id: crate::domain::ProviderId,

    conversations: Vec<Conversation>,
    groups: Vec<Group>,
    trees: HashMap<ConversationId, MessageTree>,
    generations: HashMap<ConversationId, Vec<Generation>>,
    usage_cache: HashMap<ConversationId, Vec<crate::domain::UsageRecord>>,
    aux_usage_cache: HashMap<ConversationId, (usize, usize)>,
    current: Option<ConversationId>,

    renders: HashMap<MessageId, RenderedMessage>,
    math_cache: MathCache,
    style: RenderStyle,
    streaming: HashMap<MessageId, StreamState>,
    /// 流式脏消息集合:一次 tick 全部刷新(替代旧单 ID 覆盖,PLAN 5.2)。
    dirty_stream: HashSet<MessageId>,

    // 配置(PLAN 3.1/3.2/8:general settings + window)
    config: storage::repo::AppConfig,
    window_size: (f32, f32),
    sidebar_dragging: bool,

    panel: Panels,
    conv_search: String,
    renaming_conv: Option<(ConversationId, String)>,
    new_group_name: String,
    renaming_group: Option<(GroupId, String)>,

    catalog: Vec<CatalogEntry>,
    prefs: HashMap<String, ModelPreference>,
    key_input: String,
    key_saved: bool,
    models_status: String,
    key_status_detail: String,
    export_menu_open: bool,
    usage_expanded: bool,
    model_search: String,
    /// 模型用量排名缓存(启动/成功提交/Started 后刷新;不在 view 查库)。
    usage_rank: Vec<String>,

    editor: text_editor::Content,
    model_sel: ModelSelectionState,
    /// 多支追问:已勾选的 assistant 叶子。
    branch_selection: Vec<MessageId>,
    /// 输入区待发送附件(随会话草稿保存/恢复)。
    pending_attachments: Vec<Attachment>,
    /// 长 user 消息展开状态(会话内临时;重启默认折叠)。
    expanded_user: HashMap<ConversationId, HashSet<MessageId>>,

    // 正文局部选择
    selection: Option<(MessageId, DocSelection)>,
    selecting: bool,
    copy_snapshot: Option<String>,

    toast: Option<String>,
    /// 存储/门禁类错误提示(错误色,与成功 toast 区分)。
    toast_error: Option<String>,
    pending_export: Option<ExportScope>,
    tick_count: u32,
    scroll: scroll::ScrollController,

    // 请求恢复/门禁(P2/P3)
    guard: Arc<dyn EgressGuard>,
    egress_modal: Option<EgressModal>,
    pending_send: Option<PendingSend>,
    intent_seq: u64,
    /// 重试点击防重(处理 Retry 消息第一步置位;服务事务+索引兜底)。
    retrying: HashSet<MessageId>,
    /// 选择器搜索(关闭选择器后清理)。
    picker_search: String,
    /// 展示旧重试尝试(会话级;默认只显示最新一次)。
    show_attempts: HashSet<ConversationId>,
}

impl App {
    fn boot() -> (Self, Task<Message>) {
        let db_path = storage::repo::db_path().unwrap_or_else(|e| {
            let _ = rfd::MessageDialog::new()
                .set_title(i18n::t("dialog.db.title"))
                .set_description(e.to_string())
                .set_level(rfd::MessageLevel::Error)
                .show();
            std::process::exit(1);
        });
        let db = match storage::open(&db_path) {
            Ok(db) => Arc::new(db),
            Err(e) => {
                let _ = rfd::MessageDialog::new()
                    .set_title(i18n::t("dialog.db.title"))
                    .set_description(e.to_string())
                    .set_level(rfd::MessageLevel::Error)
                    .show();
                std::process::exit(1);
            }
        };

        let provider_id = crate::domain::ProviderId::from_uuid_str(OPENROUTER_ID);
        let _ = db.ensure_provider(
            provider_id,
            PROVIDER_KIND,
            crate::providers::openrouter::DEFAULT_BASE_URL,
        );

        // 启动恢复:遗留未终态 → Interrupted;标题 in_flight → needs_generation
        if let Err(e) = db.recover_interrupted() {
            tracing::warn!(%e, "启动恢复失败");
        }
        if let Err(e) = db.recover_title_in_flight() {
            tracing::warn!(%e, "标题状态恢复失败");
        }

        let (event_tx, _event_rx) = tokio::sync::broadcast::channel(4096);
        let event_tx = Arc::new(event_tx);
        let (title_tx, _title_rx) = tokio::sync::broadcast::channel(64);
        let title_tx = Arc::new(title_tx);
        let vault = Arc::new(CredentialVault::new());

        let api_key = vault.load("openrouter").ok().flatten().unwrap_or_default();

        let provider = match std::env::var("AICOUNCIL_BASE_URL") {
            Ok(url) => Arc::new(OpenRouter::with_base_url(provider_id, url, api_key.clone())),
            Err(_) => Arc::new(OpenRouter::new(provider_id, api_key)),
        };
        // 统一网络配置(检测与生成;显式代理可用 AICOUNCIL_PROXY 固定出口)
        let net_config = crate::services::network::NetworkConfig {
            proxy: crate::services::network::explicit_proxy_from_env(),
            ..Default::default()
        };
        let guard: Arc<dyn EgressGuard> =
            match crate::services::egress::GeoIpGuard::new(&net_config) {
                Ok(g) => Arc::new(g),
                Err(e) => {
                    // 代理配置失败不静默直连:可见报错后进入 Unknown 门禁
                    tracing::error!(%e, "出口门禁初始化失败");
                    Arc::new(crate::services::egress::StaticGuard::new(
                        EgressVerdict::Unknown(e),
                    ))
                }
            };
        let chat = Arc::new(
            ChatService::new(provider.clone(), db.clone(), (*event_tx).clone(), 4)
                .with_egress_guard(guard.clone()),
        );

        // 配置(缺省 Soft Sage / 15px / en)
        let mut config = storage::repo::AppConfig::load_or_default().unwrap_or_default();
        let body_size = config
            .general
            .body_text_size
            .filter(|s| {
                (crate::ui_style::BODY_SIZE_MIN..=crate::ui_style::BODY_SIZE_MAX).contains(s)
            })
            .unwrap_or(crate::ui_style::BODY_SIZE_DEFAULT);

        let conversations = db.conversations().unwrap_or_default();
        let groups = db.groups().unwrap_or_default();
        let mut trees = HashMap::new();
        let mut generations = HashMap::new();
        for c in &conversations {
            let msgs = db.messages_of(c.id).unwrap_or_default();
            let (tree, orphans) = MessageTree::from_messages(msgs);
            if orphans > 0 {
                tracing::warn!(conversation = %c.id, orphans, "孤儿消息被忽略");
            }
            trees.insert(c.id, tree);
            generations.insert(c.id, db.generations_of(c.id).unwrap_or_default());
        }
        let catalog = db.catalog(provider_id).unwrap_or_default();
        let prefs: HashMap<String, ModelPreference> = db
            .preferences(provider_id)
            .unwrap_or_default()
            .into_iter()
            .collect();

        let last_conv: Option<ConversationId> = db
            .get_ui_state("current_conversation")
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(ConversationId::from_uuid_str));
        let current = last_conv
            .filter(|id| conversations.iter().any(|c| c.id == *id))
            .or_else(|| conversations.first().map(|c| c.id));

        // 标题服务(DeepSeek;无凭据也能工作:仅 fallback)
        let title_client = DeepSeekTitleClient::from_env();
        let titles = TitleService::new(db.clone(), title_client, (*title_tx).clone());

        // 侧栏宽度清洗(非法/NaN/越界回退默认;T16;写回 config 供 view 使用)
        config.window.sidebar_width = {
            let w = config.window.sidebar_width;
            if w.is_finite() && (SIDEBAR_MIN..=SIDEBAR_MAX).contains(&w) {
                w
            } else {
                SIDEBAR_DEFAULT
            }
        };

        let mut app = Self {
            provider,
            chat,
            bus: EventBus(event_tx.clone()),
            title_bus: TitleBus(title_tx.clone()),
            titles,
            event_tx,
            db,
            vault,
            provider_id,
            conversations,
            groups,
            trees,
            generations,
            usage_cache: HashMap::new(),
            aux_usage_cache: HashMap::new(),
            current,
            renders: HashMap::new(),
            math_cache: MathCache::new(),
            style: RenderStyle::soft_sage(body_size),
            streaming: HashMap::new(),
            dirty_stream: HashSet::new(),
            config,
            window_size: (1150.0, 680.0),
            sidebar_dragging: false,
            panel: match std::env::var("AICOUNCIL_DEMO_PANEL").as_deref() {
                Ok("models") if std::env::var_os("AICOUNCIL_DEMO_MODE").is_some() => Panels::Models,
                Ok("settings") if std::env::var_os("AICOUNCIL_DEMO_MODE").is_some() => {
                    Panels::Settings
                }
                _ => Panels::History,
            },
            conv_search: String::new(),
            renaming_conv: None,
            new_group_name: String::new(),
            renaming_group: None,
            catalog,
            prefs,
            key_input: String::new(),
            key_saved: false,
            models_status: String::new(),
            key_status_detail: i18n::t("models.key_not_checked").to_string(),
            export_menu_open: false,
            usage_expanded: std::env::var_os("AICOUNCIL_DEMO_MODE").is_some()
                && std::env::var_os("AICOUNCIL_DEMO_USAGE").is_some(),
            model_search: String::new(),
            usage_rank: Vec::new(),
            editor: text_editor::Content::new(),
            model_sel: ModelSelectionState::default(),
            branch_selection: Vec::new(),
            pending_attachments: Vec::new(),
            expanded_user: HashMap::new(),
            selection: None,
            selecting: false,
            copy_snapshot: None,
            toast: None,
            toast_error: None,
            pending_export: None,
            tick_count: 0,
            scroll: scroll::ScrollController::new(),
            guard,
            egress_modal: None,
            pending_send: None,
            intent_seq: 0,
            retrying: HashSet::new(),
            picker_search: String::new(),
            show_attempts: HashSet::new(),
        };
        app.model_sel.primary = app
            .prefs
            .iter()
            .find(|(_, p)| p.is_default && p.enabled)
            .map(|(m, _)| m.clone());
        app.refresh_usage_rank();

        // AICOUNCIL_DEMO_MODE(截图/演示辅助;隔离数据目录使用)
        if std::env::var_os("AICOUNCIL_DEMO_MODE").is_some() {
            if let Ok(mode) = std::env::var("AICOUNCIL_DEMO_ATTACH") {
                let demo = [
                    ("openai/gpt-4o-mini", "GPT-4o mini"),
                    ("anthropic/claude-3.5-sonnet", "Claude 3.5 Sonnet"),
                    ("google/gemini-2.0-flash", "Gemini 2.0 Flash"),
                ];
                for (i, (id, name)) in demo.iter().enumerate() {
                    app.catalog.push(CatalogEntry {
                        model_id: (*id).to_string(),
                        name: (*name).to_string(),
                        context_window: None,
                        max_output: None,
                        pricing: Default::default(),
                        capabilities: vec![],
                        server_defaults: None,
                        available: true,
                    });
                    let p = app.prefs.entry((*id).to_string()).or_default();
                    p.enabled = true;
                    p.sort_order = i as i64;
                }
                app.model_sel.primary = Some(demo[0].0.to_string());
                app.pending_attachments = vec![Attachment {
                    name: "notes.md".into(),
                    content: "- attachments are appended to the prompt\n".repeat(3),
                }];
                if mode == "parallel" {
                    let enabled = app.enabled_models();
                    let (checked, primary) = default_parallel_pair(
                        &enabled,
                        &app.usage_rank,
                        app.model_sel.primary.as_deref(),
                    );
                    app.model_sel.parallel = true;
                    app.model_sel.checked = checked;
                    app.model_sel.primary = primary;
                    app.model_sel.picker_open = true;
                }
            }
        }

        // 草稿恢复
        if let Some(id) = app.current {
            if let Some(c) = app.conversations.iter().find(|c| c.id == id) {
                app.editor = text_editor::Content::with_text(&c.draft);
                app.pending_attachments = c.draft_attachments.clone();
            }
        }

        // 渲染当前会话主分支
        app.rerender_current();
        app.scroll
            .set_current_conversation(app.current.unwrap_or_default());

        let mut tasks = vec![
            Task::perform(fetch_catalog(app.provider.clone()), |r| match r {
                Ok(entries) => Message::CatalogLoaded(Ok(entries)),
                Err(e) => Message::CatalogLoaded(Err(e)),
            }),
            Task::perform(test_key(app.provider.clone()), Message::KeyStatusDone),
            // AICOUNCIL_E2E(端到端辅助;不改变正常路径)
        ];
        if std::env::var("AICOUNCIL_E2E").is_ok() {
            tasks.push(Task::perform(
                async {
                    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
                },
                |()| Message::E2eAutopilot,
            ));
            if std::env::var("AICOUNCIL_E2E").as_deref() == Ok("multi2") {
                tasks.push(Task::perform(
                    async {
                        tokio::time::sleep(std::time::Duration::from_millis(9000)).await;
                    },
                    |()| Message::E2eStage2,
                ));
            }
        }
        let task = Task::batch(tasks);
        (app, task)
    }

    // ----- 派生查询 -----

    fn current_conv(&self) -> Option<&Conversation> {
        self.conversations
            .iter()
            .find(|c| Some(c.id) == self.current)
    }

    fn current_tree(&self) -> Option<&MessageTree> {
        self.current.and_then(|id| self.trees.get(&id))
    }

    fn main_path(&self) -> Vec<MessageId> {
        let Some(conv) = self.current_conv() else {
            return Vec::new();
        };
        let Some(tree) = self.trees.get(&conv.id) else {
            return Vec::new();
        };
        match conv.main_leaf_id {
            Some(leaf) => tree.path_from_root(leaf).unwrap_or_default(),
            None => Vec::new(),
        }
    }

    /// 统一状态 view model:活动内存覆盖流式增量;其余从 generations
    /// 权威状态构建(历史/重启/晚完成失败都有状态与错误;5.3)。
    fn status_of(&self, msg: MessageId) -> Option<StreamState> {
        if let Some(s) = self.streaming.get(&msg) {
            return Some(s.clone());
        }
        let conv = self.current?;
        let g = self.generation_for(conv, msg)?;
        Some(StreamState {
            request_id: g.request_id,
            model: g.model_id.clone(),
            status: g.status,
            error: g.error.clone(),
        })
    }

    /// 该卡 lineage 的重试尝试数(展示"Attempt N";B17)。
    fn lineage_count_of(&self, conv: ConversationId, assistant: MessageId) -> usize {
        let Some(g) = self.generation_for(conv, assistant) else {
            return 1;
        };
        let root = g.retry_root_id.unwrap_or(g.request_id);
        self.generations
            .get(&conv)
            .map(|gs| gs.iter().filter(|g| g.retry_root_id == Some(root)).count())
            .filter(|n| *n > 0)
            .unwrap_or(1)
    }

    /// 该卡是否被同 lineage 更新的尝试取代(默认只显示最新;B17)。
    fn superseded_by_later_attempt(&self, conv: ConversationId, assistant: MessageId) -> bool {
        let Some(tree) = self.trees.get(&conv) else {
            return false;
        };
        let Some(g) = self.generation_for(conv, assistant) else {
            return false;
        };
        let root = g.retry_root_id.unwrap_or(g.request_id);
        let Some(mine) = tree.get(assistant) else {
            return false;
        };
        self.generations
            .get(&conv)
            .map(|gs| {
                gs.iter()
                    .filter(|g| {
                        g.retry_root_id == Some(root) && g.assistant_message_id != assistant
                    })
                    .any(|g| {
                        tree.get(g.assistant_message_id)
                            .map(|other| other.created_at > mine.created_at)
                            .unwrap_or(false)
                    })
            })
            .unwrap_or(false)
    }

    fn generation_for(
        &self,
        conversation: ConversationId,
        assistant: MessageId,
    ) -> Option<Generation> {
        self.generations
            .get(&conversation)?
            .iter()
            .filter(|g| g.assistant_message_id == assistant)
            .cloned()
            .max_by_key(|g| g.started_at.unwrap_or_else(crate::storage::now_dt))
    }

    fn enabled_models(&self) -> Vec<String> {
        let mut list: Vec<(i64, String)> = self
            .prefs
            .iter()
            .filter(|(_, p)| p.enabled)
            .map(|(m, p)| (p.sort_order, m.clone()))
            .collect();
        list.sort();
        list.into_iter().map(|(_, m)| m).collect()
    }

    /// 有效模型中的用量序(缓存;无记录的按面板顺序兜底)。
    fn ranked_enabled(&self) -> Vec<String> {
        let enabled = self.enabled_models();
        let mut out: Vec<String> = self
            .usage_rank
            .iter()
            .filter(|m| enabled.contains(m))
            .cloned()
            .collect();
        for m in enabled {
            if !out.contains(&m) {
                out.push(m);
            }
        }
        out
    }

    fn refresh_usage_rank(&mut self) {
        self.usage_rank = self
            .db
            .model_usage_rank(self.provider_id)
            .unwrap_or_default()
            .into_iter()
            .map(|(m, _, _)| m)
            .collect();
    }

    /// 当前归一化选择快照(UI/预估/发送计划共用)。
    fn normalized_selection(&self) -> NormalizedSelection {
        let ranked = self.ranked_enabled();
        resolve_selection(&self.enabled_models(), &ranked, &self.model_sel)
    }

    fn rerender_current(&mut self) {
        let style = self.style;
        let ids: Vec<MessageId> = self.main_path();
        for id in ids {
            self.rerender_message(id, &style);
        }
    }

    fn rerender_message(&mut self, id: MessageId, style: &RenderStyle) {
        let Some(conv) = self.current_conv() else {
            return;
        };
        let Some(tree) = self.trees.get(&conv.id) else {
            return;
        };
        let Some(m) = tree.get(id) else { return };
        // 解析策略:assistant 用 LlmCompatible(中文标点强调兼容),用户原文 Strict
        let policy = match m.role {
            Role::Assistant => render::md::ParsePolicy::LlmCompatible,
            Role::User => render::md::ParsePolicy::Strict,
        };
        let mut cache = std::mem::take(&mut self.math_cache);
        let rendered = render::render_message(&m.content, style, policy, &mut cache);
        self.math_cache = cache;
        self.renders.insert(id, rendered);
    }

    fn refresh_from_db(&mut self, conv: ConversationId) {
        let msgs = self.db.messages_of(conv).unwrap_or_default();
        let (tree, orphans) = MessageTree::from_messages(msgs);
        if orphans > 0 {
            tracing::warn!(%orphans, "重载存在孤儿消息");
        }
        self.trees.insert(conv, tree);
        self.generations
            .insert(conv, self.db.generations_of(conv).unwrap_or_default());
        self.usage_cache
            .insert(conv, self.db.usage_of(conv).unwrap_or_default());
        let aux = self.db.aux_usage_of(conv).unwrap_or_default();
        let known = aux.iter().filter(|a| a.cost_status == "known").count();
        let unknown = aux.len() - known;
        self.aux_usage_cache.insert(conv, (known, unknown));
        // 刷新会话元数据(标题 CAS 由后台写入)
        if let Ok(all) = self.db.conversations() {
            for c in all {
                if let Some(slot) = self.conversations.iter_mut().find(|s| s.id == c.id) {
                    *slot = c;
                }
            }
        }
    }

    fn set_current(&mut self, id: Option<ConversationId>) -> Task<Message> {
        // 保存旧会话草稿与附件草稿(字段级,不触碰标题)
        if let Some(old_id) = self.current {
            let draft = self.editor.text().to_string();
            let attachments = std::mem::take(&mut self.pending_attachments);
            let json = serde_json::to_string(&attachments).unwrap_or_default();
            let _ = self.db.set_conversation_draft(old_id, &draft, &json);
            self.pending_attachments = attachments;
        }
        self.current = id;
        self.clear_selection();
        let mut tasks = Vec::new();
        if let Some(id) = id {
            let _ = self
                .db
                .set_ui_state("current_conversation", &serde_json::json!(id.as_str()));
            if let Some(c) = self.conversations.iter().find(|c| c.id == id) {
                self.editor = text_editor::Content::with_text(&c.draft);
                self.pending_attachments = c.draft_attachments.clone();
            }
            self.rerender_current();
            self.scroll.set_current_conversation(id);
            let is_new = self
                .trees
                .get(&id)
                .map(|t| t.messages.is_empty())
                .unwrap_or(true);
            let known_height = 0.0f32;
            if let Some(op) = self.scroll.switch_conversation(id, is_new) {
                tasks.push(self.run_scroll_op(op));
            } else if let Some(op) = self.scroll.resolve_anchor(known_height) {
                tasks.push(self.run_scroll_op(op));
            }
        }
        Task::batch(tasks)
    }

    // ----- 滚动操作 -----

    fn run_scroll_op(&self, op: scroll::ScrollOp) -> Task<Message> {
        use iced::advanced::widget::operation::scrollable as scroll_op_mod;
        if op.y >= 0.999 {
            let scrolled = scroll_op_mod::snap_to(
                iced::widget::Id::new(MESSAGES_ID),
                scroll_op_mod::RelativeOffset {
                    x: Some(0.0),
                    y: Some(1.0),
                },
            );
            iced_runtime::task::widget(scrolled)
        } else {
            let scrolled = scroll_op_mod::scroll_to(
                iced::widget::Id::new(MESSAGES_ID),
                scroll_op_mod::AbsoluteOffset {
                    x: Some(0.0),
                    y: Some(op.y),
                },
            );
            iced_runtime::task::widget(scrolled)
        }
    }

    // ----- 选择协调(App 级;单消息跨块) -----

    fn clear_selection(&mut self) {
        self.selection = None;
        self.selecting = false;
        self.copy_snapshot = None;
        self.scroll.set_selecting(false);
    }

    fn selection_hooks(&self, msg: MessageId) -> SelectionHooks<'static, Message> {
        SelectionHooks {
            on_start: Arc::new(move |path, pos| Message::SelStart(msg, path.to_vec(), pos)),
            on_extend: Arc::new(move |path, pos| Message::SelExtend(msg, path.to_vec(), pos)),
            on_finish: Arc::new(|| Message::SelFinish),
            on_range: Arc::new(move |path, a, b| Message::SelRange(msg, path.to_vec(), a, b)),
        }
    }

    fn compute_copy_text(&self) -> Option<String> {
        let (msg, sel) = self.selection.as_ref()?;
        let rendered = self.renders.get(msg)?;
        let flat = render::document::FlatDoc::build(&rendered.doc, &rendered.flow);
        let text = flat.copy_text(sel);
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    }

    // ----- update -----

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::E2eAutopilot => {
                let multi = matches!(
                    std::env::var("AICOUNCIL_E2E").as_deref(),
                    Ok("multi") | Ok("multi2")
                );
                for e in &self.catalog {
                    self.prefs.entry(e.model_id.clone()).or_default().enabled = multi
                        || self
                            .prefs
                            .get(&e.model_id)
                            .map(|p| p.enabled)
                            .unwrap_or(false);
                }
                let Some(model) = self.catalog.first().map(|e| e.model_id.clone()) else {
                    tracing::warn!("e2e: 目录为空,跳过");
                    return Task::none();
                };
                self.prefs.entry(model.clone()).or_default().enabled = true;
                self.refresh_usage_rank();
                self.model_sel.primary = Some(model);
                if multi {
                    let enabled = self.enabled_models();
                    let (checked, primary) = default_parallel_pair(
                        &enabled,
                        &self.usage_rank,
                        self.model_sel.primary.as_deref(),
                    );
                    self.model_sel.parallel = true;
                    self.model_sel.checked = checked;
                    self.model_sel.primary = primary;
                }
                self.editor = text_editor::Content::with_text("用一句话介绍你自己");
                tracing::info!(
                    "e2e: model={:?}, parallel={}, sending",
                    self.model_sel.primary,
                    self.model_sel.parallel
                );
                self.send()
            }
            Message::E2eStage2 => {
                let Some(conv) = self.current else {
                    return Task::none();
                };
                let done: Vec<MessageId> = self
                    .generations
                    .get(&conv)
                    .map(|gs| {
                        gs.iter()
                            .filter(|g| g.status == GenerationStatus::Completed)
                            .map(|g| g.assistant_message_id)
                            .collect()
                    })
                    .unwrap_or_default();
                tracing::info!("e2e stage2: {} completed branches", done.len());
                if done.len() < 2 {
                    tracing::warn!("e2e stage2: 需要至少两支已完成回答");
                    return Task::none();
                }
                self.branch_selection = done;
                self.editor = text_editor::Content::with_text("第二轮:请各用一句话补充");
                self.model_sel.parallel = false;
                self.send()
            }
            // ----- 统一模型选择器 -----
            Message::TogglePicker => {
                self.model_sel.picker_open = !self.model_sel.picker_open;
                if !self.model_sel.picker_open {
                    self.picker_search.clear(); // 关闭即清理搜索(6.2 默认)
                }
                Task::none()
            }
            Message::ToggleParallelMode => {
                if !self.branch_selection.is_empty() {
                    return Task::none(); // 多支追问模式下禁用(由 UI 提示)
                }
                if self.model_sel.parallel {
                    // 关闭并行:单选回到当前 primary
                    self.model_sel.parallel = false;
                    self.model_sel.picker_open = false;
                    self.model_sel.checked.clear();
                } else {
                    // 打开并行:当前 + 最常用另一个(6 节)
                    let enabled = self.enabled_models();
                    let (checked, primary) = default_parallel_pair(
                        &enabled,
                        &self.usage_rank,
                        self.model_sel
                            .primary
                            .as_deref()
                            .or(self.selected_model_fallback().as_deref()),
                    );
                    self.model_sel.parallel = true;
                    self.model_sel.checked = checked;
                    self.model_sel.primary = primary;
                }
                Task::none()
            }
            Message::ToggleChecked(m) => {
                if !self.model_sel.parallel {
                    return Task::none();
                }
                if let Some(pos) = self.model_sel.checked.iter().position(|x| *x == m) {
                    self.model_sel.checked.remove(pos);
                    // primary 被取消 → 按稳定顺序提升
                    if self
                        .model_sel
                        .primary
                        .as_ref()
                        .map(|p| !self.model_sel.checked.contains(p))
                        .unwrap_or(false)
                    {
                        let ranked = self.ranked_enabled();
                        self.model_sel.primary = ranked
                            .iter()
                            .find(|r| self.model_sel.checked.contains(r))
                            .cloned()
                            .or_else(|| self.model_sel.checked.first().cloned());
                    }
                } else {
                    self.model_sel.checked.push(m);
                }
                Task::none()
            }
            Message::SetPrimary(m) => {
                if self.model_sel.parallel {
                    if !self.model_sel.checked.contains(&m) {
                        self.model_sel.checked.push(m.clone());
                    }
                    self.model_sel.primary = Some(m);
                } else {
                    self.model_sel.primary = Some(m);
                }
                Task::none()
            }
            Message::PickSingle(m) => {
                self.model_sel.primary = Some(m);
                self.model_sel.picker_open = false;
                Task::none()
            }
            // ----- 选择 -----
            Message::SelStart(msg, path, pos) => {
                self.selecting = true;
                self.scroll.set_selecting(true);
                let anchor = DocPos::new(path, pos.0, pos.1);
                self.selection = Some((msg, DocSelection::new(anchor.clone(), anchor)));
                Task::none()
            }
            Message::SelExtend(msg, path, pos) => {
                if !self.selecting {
                    return Task::none();
                }
                if self.selection.as_ref().map(|(m, _)| *m) != Some(msg) {
                    return Task::none(); // 单消息选择(跨消息列为限制)
                }
                let focus = DocPos::new(path, pos.0, pos.1);
                if let Some((_, sel)) = self.selection.as_mut() {
                    sel.b = focus;
                }
                Task::none()
            }
            Message::SelRange(msg, path, a, b) => {
                self.selecting = true;
                self.scroll.set_selecting(true);
                self.selection = Some((
                    msg,
                    DocSelection::new(
                        DocPos::new(path.clone(), a.0, a.1),
                        DocPos::new(path, b.0, b.1),
                    ),
                ));
                Task::none()
            }
            Message::SelFinish => {
                if !self.selecting {
                    return Task::none();
                }
                self.selecting = false;
                self.scroll.set_selecting(false);
                let non_empty = self
                    .selection
                    .as_ref()
                    .map(|(_, sel)| !sel.is_empty())
                    .unwrap_or(false);
                if !non_empty {
                    self.clear_selection();
                    return Task::none();
                }
                let Some(text) = self.compute_copy_text() else {
                    self.clear_selection();
                    return Task::none();
                };
                self.copy_snapshot = Some(text.clone());
                iced::clipboard::write::<Message>(text)
            }
            Message::CheckCopy => {
                // 编辑器有焦点时不抢占(编辑器自己的复制已生效)
                let op = iced::advanced::widget::operation::focusable::is_focused(
                    iced::widget::Id::new(COMPOSER_ID),
                );
                iced_runtime::task::widget(op).map(|focused| Message::CopySelection {
                    composer_focused: focused,
                })
            }
            Message::CopySelection { composer_focused } => {
                if composer_focused {
                    return Task::none();
                }
                match self.copy_snapshot.clone() {
                    Some(text) => iced::clipboard::write::<Message>(text),
                    None => match self.compute_copy_text() {
                        Some(text) => iced::clipboard::write::<Message>(text),
                        None => Task::none(),
                    },
                }
            }
            Message::ClearSelection => {
                self.clear_selection();
                Task::none()
            }
            Message::Nop | Message::LinkClicked(_) => Task::none(),
            Message::Provider(ev) => self.on_provider_event(ev),
            Message::SendResult(conv, result) => self.on_send_result(conv, result),
            Message::RetryOutcome(conv, card, result) => self.on_retry_outcome(conv, card, result),
            Message::EgressChecked(intent, result) => self.on_egress_checked(intent, result),
            Message::EgressRecheckResult(result) => {
                // 重新检测只更新弹窗;确认可用后关闭,由用户回到编辑区发送
                let verdict = match result {
                    Ok(v) => v,
                    Err(e) => EgressVerdict::Unknown(e),
                };
                if let Some(modal) = self.egress_modal.as_mut() {
                    modal.checking = false;
                    match verdict {
                        EgressVerdict::Allowed(obs) => {
                            modal.unknown = false;
                            modal.observation = Some(obs);
                            self.egress_modal = None;
                        }
                        EgressVerdict::Blocked(obs) => {
                            modal.unknown = false;
                            modal.observation = Some(obs);
                            modal.detail = i18n::t("egress.body.blocked").to_string();
                        }
                        EgressVerdict::Unknown(reason) => {
                            modal.unknown = true;
                            modal.observation = None;
                            modal.detail =
                                format!("{}\n{}", i18n::t("egress.body.unknown"), reason);
                        }
                    }
                }
                Task::none()
            }
            Message::PickerSearchChanged(s) => {
                self.picker_search = s;
                Task::none()
            }
            Message::EgressModal(action) => self.on_egress_modal(action),
            Message::Reconcile => {
                // broadcast lag:以 DB 为准对账(终态/错误从持久层恢复)
                if let Some(conv) = self.current {
                    self.reconcile_from_db(conv);
                }
                Task::none()
            }
            Message::TitleFinished(ev) => {
                // 后台标题结果:刷新会话元数据(失败保持 fallback)
                if ev.outcome.is_ok() {
                    if let Ok(all) = self.db.conversations() {
                        for c in all {
                            if let Some(slot) = self.conversations.iter_mut().find(|s| s.id == c.id)
                            {
                                if slot.title_source != crate::domain::TitleSource::Manual {
                                    *slot = c;
                                }
                            }
                        }
                    }
                }
                Task::none()
            }
            Message::Tick => self.on_tick(),
            Message::InputEdited(action) => {
                // 长粘贴转附件(≥2500 字符):不进输入框
                if let text_editor::Action::Edit(text_editor::Edit::Paste(s)) = &action {
                    if paste_should_attach(s) {
                        let chars = s.chars().count();
                        let att = Attachment {
                            name: i18n::f(
                                "attachment.paste_name",
                                &[("time", &chrono::Local::now().format("%H%M%S").to_string())],
                            ),
                            content: s.to_string(),
                        };
                        match crate::domain::attachment::try_push(
                            &mut self.pending_attachments,
                            att,
                        ) {
                            Ok(()) => {
                                self.toast = Some(i18n::f(
                                    "attachment.paste_attached",
                                    &[("chars", &chars.to_string())],
                                ))
                            }
                            Err(e) => self.toast = Some(e),
                        }
                        return Task::none();
                    }
                }
                self.editor.perform(action);
                Task::none()
            }
            Message::Send => self.send(),
            Message::Stop(req) => {
                // 服务侧按 request_id 定位取消令牌(submit 后立即 Stop 也可见)
                let chat = self.chat.clone();
                Task::perform(async move { chat.cancel(req).await }, |_| Message::Nop)
            }
            Message::Retry(msg_id) => self.retry(msg_id),
            Message::ToggleAttempts => {
                if let Some(conv) = self.current {
                    if !self.show_attempts.remove(&conv) {
                        self.show_attempts.insert(conv);
                    }
                }
                Task::none()
            }
            Message::Copy(msg_id) => {
                // 整条消息原始 Markdown(tooltip 与局部复制明确区分)
                let content = self
                    .current_tree()
                    .and_then(|t| t.get(msg_id))
                    .map(|m| m.content.clone())
                    .unwrap_or_default();
                iced::clipboard::write::<Message>(content)
            }
            Message::Viewport {
                content,
                view,
                offset,
            } => {
                self.scroll.on_viewport(content, view, offset);
                Task::none()
            }
            Message::BackToBottom => {
                if let Some(op) = self.scroll.request_bottom() {
                    self.run_scroll_op(op)
                } else {
                    Task::none()
                }
            }
            Message::NewConversation => {
                let conv = Conversation::new(ConversationId::new(), crate::storage::now_dt());
                let _ = self.db.upsert_conversation(&conv);
                let id = conv.id;
                self.conversations.insert(0, conv);
                self.trees.insert(id, MessageTree::new());
                self.generations.insert(id, Vec::new());
                self.set_current(Some(id))
            }
            Message::SelectConversation(id) => self.set_current(Some(id)),
            Message::RenameStart(id) => {
                let title = self
                    .conversations
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.title.clone())
                    .unwrap_or_default();
                self.renaming_conv = Some((id, title));
                Task::none()
            }
            Message::RenameInput(s) => {
                if let Some((_, buf)) = self.renaming_conv.as_mut() {
                    *buf = s;
                }
                Task::none()
            }
            Message::RenameApply => {
                let Some((id, title)) = self.renaming_conv.take() else {
                    return Task::none();
                };
                let trimmed = title.trim().to_string();
                if trimmed.is_empty() {
                    self.toast = Some(i18n::t("title.empty_rejected").to_string());
                    return Task::none();
                }
                if trimmed.chars().count() > 80 {
                    self.toast = Some(i18n::t("title.too_long").to_string());
                    return Task::none();
                }
                let Some(c) = self.conversations.iter().find(|c| c.id == id) else {
                    return Task::none();
                };
                match self
                    .db
                    .rename_conversation_title(id, &trimmed, c.title_revision)
                {
                    Ok(Some(authoritative)) => {
                        if let Some(slot) = self.conversations.iter_mut().find(|c| c.id == id) {
                            *slot = authoritative;
                        }
                        Task::none()
                    }
                    Ok(None) => {
                        self.toast =
                            Some(i18n::t("title.save_failed").replace("{error}", "conflict"));
                        Task::none()
                    }
                    Err(e) => {
                        // 失败保留编辑态并提示(不吞错)
                        self.renaming_conv = Some((id, title));
                        self.toast =
                            Some(i18n::f("title.save_failed", &[("error", &e.to_string())]));
                        Task::none()
                    }
                }
            }
            Message::RenameCancel => {
                self.renaming_conv = None;
                Task::none()
            }
            Message::DeleteConversation(id) => {
                let _ = self.db.delete_conversation(id);
                self.conversations.retain(|c| c.id != id);
                self.trees.remove(&id);
                self.generations.remove(&id);
                self.usage_cache.remove(&id);
                self.aux_usage_cache.remove(&id);
                self.expanded_user.remove(&id);
                self.scroll.forget(id);
                if self.current == Some(id) {
                    let next = self.conversations.first().map(|c| c.id);
                    return self.set_current(next);
                }
                Task::none()
            }
            Message::NewGroupInput(s) => {
                self.new_group_name = s;
                Task::none()
            }
            Message::NewGroupApply => {
                let name = self.new_group_name.trim().to_string();
                if !name.is_empty() {
                    let sort = self.groups.iter().map(|g| g.sort_order).max().unwrap_or(0) + 1;
                    let g = Group {
                        id: GroupId::new(),
                        name: name.clone(),
                        sort_order: sort,
                    };
                    let _ = self.db.upsert_group(&g);
                    self.groups.push(g);
                    self.new_group_name.clear();
                }
                Task::none()
            }
            Message::RenameGroupStart(id) => {
                let name = self
                    .groups
                    .iter()
                    .find(|g| g.id == id)
                    .map(|g| g.name.clone())
                    .unwrap_or_default();
                self.renaming_group = Some((id, name));
                Task::none()
            }
            Message::RenameGroupInput(id, name) => {
                if let Some((current, value)) = &mut self.renaming_group {
                    if *current == id {
                        *value = name;
                    }
                }
                Task::none()
            }
            Message::RenameGroupApply => {
                if let Some((id, name)) = self.renaming_group.take() {
                    if !name.trim().is_empty() {
                        if let Some(g) = self.groups.iter_mut().find(|g| g.id == id) {
                            g.name = name.clone();
                            let _ = self.db.upsert_group(g);
                        }
                    }
                }
                Task::none()
            }
            Message::DeleteGroup(id) => {
                let _ = self.db.delete_group(id);
                self.groups.retain(|g| g.id != id);
                let mut affected: Vec<ConversationId> = Vec::new();
                for c in self.conversations.iter_mut() {
                    if c.group_id == Some(id) {
                        c.group_id = None;
                        affected.push(c.id);
                    }
                }
                for conv_id in affected {
                    let _ = self.db.set_conversation_group(conv_id, None);
                }
                Task::none()
            }
            Message::MoveToGroup(conv_id, group) => {
                let _ = self.db.set_conversation_group(conv_id, group);
                if let Some(c) = self.conversations.iter_mut().find(|c| c.id == conv_id) {
                    c.group_id = group;
                }
                Task::none()
            }
            Message::ConvSearchChanged(s) => {
                self.conv_search = s;
                Task::none()
            }
            Message::Panel(p) => {
                self.panel = p;
                Task::none()
            }
            Message::ToggleSidebar => {
                let collapsed = !self.config.window.sidebar_collapsed;
                self.config.window.sidebar_collapsed = collapsed;
                save_config(self.config.clone())
            }
            Message::SidebarPress => {
                self.sidebar_dragging = true;
                Task::none()
            }
            Message::SidebarDrag(x) => {
                if !self.sidebar_dragging {
                    return Task::none();
                }
                // 分隔条在侧栏左缘:宽度 = 窗宽 - 光标 x(窗口逻辑 px)
                let max = (self.window_size.0 - MAIN_MIN_WIDTH - 6.0).min(SIDEBAR_MAX);
                let width = (self.window_size.0 - x).clamp(SIDEBAR_MIN, max.max(SIDEBAR_MIN));
                self.config.window.sidebar_width = width;
                Task::none()
            }
            Message::SidebarRelease => {
                if !self.sidebar_dragging {
                    return Task::none();
                }
                self.sidebar_dragging = false;
                save_config(self.config.clone())
            }
            Message::SidebarReset => {
                self.config.window.sidebar_width = SIDEBAR_DEFAULT;
                save_config(self.config.clone())
            }
            Message::ToggleExportMenu => {
                self.export_menu_open = !self.export_menu_open;
                Task::none()
            }
            Message::ToggleUsage => {
                self.usage_expanded = !self.usage_expanded;
                Task::none()
            }
            Message::KeyInput(k) => {
                self.key_input = k;
                Task::none()
            }
            Message::SaveKey => {
                let key = self.key_input.trim().to_string();
                if key.is_empty() {
                    self.models_status = i18n::t("models.key_empty").to_string();
                    return Task::none();
                }
                let persistent = self.vault.save("openrouter", &key).unwrap_or(false);
                self.key_saved = true;
                self.models_status = if persistent {
                    i18n::t("models.key_saved").to_string()
                } else {
                    i18n::t("models.key_saved_volatile").to_string()
                };
                let provider = Arc::new(OpenRouter::new(self.provider_id, key));
                self.chat = Arc::new(
                    ChatService::new(
                        provider.clone(),
                        self.db.clone(),
                        (*self.event_tx).clone(),
                        4,
                    )
                    .with_egress_guard(self.guard.clone()),
                );
                self.provider = provider.clone();
                Task::batch(vec![
                    Task::perform(test_key(self.provider.clone()), Message::KeyStatusDone),
                    Task::perform(fetch_catalog(self.provider.clone()), |r| match r {
                        Ok(e) => Message::CatalogLoaded(Ok(e)),
                        Err(e) => Message::CatalogLoaded(Err(e)),
                    }),
                ])
            }
            Message::TestConnection => {
                self.models_status = i18n::t("models.testing").to_string();
                Task::perform(test_key(self.provider.clone()), Message::TestConn)
            }
            Message::RefreshCatalog => {
                self.models_status = i18n::t("models.refreshing").to_string();
                Task::perform(fetch_catalog(self.provider.clone()), |r| match r {
                    Ok(e) => Message::CatalogLoaded(Ok(e)),
                    Err(e) => Message::CatalogLoaded(Err(e)),
                })
            }
            Message::CatalogLoaded(Ok(entries)) => {
                let now = crate::storage::now_dt();
                let _ = self.db.replace_catalog(self.provider_id, &entries, now);
                self.catalog = entries;
                self.models_status = i18n::f(
                    "models.catalog_updated",
                    &[("time", &now.format("%H:%M:%S").to_string())],
                );
                Task::none()
            }
            Message::CatalogLoaded(Err(e)) => {
                self.models_status = i18n::f("models.catalog_failed", &[("error", &e)]);
                Task::none()
            }
            Message::TestConn(Ok(status)) => {
                self.models_status = match status {
                    KeyStatus::Ok { .. } => i18n::t("models.connected").to_string(),
                    KeyStatus::PermissionDenied => i18n::t("models.connected_no_quota").to_string(),
                    KeyStatus::Unsupported => i18n::t("models.connected_no_support").to_string(),
                };
                Task::none()
            }
            Message::TestConn(Err(e)) => {
                self.models_status = i18n::f("models.connect_failed", &[("error", &e)]);
                Task::none()
            }
            Message::KeyStatusDone(Ok(status)) => {
                match status {
                    KeyStatus::Ok {
                        usage,
                        limit_remaining,
                        ..
                    } => {
                        let mut s = i18n::t("models.key_valid").to_string();
                        if let Some(u) = usage {
                            s.push_str(&i18n::f("models.key_used", &[("usage", &u.to_string())]));
                        }
                        if let Some(r) = limit_remaining {
                            s.push_str(&i18n::f(
                                "models.key_remaining",
                                &[("remaining", &r.to_string())],
                            ));
                        }
                        self.models_status = s;
                        self.key_status_detail = self.models_status.clone();
                    }
                    KeyStatus::PermissionDenied => {
                        self.models_status = i18n::t("models.connected_no_quota").to_string();
                        self.key_status_detail = self.models_status.clone();
                    }
                    KeyStatus::Unsupported => {
                        self.models_status = i18n::t("models.connected_no_support").to_string();
                        self.key_status_detail = self.models_status.clone();
                    }
                }
                Task::none()
            }
            Message::KeyStatusDone(Err(e)) => {
                self.models_status = i18n::f("models.key_check_failed", &[("error", &e)]);
                self.key_status_detail = self.models_status.clone();
                Task::none()
            }
            Message::ToggleModel(m) => {
                let mut pref = self.prefs.get(&m).cloned().unwrap_or_default();
                pref.enabled = !pref.enabled;
                if !pref.enabled {
                    pref.is_default = false;
                    // 禁用模型退出并行勾选与 primary
                    self.model_sel.checked.retain(|x| x != &m);
                    if self.model_sel.primary.as_deref() == Some(&m) {
                        let ranked = self.ranked_enabled();
                        self.model_sel.primary = ranked
                            .into_iter()
                            .find(|x| self.model_sel.checked.contains(x))
                            .or_else(|| self.model_sel.checked.first().cloned());
                    }
                }
                let _ = self.db.upsert_preference(self.provider_id, &m, &pref);
                self.prefs.insert(m.clone(), pref);
                Task::none()
            }
            Message::SetDefault(m) => {
                let provider_id = self.provider_id;
                let updates: Vec<(String, ModelPreference)> = self
                    .prefs
                    .iter()
                    .map(|(model, pref)| {
                        let mut p = pref.clone();
                        p.is_default = model == &m;
                        (model.clone(), p)
                    })
                    .collect();
                for (model, pref) in updates {
                    let _ = self.db.upsert_preference(provider_id, &model, &pref);
                    self.prefs.insert(model, pref);
                }
                self.model_sel.primary = Some(m);
                Task::none()
            }
            Message::MoveModel(m, delta) => {
                let mut pref = self.prefs.get(&m).cloned().unwrap_or_default();
                pref.sort_order += delta;
                let _ = self.db.upsert_preference(self.provider_id, &m, &pref);
                self.prefs.insert(m, pref);
                Task::none()
            }
            Message::ModelSearchChanged(s) => {
                self.model_search = s;
                Task::none()
            }
            Message::Export(scope) => {
                self.export_menu_open = false;
                if self.current_conv().is_none() || self.main_path().is_empty() {
                    self.toast = Some(i18n::t("export.none").to_string());
                    return Task::none();
                }
                let title = self
                    .current_conv()
                    .map(|c| {
                        if c.title.trim().is_empty() {
                            i18n::t("export.default_title").to_string()
                        } else {
                            c.title.clone()
                        }
                    })
                    .unwrap_or_else(|| i18n::t("export.default_title").to_string());
                let default = export::default_filename(&title, scope.clone());
                self.pending_export = Some(scope);
                Task::perform(pick_save_path(default), Message::ExportPathChosen)
            }
            Message::ExportPathChosen(path) => {
                let Some(path) = path else {
                    self.pending_export = None;
                    return Task::none();
                };
                let Some(scope) = self.pending_export.take() else {
                    return Task::none();
                };
                let conv: Option<Conversation> = self.current_conv().cloned();
                let title = conv.as_ref().map(|c| c.title.clone()).unwrap_or_default();
                let main_leaf = conv.as_ref().and_then(|c| c.main_leaf_id);
                let tree = self
                    .current
                    .and_then(|id| self.trees.get(&id))
                    .cloned()
                    .unwrap_or_default();
                let gens = self
                    .current
                    .and_then(|id| self.generations.get(&id))
                    .cloned()
                    .unwrap_or_default();
                Task::perform(
                    async move {
                        let input = export::ExportInput {
                            title: &title,
                            tree: &tree,
                            generations: &gens,
                            main_leaf,
                        };
                        export::export(&input, scope, &path)
                            .map(|r| {
                                i18n::f(
                                    "export.done",
                                    &[
                                        ("sections", &r.sections.to_string()),
                                        ("path", &r.path.display().to_string()),
                                    ],
                                )
                            })
                            .map_err(|e| e.to_string())
                    },
                    Message::ExportDone,
                )
            }
            Message::ExportDone(Ok(msg)) => {
                self.toast = Some(msg);
                Task::none()
            }
            Message::ExportDone(Err(e)) => {
                self.toast = Some(i18n::f("export.failed", &[("error", &e)]));
                Task::none()
            }
            Message::ToggleBranchSelect(id) => {
                if let Some(pos) = self.branch_selection.iter().position(|x| *x == id) {
                    self.branch_selection.remove(pos);
                } else {
                    self.branch_selection.push(id);
                }
                Task::none()
            }
            Message::ContinueFrom(id) => {
                let Some(conv) = self.current else {
                    return Task::none();
                };
                let Some(g) = self.generation_for(conv, id) else {
                    return Task::none();
                };
                if g.status != GenerationStatus::Completed {
                    self.toast = Some(i18n::t("toast.continue_not_completed").to_string());
                    return Task::none();
                }
                let _ = self.db.set_conversation_main_leaf(conv, Some(id));
                if let Some(c) = self.conversations.iter_mut().find(|c| c.id == conv) {
                    c.main_leaf_id = Some(id);
                }
                self.rerender_current();
                if let Some(op) = self.scroll.notify_sent() {
                    return self.run_scroll_op(op);
                }
                Task::none()
            }
            Message::ToggleExpand(id) => {
                let Some(conv) = self.current else {
                    return Task::none();
                };
                let set = self.expanded_user.entry(conv).or_default();
                if set.contains(&id) {
                    set.remove(&id);
                } else {
                    set.insert(id);
                }
                Task::none()
            }
            Message::PickAttachments => Task::perform(pick_attachments(), Message::AttachLoaded),
            Message::AttachLoaded(picked) => {
                let mut added = 0usize;
                let mut notes = picked.errors;
                for att in picked.files {
                    match crate::domain::attachment::try_push(&mut self.pending_attachments, att) {
                        Ok(()) => added += 1,
                        Err(e) => notes.push(e),
                    }
                }
                if added > 0 || !notes.is_empty() {
                    let mut msg = i18n::f("attachment.added", &[("n", &added.to_string())]);
                    if !notes.is_empty() {
                        msg.push_str(&format!(";{}", notes.join(";")));
                    }
                    self.toast = Some(msg);
                }
                Task::none()
            }
            Message::RemoveAttachment(i) => {
                if i < self.pending_attachments.len() {
                    self.pending_attachments.remove(i);
                }
                Task::none()
            }
            Message::FontSize(v) => {
                let size = v.round().clamp(
                    crate::ui_style::BODY_SIZE_MIN,
                    crate::ui_style::BODY_SIZE_MAX,
                );
                if (self.style.body - size).abs() < f32::EPSILON {
                    return Task::none();
                }
                self.config.general.body_text_size = Some(size);
                self.style = RenderStyle::soft_sage(size);
                // 全量重渲染(缓存键含字号)
                self.rerender_current();
                save_config(self.config.clone())
            }
            Message::WindowResized(size) => {
                self.window_size = (size.width, size.height);
                Task::none()
            }
            Message::KeyPressed(key, modifiers) => {
                use iced::keyboard::{Key, Modifiers};
                if key == Key::Named(iced::keyboard::key::Named::Escape) {
                    if self.egress_modal.take().is_some() {
                        // Esc 取消弹窗:丢弃当前 intent,避免 modal 堆叠
                        self.pending_send = None;
                        return Task::none();
                    }
                    if self.renaming_conv.take().is_some() || self.renaming_group.take().is_some() {
                        return Task::none();
                    }
                    if self.selection.is_some() {
                        self.clear_selection();
                    }
                    return Task::none();
                }
                if matches!(key, Key::Character(ref c) if c.eq_ignore_ascii_case("c"))
                    && modifiers.command()
                    && self.selection.is_some()
                {
                    return self.update(Message::CheckCopy);
                }
                let _ = Modifiers::default();
                Task::none()
            }
            Message::ConfigSaved(result) => {
                if let Err(e) = result {
                    tracing::warn!(%e, "配置写入失败");
                }
                Task::none()
            }
        }
    }

    fn selected_model_fallback(&self) -> Option<String> {
        self.enabled_models().first().cloned()
    }

    fn on_tick(&mut self) -> Task<Message> {
        self.tick_count = self.tick_count.wrapping_add(1);
        let mut tasks = Vec::new();
        // 一次 tick 刷新所有脏流式消息,再计算一次阅读位置(PLAN 5.2)
        if !self.dirty_stream.is_empty() {
            let ids: Vec<MessageId> = self.dirty_stream.drain().collect();
            let style = self.style;
            for id in ids {
                self.rerender_message(id, &style);
            }
            if let Some(op) = self.scroll.notify_content_growth() {
                tasks.push(self.run_scroll_op(op));
            }
        }
        if self.tick_count.is_multiple_of(60) && self.toast.is_some() {
            self.toast = None;
        }
        if self.tick_count.is_multiple_of(60) && self.toast_error.is_some() {
            self.toast_error = None;
        }
        if tasks.is_empty() {
            Task::none()
        } else {
            Task::batch(tasks)
        }
    }

    fn on_provider_event(&mut self, ev: ChatEvent) -> Task<Message> {
        let conv = ev.conversation_id;
        let msg = ev.assistant_message_id;
        let is_current = self.current == Some(conv);

        match ev.event {
            GenEvent::Started => {
                self.refresh_usage_rank();
                if is_current {
                    if let Some(s) = self.streaming.get_mut(&msg) {
                        s.status = GenerationStatus::Connecting;
                    }
                }
            }
            GenEvent::TextDelta(t) => {
                if is_current {
                    if let Some(tree) = self.trees.get_mut(&conv) {
                        if let Some(m) = tree.messages.get_mut(&msg) {
                            m.content.push_str(&t);
                        }
                    }
                    if let Some(s) = self.streaming.get_mut(&msg) {
                        s.status = GenerationStatus::Streaming;
                    }
                    self.dirty_stream.insert(msg);
                }
            }
            GenEvent::UsageUpdated { .. } => {}
            GenEvent::Finished { .. } => {
                // main_leaf 由服务层 CAS 移动;此处只对账
                self.streaming.remove(&msg);
                self.refresh_from_db(conv);
                self.refresh_usage_rank();
                if is_current {
                    let style = self.style;
                    self.rerender_message(msg, &style);
                }
            }
            GenEvent::Failed { error } => {
                // 失败也刷新 generations:历史/重启失败同样显示(B02)
                self.streaming.remove(&msg);
                self.refresh_from_db(conv);
                if is_current {
                    let style = self.style;
                    self.rerender_message(msg, &style);
                }
                let _ = error;
            }
            GenEvent::Cancelled => {
                self.streaming.remove(&msg);
                self.refresh_from_db(conv);
                if is_current {
                    let style = self.style;
                    self.rerender_message(msg, &style);
                }
            }
            GenEvent::Rejected { detail } => {
                // dispatch 门禁晚到拒绝:卡片显示持久化原因
                self.streaming.remove(&msg);
                self.refresh_from_db(conv);
                if is_current {
                    let style = self.style;
                    self.rerender_message(msg, &style);
                }
                self.toast_error = Some(i18n::f(
                    "toast.rejected_no_dispatch",
                    &[("reason", &detail)],
                ));
            }
            GenEvent::PersistError { error } => {
                // 可见存储故障(B18)
                self.toast_error = Some(i18n::f("toast.storage_error", &[("error", &error)]));
            }
        }
        Task::none()
    }

    /// 续聊资格(4.2/5.5(1)):当前路径活动请求未结束时禁止重复发送;
    /// 终态失败/取消/中断不阻塞新轮次。
    fn continuation_ready(&self, conv: ConversationId) -> bool {
        let Some(leaf) = self.current_conv().and_then(|c| c.main_leaf_id) else {
            return true;
        };
        let Some(tree) = self.trees.get(&conv) else {
            return true;
        };
        match tree.get(leaf).map(|m| m.role) {
            Some(Role::User) | None => true,
            Some(Role::Assistant) => self
                .generation_for(conv, leaf)
                .map(|g| g.status.is_terminal())
                .unwrap_or(true),
        }
    }

    fn send(&mut self) -> Task<Message> {
        let prompt = self.editor.text().to_string();
        if prompt.trim().is_empty() && self.pending_attachments.is_empty() {
            return Task::none();
        }
        let ranked = self.ranked_enabled();
        let snapshot = resolve_selection(&self.enabled_models(), &ranked, &self.model_sel);
        if let Some(reason) = snapshot.invalid_reason {
            self.toast = Some(i18n::t(reason).to_string());
            return Task::none();
        }
        let primary_model = snapshot.primary.clone().unwrap_or_default();

        // 同路径活动请求防重(B05);终态失败不阻塞(B01/B02/B05)
        if let Some(conv) = self.current {
            if !self.continuation_ready(conv) {
                self.toast = Some(i18n::t("toast.branch_active").to_string());
                return Task::none();
            }
        }

        // 会话不存在则创建
        if self.current.is_none() {
            let conv = Conversation::new(ConversationId::new(), crate::storage::now_dt());
            let _ = self.db.upsert_conversation(&conv);
            let id = conv.id;
            self.conversations.insert(0, conv);
            self.trees.insert(id, MessageTree::new());
            self.generations.insert(id, Vec::new());
            self.current = Some(id);
        }
        let conv_id = self.current.unwrap();

        // 主模型参数合成
        let entry = self.catalog.iter().find(|e| e.model_id == primary_model);
        let pref = self.prefs.get(&primary_model).cloned().unwrap_or_default();
        let (_params, rejected) = entry
            .map(|e| compose_params(e, &pref))
            .unwrap_or((None, Vec::new()));
        if !rejected.is_empty() {
            self.toast = Some(i18n::f(
                "toast.params_rejected",
                &[("params", &format!("{rejected:?}"))],
            ));
        }

        let leaf = self.current_conv().and_then(|c| c.main_leaf_id);
        let system_prompt = self
            .current_conv()
            .map(|c| c.system_prompt.clone())
            .filter(|s| !s.trim().is_empty());
        let tree = self.trees.get(&conv_id).cloned().unwrap_or_default();

        let selection = if !self.branch_selection.is_empty() {
            // 多支追问:每支沿自己的分支,各带当前 primary;禁止分支×模型
            let target = ModelTarget {
                provider_id: self.provider_id,
                model_id: primary_model.clone(),
                params: _params,
                price: entry.map(|e| e.pricing),
            };
            let primary_leaf = leaf;
            let mut branches: Vec<(MessageId, ModelTarget)> = Vec::new();
            let mut primary_idx = 0usize;
            for leaf_id in &self.branch_selection {
                if Some(*leaf_id) == primary_leaf {
                    primary_idx = branches.len();
                }
                branches.push((*leaf_id, target.clone()));
            }
            if let Some(pl) = primary_leaf {
                if !branches.iter().any(|(l, _)| l == &pl) {
                    branches.insert(0, (pl, target.clone()));
                    primary_idx = 0;
                }
            }
            ContinuationSelection::MultiBranch {
                branches,
                primary: primary_idx,
            }
        } else if self.model_sel.parallel {
            // 同题 N 模型:严格按归一化快照(主模型在首位)
            let mut targets: Vec<ModelTarget> = Vec::new();
            for m in &snapshot.targets {
                let entry = self.catalog.iter().find(|e| e.model_id == *m);
                let pref = self.prefs.get(m).cloned().unwrap_or_default();
                let (params, _) = entry
                    .map(|e| compose_params(e, &pref))
                    .unwrap_or((None, Vec::new()));
                targets.push(ModelTarget {
                    provider_id: self.provider_id,
                    model_id: m.clone(),
                    params,
                    price: entry.map(|e| e.pricing),
                });
            }
            ContinuationSelection::SameQuestionMulti {
                leaf,
                targets,
                primary: 0,
            }
        } else {
            ContinuationSelection::Single {
                leaf,
                provider_id: self.provider_id,
                model_id: primary_model,
            }
        };
        let Ok(plan) = Planner::prepare(
            conv_id,
            &selection,
            &prompt,
            &self.pending_attachments,
            system_prompt,
            &tree,
            crate::storage::now_dt(),
        ) else {
            return Task::none();
        };

        // 发送意图原子锁定(防连点):一份 intent 绑定其草稿 revision。
        // 事务前不插树/不清草稿/不动 leaf —— 预检或落库失败零回滚(5.3)。
        self.intent_seq += 1;
        let intent = self.intent_seq;
        self.pending_send = Some(PendingSend {
            conv: conv_id,
            plan: plan.clone(),
            submitted_text: prompt,
            submitted_attachments: self.pending_attachments.clone(),
            intent,
        });

        // Anthropic 全模型出口门禁(不限 Claude;按开发商归属)
        let has_anthropic = plan
            .branches
            .iter()
            .flat_map(|b| b.requests.iter().map(|r| r.model_id.as_str()))
            .any(is_anthropic);
        if has_anthropic {
            self.spawn_egress_check(intent)
        } else {
            self.finalize_send(None)
        }
    }

    /// 异步出口预检(弹窗可取消;结果按 intent 校验)。
    fn spawn_egress_check(&self, intent: u64) -> Task<Message> {
        let guard = self.guard.clone();
        Task::perform(
            async move { guard.check(CancellationToken::new()).await },
            move |v| Message::EgressChecked(intent, Ok(v)),
        )
    }

    /// 门禁通过(或无需门禁)后真正提交。
    fn finalize_send(&mut self, permit: Option<EgressPermit>) -> Task<Message> {
        let Some(pending) = self.pending_send.clone() else {
            return Task::none();
        };
        let conv = pending.conv;
        // 清编辑器:仅当内容仍是已提交 revision(A10:预检期间的新内容不清除)
        if self.current == Some(conv) && self.editor.text() == pending.submitted_text {
            self.editor = text_editor::Content::new();
            self.pending_attachments.clear();
        }
        let chat = self.chat.clone();
        let titles = self.titles.clone();
        let turn_prompt = pending.plan.prompt.clone();
        let turn_attachments = pending.plan.attachments.clone();
        let turn_user = pending.plan.branches[0].user_message_id;
        Task::perform(
            async move {
                chat.submit_with_permit(pending.plan, permit)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |r| {
                if let Ok(receipt) = &r {
                    // 成功落库 → fallback + 异步标题(不阻塞 UI)
                    titles.on_user_turn_persisted(
                        receipt.conversation_id,
                        turn_user.as_str().to_string(),
                        &turn_prompt,
                        &turn_attachments,
                    );
                }
                Message::SendResult(conv, r)
            },
        )
    }

    /// 出口预检结果:Allowed → 提交;Blocked/Unknown → 单弹窗。
    fn on_egress_checked(
        &mut self,
        intent: u64,
        result: Result<EgressVerdict, String>,
    ) -> Task<Message> {
        let verdict = match result {
            Ok(v) => v,
            Err(e) => EgressVerdict::Unknown(e),
        };
        // 旧 intent 失效:用户已编辑/换模型/切会话/重发(A10)
        let Some(pending) = &self.pending_send else {
            return Task::none();
        };
        if pending.intent != intent {
            return Task::none();
        }
        match verdict {
            EgressVerdict::Allowed(_) => {
                self.egress_modal = None;
                let permit = EgressPermit::fresh(self.guard.network_revision());
                self.finalize_send(Some(permit))
            }
            EgressVerdict::Blocked(obs) => {
                let mixed = self.pending_send.as_ref().is_some_and(|p| {
                    p.plan
                        .branches
                        .iter()
                        .flat_map(|b| b.requests.iter().map(|r| r.model_id.as_str()))
                        .any(|m| !is_anthropic(m))
                });
                self.egress_modal = Some(EgressModal {
                    unknown: false,
                    observation: Some(obs),
                    detail: i18n::t("egress.body.blocked").to_string(),
                    mixed,
                    checking: false,
                });
                Task::none()
            }
            EgressVerdict::Unknown(reason) => {
                let mixed = self.pending_send.as_ref().is_some_and(|p| {
                    p.plan
                        .branches
                        .iter()
                        .flat_map(|b| b.requests.iter().map(|r| r.model_id.as_str()))
                        .any(|m| !is_anthropic(m))
                });
                self.egress_modal = Some(EgressModal {
                    unknown: true,
                    observation: None,
                    detail: format!("{}\n{}", i18n::t("egress.body.unknown"), reason),
                    mixed,
                    checking: false,
                });
                Task::none()
            }
        }
    }

    /// 门禁弹窗动作。
    fn on_egress_modal(&mut self, action: EgressModalAction) -> Task<Message> {
        match action {
            EgressModalAction::Recheck => {
                // 只重新检测,不自动发送;确认可用后回到编辑区由用户发送
                if let Some(m) = self.egress_modal.as_mut() {
                    m.checking = true;
                }
                let guard = self.guard.clone();
                Task::perform(
                    async move { guard.check(CancellationToken::new()).await },
                    move |v| Message::EgressRecheckResult(Ok(v)),
                )
            }
            EgressModalAction::Dismiss => {
                self.egress_modal = None;
                self.pending_send = None;
                Task::none()
            }
            EgressModalAction::RemoveAnthropicAndSend => {
                self.egress_modal = None;
                self.pending_send = None;
                // 显式新动作:从并行勾选移除全部 Anthropic 目标;主模型
                // 若被移除则提升为首个剩余目标(主模型可见且正确)
                let enabled = self.enabled_models();
                let non_anthropic: Vec<String> = enabled
                    .iter()
                    .filter(|m| !is_anthropic(m))
                    .cloned()
                    .collect();
                self.model_sel.checked.retain(|m| non_anthropic.contains(m));
                if self
                    .model_sel
                    .primary
                    .as_ref()
                    .is_some_and(|p| is_anthropic(p))
                {
                    self.model_sel.primary = non_anthropic.first().cloned();
                }
                self.send()
            }
        }
    }

    /// submit 回执:按 DB 权威状态对账(不把已终态请求写成 Queued)。
    fn on_send_result(
        &mut self,
        conv: ConversationId,
        result: Result<SubmitReceipt, String>,
    ) -> Task<Message> {
        match result {
            Ok(receipt) => {
                // 草稿字段级清空(仅已提交 revision)
                if self.pending_send.as_ref().is_some_and(|p| p.conv == conv) {
                    self.pending_send = None;
                }
                let _ = self.db.set_conversation_draft(
                    conv,
                    "",
                    &serde_json::to_string(&Vec::<Attachment>::new()).unwrap_or_default(),
                );
                self.on_submitted(conv, receipt)
            }
            Err(e) => {
                // 落库失败:无假消息/假 leaf;恢复草稿与附件(B11)
                tracing::error!(%e, "提交失败(草稿保留)");
                if let Some(p) = self.pending_send.take() {
                    if p.conv == conv && self.current == Some(conv) && self.editor.text().is_empty()
                    {
                        self.editor = text_editor::Content::with_text(&p.submitted_text);
                        self.pending_attachments = p.submitted_attachments;
                    }
                }
                self.toast_error = Some(i18n::f("toast.send_failed", &[("error", &e)]));
                Task::none()
            }
        }
    }

    /// submit/retry 回执合并:占位/流式状态/渲染。
    fn on_submitted(
        &mut self,
        conv: ConversationId,
        receipt: crate::services::SubmitReceipt,
    ) -> Task<Message> {
        self.branch_selection.clear();
        if self.current != Some(conv) {
            // 后台会话:只刷新缓存,不碰当前编辑器(B12)
            self.refresh_from_db(conv);
            return Task::none();
        }
        self.refresh_from_db(conv);
        self.refresh_usage_rank();
        for (req, assistant) in &receipt.assistant_of {
            let gen = self
                .generations
                .get(&conv)
                .and_then(|gs| gs.iter().find(|g| g.request_id == *req));
            let Some(g) = gen else { continue };
            // 回执可能晚于开始/失败/完成:以 DB 为准(B12)
            self.streaming.insert(
                *assistant,
                StreamState {
                    request_id: *req,
                    model: g.model_id.clone(),
                    status: g.status,
                    error: g.error.clone(),
                },
            );
        }
        self.rerender_current();
        if let Some(op) = self.scroll.notify_sent() {
            return self.run_scroll_op(op);
        }
        Task::none()
    }

    /// 重试入口:UI 资格先行(服务层以最新持久数据兜底;B07/B09/B10)。
    fn retry(&mut self, msg_id: MessageId) -> Task<Message> {
        let Some(conv_id) = self.current else {
            return Task::none();
        };
        let Some(original) = self.generation_for(conv_id, msg_id) else {
            return Task::none();
        };
        let Some(tree) = self.trees.get(&conv_id) else {
            return Task::none();
        };
        let Some(user_msg) = tree.get(original.user_message_id) else {
            return Task::none();
        };

        // 资格:lineage 活动尝试 / 冷却 / 补救
        let root = original.retry_root_id.unwrap_or(original.request_id);
        let lineage_active = self
            .generations
            .get(&conv_id)
            .map(|gs| {
                gs.iter()
                    .any(|g| g.retry_root_id == Some(root) && !g.status.is_terminal())
            })
            .unwrap_or(false);
        let latest_failure: Option<FailureInfo> = self.generations.get(&conv_id).and_then(|gs| {
            gs.iter()
                .filter(|g| g.retry_root_id == Some(root) && g.failure.is_some())
                .max_by_key(|g| g.finished_at.unwrap_or_else(crate::storage::now_dt))
                .and_then(|g| g.failure.clone())
        });
        match retry_decision(
            original.status,
            latest_failure.as_ref(),
            crate::domain::recovery::now_ms(),
            lineage_active,
        ) {
            RetryDecision::Allowed => {}
            RetryDecision::InFlight => {
                self.toast = Some(i18n::t("toast.retry_in_flight").to_string());
                return Task::none();
            }
            RetryDecision::Cooldown(until) => {
                let secs = ((until - crate::domain::recovery::now_ms()) / 1000).max(0);
                self.toast = Some(i18n::f("retry.cooldown", &[("s", &secs.to_string())]));
                return Task::none();
            }
            RetryDecision::NeedsAction(remedy) => {
                let key = match remedy {
                    crate::domain::Remedy::SwitchIp => "retry.needs.switch_ip",
                    crate::domain::Remedy::Credentials => "retry.needs.credentials",
                    crate::domain::Remedy::Balance => "retry.needs.balance",
                    crate::domain::Remedy::EditInput => "retry.needs.edit_input",
                    crate::domain::Remedy::PickModel => "retry.needs.pick_model",
                    crate::domain::Remedy::None => "retry.needs.none",
                };
                self.toast = Some(i18n::t(key).to_string());
                return Task::none();
            }
        }
        // UI 防重第一步置位;服务事务 + lineage 唯一索引兜底
        if !self.retrying.insert(msg_id) {
            self.toast = Some(i18n::t("toast.retry_in_flight").to_string());
            return Task::none();
        }

        let prompt =
            crate::domain::attachment::compose_prompt(&user_msg.content, &user_msg.attachments);
        // 兜底上下文(缺旧快照时的"按现有历史重新生成";服务优先用快照)
        let history = Planner::history_before(tree, original.user_message_id);

        let chat = self.chat.clone();
        let conv = conv_id;
        let orig = original.clone();
        Task::perform(
            async move {
                chat.retry(conv, &orig, &prompt, history)
                    .await
                    .map_err(|e| e.to_string())
            },
            move |r| Message::RetryOutcome(conv, msg_id, r),
        )
    }

    fn on_retry_outcome(
        &mut self,
        conv: ConversationId,
        card: MessageId,
        result: Result<SubmitReceipt, String>,
    ) -> Task<Message> {
        self.retrying.remove(&card);
        match result {
            Ok(receipt) => {
                self.toast_error = None;
                self.on_submitted(conv, receipt)
            }
            Err(e) => {
                tracing::warn!(%e, "重试被拒绝");
                self.toast_error = Some(i18n::f("toast.retry_rejected", &[("reason", &e)]));
                if self.current == Some(conv) {
                    self.refresh_from_db(conv);
                    self.rerender_current();
                }
                Task::none()
            }
        }
    }

    /// broadcast lag / 重进会话:以 DB 为准恢复终态与错误。
    fn reconcile_from_db(&mut self, conv: ConversationId) {
        self.refresh_from_db(conv);
        // 清理已被持久层判定终态的活动 UI 映射(不动其他未 flush 的流)
        if let Some(gs) = self.generations.get(&conv) {
            let terminal: HashSet<GenerationId> = gs
                .iter()
                .filter(|g| g.status.is_terminal())
                .map(|g| g.request_id)
                .collect();
            self.streaming
                .retain(|_, s| !terminal.contains(&s.request_id));
        }
        self.rerender_current();
    }
}

/// 安静按钮快捷构造(quiet 样式)。
fn button<'a, M: Clone + 'a>(content: impl Into<Element<'a, M>>) -> iced::widget::Button<'a, M> {
    iced_button(content).style(design::quiet_button)
}

fn save_config(config: storage::repo::AppConfig) -> Task<Message> {
    Task::perform(
        async move { tokio::task::spawn_blocking(move || config.save()).await },
        |r| {
            Message::ConfigSaved(match r {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            })
        },
    )
}

// ---------------------------------------------------------------------------
// view
// ---------------------------------------------------------------------------

impl App {
    fn view(&self) -> Element<'_, Message> {
        // AICOUNCIL_PROBE=1:仅渲染最小内容(渲染管线诊断用)
        if std::env::var("AICOUNCIL_PROBE").is_ok() {
            return column![text("PROBE OK").size(20)].padding(20).into();
        }
        let main = self.view_main();
        if self.config.window.sidebar_collapsed {
            return main;
        }
        let width = self.config.window.sidebar_width;
        let divider = mouse_area(
            container(Space::new().width(Length::Fixed(1.0)))
                .width(Length::Fixed(6.0))
                .height(Length::Fill)
                .style(design::divider_style),
        )
        .on_press(Message::SidebarPress)
        .on_release(Message::SidebarRelease)
        .on_move(|p| Message::SidebarDrag(p.x))
        .on_double_click(Message::SidebarReset)
        .interaction(iced::mouse::Interaction::ResizingHorizontally);
        let sidebar = self.view_sidebar();
        iced::widget::row![
            container(main).width(Length::Fill).height(Length::Fill),
            divider,
            container(sidebar)
                .width(Length::Fixed(width))
                .height(Length::Fill),
        ]
        .spacing(0)
        .into()
    }

    fn view_main(&self) -> Element<'_, Message> {
        let header = self.view_header();
        let messages = self.view_messages();
        let input = self.view_input();
        column![
            header,
            container(messages).width(Length::Fill).height(Length::Fill),
            input,
        ]
        .into()
    }

    fn view_header(&self) -> Element<'_, Message> {
        let back: Element<'_, Message> = if self.scroll.mode() == scroll::Mode::Reading {
            // Reading 才出现;Following 不占位(固定位置图标,不随 token 重排)
            row![iced::widget::tooltip(
                button(icons::icon("down")).on_press(Message::BackToBottom),
                text(i18n::t("panel.back_bottom_tooltip")).size(12),
                iced::widget::tooltip::Position::Bottom,
            )]
            .into()
        } else {
            row![].into()
        };

        // 可编辑标题(主区;Enter 保存 / Esc 取消;与侧栏同一组件形态)
        let title_el: Element<'_, Message> = if let Some((id, buf)) = &self.renaming_conv {
            if Some(*id) == self.current {
                text_input(i18n::t("title.placeholder"), buf)
                    .on_input(Message::RenameInput)
                    .on_submit(Message::RenameApply)
                    .size(15)
                    .width(Length::Fixed(360.0))
                    .style(design::text_input_style)
                    .into()
            } else {
                text(self.title_text()).size(16).into()
            }
        } else {
            match self.current_conv() {
                Some(conv) => row![
                    text(self.title_text()).size(16),
                    iced::widget::tooltip(
                        button(icons::icon("edit")).on_press(Message::RenameStart(conv.id)),
                        text(i18n::t("title.edit_tooltip")).size(12),
                        iced::widget::tooltip::Position::Bottom,
                    ),
                ]
                .spacing(design::SPACING_XS)
                .align_y(iced::Alignment::Center)
                .into(),
                None => text(self.title_text()).size(16).into(),
            }
        };

        let export_menu: Element<'_, Message> = if self.export_menu_open {
            row![
                button(text(i18n::t("panel.export_main")).size(12))
                    .on_press(Message::Export(ExportScope::Main)),
                button(text(i18n::t("panel.export_all")).size(12))
                    .on_press(Message::Export(ExportScope::AllBranches)),
            ]
            .spacing(design::SPACING_XS)
            .into()
        } else {
            row![].into()
        };
        let export_row = row![
            iced::widget::tooltip(
                button(
                    row![
                        icons::icon("export"),
                        text(i18n::t("panel.export")).size(13)
                    ]
                    .spacing(design::SPACING_XS)
                )
                .on_press(Message::ToggleExportMenu),
                text(i18n::t("panel.export_tooltip")).size(12),
                iced::widget::tooltip::Position::Bottom,
            ),
            iced::widget::tooltip(
                button(icons::icon("panel")).on_press(Message::ToggleSidebar),
                text(i18n::t("panel.sidebar_collapse_tooltip")).size(12),
                iced::widget::tooltip::Position::Bottom,
            ),
            export_menu,
        ]
        .spacing(design::SPACING_XS)
        .align_y(iced::Alignment::Center);

        container(
            row![title_el, Space::new().width(Length::Fill), back, export_row]
                .spacing(design::SPACING_S)
                .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fixed(52.0))
        .padding(Padding::new(design::SPACING_S))
        .style(design::composer_style)
        .into()
    }

    fn title_text(&self) -> String {
        self.current_conv()
            .map(|c| {
                if c.title.trim().is_empty() {
                    i18n::t("conversation.untitled").to_string()
                } else {
                    c.title.clone()
                }
            })
            .unwrap_or_else(|| i18n::t("app.title").to_string())
    }

    fn view_messages(&self) -> Element<'_, Message> {
        let path = self.main_path();
        let tree = self.current_tree().cloned().unwrap_or_default();
        let mut cards: Vec<Element<Message>> = Vec::with_capacity(path.len());
        let mut skip_next = false;
        for (idx, id) in path.iter().enumerate() {
            if skip_next {
                skip_next = false;
                continue;
            }
            let Some(m) = tree.get(*id) else {
                continue;
            };
            if m.role == Role::User {
                let children = tree.children_of(Some(*id));
                if children.len() > 1 {
                    cards.push(self.view_message_card(*id));
                    // 重试尝试聚合:默认只显示未被更新的尝试(B17);
                    // 展开后显示全部(账目按每次 request 独立计费)
                    let expand = self.show_attempts.contains(&m.conversation_id);
                    let visible: Vec<MessageId> = children
                        .iter()
                        .copied()
                        .filter(|c| {
                            expand
                                || tree.get(*c).is_some_and(|cm| cm.role == Role::User)
                                || !self.superseded_by_later_attempt(m.conversation_id, *c)
                        })
                        .collect();
                    let hidden = children.len() - visible.len();
                    let parallel: Vec<Element<Message>> =
                        visible.iter().map(|c| self.view_message_card(*c)).collect();
                    cards.push(
                        container(row(parallel).spacing(design::SPACING_XS))
                            .width(Length::Fill)
                            .into(),
                    );
                    if hidden > 0 {
                        let label = i18n::f("message.show_attempts", &[("n", &hidden.to_string())]);
                        cards.push(
                            button(text(label).size(11))
                                .on_press(Message::ToggleAttempts)
                                .style(design::quiet_button)
                                .into(),
                        );
                    } else if expand {
                        cards.push(
                            button(text(i18n::t("message.hide_attempts")).size(11))
                                .on_press(Message::ToggleAttempts)
                                .style(design::quiet_button)
                                .into(),
                        );
                    }
                    if idx + 1 < path.len() {
                        skip_next = true;
                    }
                    continue;
                }
            }
            cards.push(self.view_message_card(*id));
        }
        if cards.is_empty() {
            let configure: Element<Message> = if self.enabled_models().is_empty() {
                button(
                    row![
                        icons::icon("models"),
                        text(i18n::t("empty.configure")).size(13)
                    ]
                    .spacing(design::SPACING_XS),
                )
                .on_press(Message::Panel(Panels::Models))
                .into()
            } else {
                Space::new().height(Length::Fixed(1.0)).into()
            };
            let empty = column![
                text(i18n::t("empty.title")).size(22),
                text(i18n::t("empty.subtitle"))
                    .size(14)
                    .color(crate::ui_style::palette::TEXT_MUTED),
                configure,
            ]
            .spacing(design::SPACING_M)
            .align_x(iced::Alignment::Center);
            cards.push(container(empty).center(Length::Fill).padding(40).into());
        }
        let list = column(cards)
            .spacing(design::SPACING_M)
            .width(Length::Fill)
            .max_width(860);
        let scroll = scrollable(
            container(list)
                .padding(Padding::new(design::SPACING_L))
                .width(Length::Fill)
                .center_x(Length::Fill),
        )
        .id(iced::widget::Id::new(MESSAGES_ID))
        .height(Length::Fill)
        .width(Length::Fill)
        .on_scroll(|vp: scrollable::Viewport| {
            let offset = vp.absolute_offset().y;
            Message::Viewport {
                content: vp.content_bounds().height,
                view: vp.bounds().height,
                offset,
            }
        });
        scroll.into()
    }

    fn view_message_card(&self, id: MessageId) -> Element<'_, Message> {
        let Some(tree) = self.current_tree() else {
            return row![].into();
        };
        let Some(m) = tree.get(id) else {
            return row![].into();
        };
        let conv_id = m.conversation_id;

        let stream = self.status_of(id);
        let is_user = m.role == Role::User;
        let gen = if is_user {
            None
        } else {
            self.generation_for(conv_id, id)
        };
        let streaming_now = stream
            .as_ref()
            .map(|s| {
                matches!(
                    s.status,
                    GenerationStatus::Queued
                        | GenerationStatus::Connecting
                        | GenerationStatus::Streaming
                )
            })
            .unwrap_or(false);

        // 模型展示名:家族归一化 full_label;raw ID 进 tooltip(6.2)
        let raw_model_id = stream
            .as_ref()
            .map(|s| s.model.clone())
            .or_else(|| gen.as_ref().map(|g| g.model_id.clone()));
        let model_name = raw_model_id.as_deref().map(|mid| {
            let cat_name = self
                .catalog
                .iter()
                .find(|e| e.model_id == mid)
                .map(|e| e.name.clone())
                .unwrap_or_default();
            presentation(mid, &cat_name).full_label
        });
        let role_label = if is_user {
            i18n::t("message.role.user").to_string()
        } else {
            model_name
                .clone()
                .unwrap_or_else(|| i18n::t("message.role.assistant").to_string())
        };
        let status_label = match stream.as_ref().map(|s| s.status) {
            Some(GenerationStatus::Queued) => Some(i18n::t("message.queued")),
            Some(GenerationStatus::Connecting) => Some(i18n::t("message.connecting")),
            Some(GenerationStatus::Streaming) => Some(i18n::t("message.streaming")),
            Some(GenerationStatus::Failed) => Some(i18n::t("message.failed")),
            Some(GenerationStatus::Cancelled) => Some(i18n::t("message.cancelled")),
            Some(GenerationStatus::Interrupted) => Some(i18n::t("message.interrupted")),
            _ => None,
        };
        // 截断/部分回答标注(有可用正文时的含义提示)
        let partial_label = gen.as_ref().and_then(|g| {
            let kind = g.failure.as_ref().map(|f| f.kind());
            match kind {
                Some(crate::domain::FailureKind::Truncated) => {
                    Some(i18n::t("message.truncated").to_string())
                }
                Some(crate::domain::FailureKind::StreamAborted) if !m.content.trim().is_empty() => {
                    Some(i18n::t("message.partial").to_string())
                }
                _ => None,
            }
        });
        // 失败短原因(结构化类别;详情 tooltip 展开,不进正文/上下文)
        let short_reason = gen.as_ref().and_then(|g| {
            g.failure
                .as_ref()
                .map(|f| i18n::t(f.kind().i18n_key()).to_string())
                .or_else(|| g.error.clone().map(|e| ellipsize(&e, 60)))
        });
        let full_error = stream
            .as_ref()
            .and_then(|s| s.error.clone())
            .or_else(|| gen.as_ref().and_then(|g| g.error.clone()));

        let mut header = row![text(role_label)
            .size(12)
            .color(crate::ui_style::palette::TEXT_MUTED)]
        .spacing(design::SPACING_XS)
        .align_y(iced::Alignment::Center);
        // raw ID tooltip(消除正文里的 anthropic/… 噪音;审计可见)
        if let Some(mid) = &raw_model_id {
            if let Some(name) = &model_name {
                if name != mid {
                    header = header.push(iced::widget::tooltip(
                        text(i18n::t("models.id_tooltip").replace("{id}", mid))
                            .size(10)
                            .color(crate::ui_style::palette::TEXT_MUTED),
                        text(mid.clone()).size(10),
                        iced::widget::tooltip::Position::Bottom,
                    ));
                }
            }
        }
        if let Some(s) = status_label {
            header = header.push(text(s).size(11).color(crate::ui_style::palette::WARNING));
        }
        if let Some(p) = partial_label {
            header = header.push(text(p).size(11).color(crate::ui_style::palette::WARNING));
        }
        if let Some(reason) = short_reason {
            let err_el: Element<Message> = if let Some(full) = &full_error {
                iced::widget::tooltip(
                    text(reason).size(11).color(crate::ui_style::palette::ERROR),
                    text(full.clone()).size(11),
                    iced::widget::tooltip::Position::Bottom,
                )
                .into()
            } else {
                text(reason)
                    .size(11)
                    .color(crate::ui_style::palette::ERROR)
                    .into()
            };
            header = header.push(err_el);
        }
        header = header.push(Space::new().width(Length::Fill));
        header = header.push(iced::widget::tooltip(
            button(icons::icon("copy")).on_press(Message::Copy(id)),
            text(i18n::t("message.copy_tooltip")).size(12),
            iced::widget::tooltip::Position::Top,
        ));
        if streaming_now {
            let req = stream.as_ref().unwrap().request_id;
            header = header.push(iced::widget::tooltip(
                button(icons::icon("stop")).on_press(Message::Stop(req)),
                text(i18n::t("message.stop")).size(12),
                iced::widget::tooltip::Position::Top,
            ));
        } else if !is_user {
            // 重试按钮:资格门禁(冷却倒计时/补救;服务层兜底)
            let decision = gen.as_ref().map(|g| {
                let root = g.retry_root_id.unwrap_or(g.request_id);
                let lineage_active = self
                    .generations
                    .get(&conv_id)
                    .map(|gs| {
                        gs.iter()
                            .any(|x| x.retry_root_id == Some(root) && !x.status.is_terminal())
                    })
                    .unwrap_or(false);
                let latest_failure: Option<FailureInfo> =
                    self.generations.get(&conv_id).and_then(|gs| {
                        gs.iter()
                            .filter(|x| x.retry_root_id == Some(root) && x.failure.is_some())
                            .max_by_key(|x| x.finished_at.unwrap_or_else(crate::storage::now_dt))
                            .and_then(|x| x.failure.clone())
                    });
                retry_decision(
                    g.status,
                    latest_failure.as_ref(),
                    crate::domain::recovery::now_ms(),
                    lineage_active,
                )
            });
            let completed = gen
                .as_ref()
                .map(|g| g.status == GenerationStatus::Completed)
                .unwrap_or(false);
            let retry_tip = match &decision {
                Some(RetryDecision::Allowed) => {
                    if completed {
                        i18n::t("message.regenerate").to_string()
                    } else {
                        i18n::t("message.retry").to_string()
                    }
                }
                Some(RetryDecision::InFlight) => i18n::t("retry.in_flight").to_string(),
                Some(RetryDecision::Cooldown(until)) => {
                    let secs = ((until - crate::domain::recovery::now_ms()) / 1000).max(0);
                    i18n::f("retry.cooldown", &[("s", &secs.to_string())])
                }
                Some(RetryDecision::NeedsAction(remedy)) => {
                    let key = match remedy {
                        crate::domain::Remedy::SwitchIp => "retry.needs.switch_ip",
                        crate::domain::Remedy::Credentials => "retry.needs.credentials",
                        crate::domain::Remedy::Balance => "retry.needs.balance",
                        crate::domain::Remedy::EditInput => "retry.needs.edit_input",
                        crate::domain::Remedy::PickModel => "retry.needs.pick_model",
                        crate::domain::Remedy::None => "retry.needs.none",
                    };
                    i18n::t(key).to_string()
                }
                None => i18n::t("message.retry").to_string(),
            };
            let allowed = matches!(decision, Some(RetryDecision::Allowed) | None);
            let mut retry_btn = button(icons::icon("retry")).style(design::quiet_button);
            if allowed {
                retry_btn = retry_btn.on_press(Message::Retry(id));
            }
            header = header.push(iced::widget::tooltip(
                retry_btn,
                text(retry_tip).size(12),
                iced::widget::tooltip::Position::Top,
            ));
            if completed && self.current_conv().and_then(|c| c.main_leaf_id) != Some(id) {
                header = header.push(iced::widget::tooltip(
                    button(icons::icon("continue")).on_press(Message::ContinueFrom(id)),
                    text(i18n::t("message.continue_tooltip")).size(12),
                    iced::widget::tooltip::Position::Top,
                ));
            }
            if completed {
                let checked = self.branch_selection.contains(&id);
                header = header.push(iced::widget::tooltip(
                    checkbox(checked)
                        .on_toggle(move |_| Message::ToggleBranchSelect(id))
                        .size(14),
                    text(i18n::t("message.select_branch_tooltip")).size(12),
                    iced::widget::tooltip::Position::Top,
                ));
            }
            // 尝试数(lineage 聚合;默认显示最新,旧尝试可展开)
            let attempts = self.lineage_count_of(conv_id, id);
            if attempts > 1 {
                header = header.push(
                    text(i18n::f("message.attempts", &[("n", &attempts.to_string())]))
                        .size(10)
                        .color(crate::ui_style::palette::TEXT_MUTED),
                );
            }
        }

        let body: Element<Message> = if m.content.is_empty() && streaming_now {
            text("…").size(self.style.body).into()
        } else if let Some(rendered) = self.renders.get(&id) {
            let sel_ref = self
                .selection
                .as_ref()
                .filter(|(mid, _)| *mid == id)
                .map(|(_, s)| s);
            let hooks = self.selection_hooks(id);
            let on_link: view::OnLink<Message> = Arc::new(|url: String| Message::LinkClicked(url));
            view::view_message(rendered, self.style, sel_ref, hooks, on_link)
        } else {
            text(m.content.clone()).size(self.style.body).into()
        };

        let collapsible = is_user && should_collapse(&m.content);
        let expanded = collapsible
            && self
                .expanded_user
                .get(&conv_id)
                .map(|s| s.contains(&id))
                .unwrap_or(false);
        let body_elem: Element<Message> = if collapsible {
            let expand_msg = Message::ToggleExpand(id);
            let tip = if expanded {
                i18n::t("message.collapse")
            } else {
                i18n::t("message.expand")
            };
            let chevron = if expanded { "up" } else { "down" };
            let clipped: Element<Message> = if expanded {
                body
            } else {
                container(body)
                    .width(Length::Fill)
                    .height(Length::Fixed(COLLAPSED_HEIGHT))
                    .clip(true)
                    .into()
            };
            column![
                clipped,
                container(iced::widget::tooltip(
                    button(icons::icon(chevron)).on_press(expand_msg),
                    text(tip).size(12),
                    iced::widget::tooltip::Position::Top,
                ))
                .center_x(Length::Fill)
                .height(Length::Fixed(24.0)),
            ]
            .spacing(design::SPACING_XS)
            .width(Length::Fill)
            .into()
        } else {
            body
        };

        let mut card = column![container(header.wrap()).width(Length::Fill), body_elem]
            .spacing(design::SPACING_XS)
            .width(Length::Fill);
        if is_user && !m.attachments.is_empty() {
            let mut chips = row![].spacing(design::SPACING_XS);
            for a in &m.attachments {
                chips = chips.push(
                    container(
                        row![
                            icons::icon("attach"),
                            text(ellipsize(&a.name, 26)).size(11),
                            text(size_label(a.content.len()))
                                .size(10)
                                .color(crate::ui_style::palette::TEXT_MUTED),
                        ]
                        .spacing(design::SPACING_XS)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding(Padding::new(3.0))
                    .style(design::chip_style),
                );
            }
            card = card.push(chips);
        }

        container(card)
            .width(Length::Fill)
            .padding(Padding::new(design::SPACING_M))
            .style(design::card_style(is_user))
            .into()
    }

    fn view_input(&self) -> Element<'_, Message> {
        // 门禁弹窗打开时:主区底部显示弹窗卡片(替代输入区;Esc/取消关闭)
        if self.egress_modal.is_some() {
            return self.view_egress_modal();
        }
        let snapshot = self.normalized_selection();
        let enabled = self.enabled_models();

        let display = |m: &str| -> String {
            // 收起按钮/卡片显示完整易读家族名(无公司前缀;6.2)
            let cat_name = self
                .catalog
                .iter()
                .find(|e| e.model_id == m)
                .map(|e| e.name.clone())
                .unwrap_or_default();
            presentation(m, &cat_name).full_label
        };
        let label = if enabled.is_empty() {
            i18n::t("composer.model_picker").to_string()
        } else if self.model_sel.parallel {
            let primary = snapshot
                .primary
                .as_deref()
                .map(display)
                .unwrap_or_else(|| "?".into());
            i18n::f(
                "selector.primary_plus",
                &[
                    ("primary", &ellipsize(&primary, 18)),
                    ("n", &snapshot.targets.len().to_string()),
                ],
            )
        } else {
            snapshot
                .primary
                .as_deref()
                .map(|m| ellipsize(&display(m), 24))
                .unwrap_or_else(|| i18n::t("composer.model_picker").to_string())
        };
        let selector_tooltip = if enabled.len() < 2 && !self.model_sel.parallel {
            text(i18n::t("composer.models_single_tooltip")).size(12)
        } else {
            text(i18n::f(
                "composer.models_tooltip",
                &[("models", &snapshot.targets.join(", "))],
            ))
            .size(12)
        };
        let selector_button = iced::widget::tooltip(
            button(
                row![
                    icons::icon("models"),
                    text(label).size(13),
                    icons::icon(if self.model_sel.picker_open {
                        "up"
                    } else {
                        "down"
                    }),
                ]
                .spacing(design::SPACING_XS)
                .align_y(iced::Alignment::Center),
            )
            .on_press_maybe((!enabled.is_empty()).then_some(Message::TogglePicker))
            .style(design::quiet_button),
            selector_tooltip,
            iced::widget::tooltip::Position::Top,
        );

        let picker: Element<Message> = if self.model_sel.picker_open && !enabled.is_empty() {
            let branch_mode = !self.branch_selection.is_empty();
            let mut list = column![].spacing(2);
            if branch_mode {
                list = list.push(
                    text(i18n::t("composer.branch_mode_hint"))
                        .size(11)
                        .color(crate::ui_style::palette::WARNING),
                );
            }
            // 家族分组(组标题非交互、小号 muted;组内短名;6.1/6.2)
            let groups = model_picker::build_groups(&enabled, &self.catalog);
            let filtered = model_picker::filter_groups(&groups, &self.picker_search);
            if filtered.is_empty() {
                list = list.push(
                    text(i18n::f(
                        "selector.no_results",
                        &[("query", self.picker_search.trim())],
                    ))
                    .size(11)
                    .color(crate::ui_style::palette::TEXT_MUTED),
                );
            }
            for group in filtered {
                if !self.picker_search.trim().is_empty() {
                    // 搜索模式:显示"家族"行内联,减少组标题堆积
                    list = list.push(
                        text(group.label.clone())
                            .size(10)
                            .color(crate::ui_style::palette::TEXT_MUTED),
                    );
                } else {
                    list = list.push(
                        container(
                            text(group.label.clone())
                                .size(10)
                                .color(crate::ui_style::palette::TEXT_MUTED),
                        )
                        .padding(Padding {
                            top: 6.0,
                            bottom: 2.0,
                            left: 2.0,
                            right: 2.0,
                        }),
                    );
                }
                for item in &group.items {
                    let m_owned = item.model_id.clone();
                    let m_for_toggle = m_owned.clone();
                    let is_primary = snapshot.primary.as_deref() == Some(item.model_id.as_str());
                    if self.model_sel.parallel {
                        let checked = self.model_sel.checked.contains(&m_owned);
                        list = list.push(
                            row![
                                checkbox(checked)
                                    .label(ellipsize(&item.label, 26))
                                    .on_toggle(move |_| {
                                        Message::ToggleChecked(m_for_toggle.clone())
                                    })
                                    .size(14)
                                    .text_size(12),
                                if is_primary {
                                    text(i18n::t("composer.primary_badge"))
                                        .size(10)
                                        .color(crate::ui_style::palette::ACCENT)
                                } else {
                                    text("").size(10)
                                },
                                Space::new().width(Length::Fill),
                                iced::widget::tooltip(
                                    button(icons::icon("star"))
                                        .on_press(Message::SetPrimary(m_owned.clone())),
                                    text(i18n::t("composer.primary_tooltip")).size(12),
                                    iced::widget::tooltip::Position::Left,
                                ),
                            ]
                            .spacing(design::SPACING_XS)
                            .align_y(iced::Alignment::Center)
                            .padding(Padding::new(2.0)),
                        );
                    } else if branch_mode {
                        let btn = button(
                            row![
                                if is_primary {
                                    icons::icon("check")
                                } else {
                                    icons::icon("blank")
                                },
                                text(ellipsize(&item.label, 28)).size(12),
                            ]
                            .spacing(design::SPACING_XS),
                        )
                        .style(design::quiet_button);
                        list = list.push(btn);
                    } else {
                        let btn = button(
                            row![
                                if is_primary {
                                    icons::icon("check")
                                } else {
                                    icons::icon("blank")
                                },
                                text(ellipsize(&item.label, 28)).size(12),
                            ]
                            .spacing(design::SPACING_XS),
                        )
                        .style(design::quiet_button)
                        .on_press(Message::PickSingle(m_owned));
                        list = list.push(btn);
                    }
                }
            }
            let hint = if self.model_sel.parallel {
                i18n::t("selector.multi_hint")
            } else {
                i18n::t("selector.single_hint")
            };
            // 自适应高度:桌面约 150–300,按窗口高度缩放(6.2;D04/D07)
            let picker_height = (self.window_size.1 * 0.35).clamp(150.0, 300.0);
            column![
                row![
                    checkbox(self.model_sel.parallel)
                        .label(i18n::t("composer.parallel"))
                        .on_toggle(|_| Message::ToggleParallelMode)
                        .size(14)
                        .text_size(12),
                    Space::new().width(Length::Fill),
                    text(hint)
                        .size(10)
                        .color(crate::ui_style::palette::TEXT_MUTED),
                ]
                .spacing(design::SPACING_XS)
                .align_y(iced::Alignment::Center),
                text_input(i18n::t("selector.search_placeholder"), &self.picker_search,)
                    .on_input(Message::PickerSearchChanged)
                    .size(12)
                    .style(design::text_input_style),
                scrollable(list)
                    .width(Length::Fill)
                    .height(Length::Fixed(picker_height)),
            ]
            .spacing(design::SPACING_XS)
            .padding(Padding::new(design::SPACING_XS))
            .width(Length::Fill)
            .into()
        } else {
            column![].into()
        };

        let editor = container(
            text_editor(&self.editor)
                .on_action(Message::InputEdited)
                .height(Length::Shrink)
                .min_height(COMPOSER_MIN)
                .max_height((self.window_size.1 * 0.30).min(COMPOSER_MAX))
                .padding(8)
                .line_height(iced::advanced::text::LineHeight::Relative(
                    crate::ui_style::BODY_LINE_HEIGHT,
                ))
                .wrapping(iced::advanced::text::Wrapping::Word)
                .id(iced::widget::Id::new(COMPOSER_ID)),
        )
        .width(Length::Fill)
        .height(Length::Shrink);

        let chips: Element<Message> = if self.pending_attachments.is_empty() {
            column![].into()
        } else {
            let mut r = row![].spacing(design::SPACING_XS);
            for (i, a) in self.pending_attachments.iter().enumerate() {
                r = r.push(
                    container(
                        row![
                            icons::icon("attach"),
                            text(ellipsize(&a.name, 22)).size(11),
                            text(size_label(a.content.len()))
                                .size(10)
                                .color(crate::ui_style::palette::TEXT_MUTED),
                            button(icons::icon("x"))
                                .on_press(Message::RemoveAttachment(i))
                                .padding(1),
                        ]
                        .spacing(design::SPACING_XS)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding(Padding::new(4.0))
                    .style(design::chip_style),
                );
            }
            scrollable(r)
                .direction(iced::widget::scrollable::Direction::Horizontal(
                    iced::widget::scrollable::Scrollbar::default(),
                ))
                .width(Length::Fill)
                .into()
        };

        let any_streaming = self.streaming.values().any(|s| !s.status.is_terminal());
        let action: Element<Message> = if any_streaming {
            let mut col = column![];
            for s in self.streaming.values() {
                if !s.status.is_terminal() {
                    col = col.push(iced::widget::tooltip(
                        button(
                            row![icons::icon("stop"), text(i18n::t("message.stop")).size(13)]
                                .spacing(design::SPACING_XS),
                        )
                        .on_press(Message::Stop(s.request_id))
                        .style(design::quiet_button),
                        text(i18n::t("composer.stop_tooltip")).size(12),
                        iced::widget::tooltip::Position::Top,
                    ));
                }
            }
            col.into()
        } else {
            let sendable = snapshot.invalid_reason.is_none();
            let mut btn = button(
                row![icons::icon("send"), text(i18n::t("composer.send")).size(13)]
                    .spacing(design::SPACING_XS),
            )
            .style(design::accent_button);
            if sendable {
                btn = btn.on_press(Message::Send);
            }
            let tip = match snapshot.invalid_reason {
                Some(reason) => text(i18n::t(reason)).size(12),
                None => text(i18n::t("composer.send_tooltip")).size(12),
            };
            iced::widget::tooltip(btn, tip, iced::widget::tooltip::Position::Top).into()
        };

        let usage_button = iced::widget::tooltip(
            button(
                row![
                    icons::icon("usage"),
                    text(i18n::t("composer.usage")).size(12)
                ]
                .spacing(5),
            )
            .on_press(Message::ToggleUsage),
            text(i18n::t("composer.usage_tooltip")).size(12),
            iced::widget::tooltip::Position::Top,
        );
        let attach_button = iced::widget::tooltip(
            button(
                row![
                    icons::icon("attach"),
                    text(i18n::t("composer.attach")).size(12)
                ]
                .spacing(5),
            )
            .on_press(Message::PickAttachments),
            text(i18n::t("composer.attach_tooltip")).size(12),
            iced::widget::tooltip::Position::Top,
        );

        let toolbar = container(
            row![
                selector_button,
                Space::new().width(Length::Fill),
                attach_button,
                usage_button,
                action,
            ]
            .spacing(design::SPACING_M)
            .align_y(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Shrink);

        let toast = self.toast.clone().unwrap_or_default();
        let toast_el: Element<Message> = if !toast.is_empty() {
            text(toast)
                .size(11)
                .color(crate::ui_style::palette::SUCCESS)
                .into()
        } else if let Some(err) = self.toast_error.clone() {
            text(err)
                .size(11)
                .color(crate::ui_style::palette::ERROR)
                .into()
        } else {
            text("").size(11).into()
        };
        let usage_panel: Element<Message> = if self.usage_expanded {
            let usage = self.current.and_then(|conv| self.usage_cache.get(&conv));
            let summary = usage
                .map(|items| crate::domain::CostSummary::from_records(items.iter()))
                .unwrap_or_default();
            let aux = self
                .current
                .and_then(|conv| self.aux_usage_cache.get(&conv))
                .copied()
                .unwrap_or((0, 0));
            column![
                text(i18n::f(
                    "usage.known_pending",
                    &[
                        ("known", &summary.known_total.to_string()),
                        ("pending", &summary.pending_count.to_string()),
                        ("unavailable", &summary.unavailable_count.to_string()),
                    ],
                ))
                .size(11),
                text(i18n::f(
                    "settings.title_requests",
                    &[
                        ("known", &aux.0.to_string()),
                        ("unknown", &aux.1.to_string())
                    ],
                ))
                .size(11)
                .color(crate::ui_style::palette::TEXT_MUTED),
                text(self.key_status_detail.clone())
                    .size(11)
                    .color(crate::ui_style::palette::TEXT_MUTED),
            ]
            .spacing(3)
            .into()
        } else {
            column![].into()
        };

        let input = column![editor, chips, toolbar, picker, toast_el, usage_panel,]
            .spacing(design::SPACING_XS)
            .padding(Padding::new(10.0))
            .height(Length::Shrink);
        container(input)
            .width(Length::Fill)
            .height(Length::Shrink)
            .style(design::composer_style)
            .into()
    }

    /// Anthropic 出口门禁弹窗(替代输入区渲染;Esc/Cancel 关闭)。
    /// 单弹窗按 intent 去重;说明明确覆盖全部 Anthropic 模型(不只 Claude)。
    fn view_egress_modal(&self) -> Element<'_, Message> {
        let Some(modal) = &self.egress_modal else {
            return row![].into();
        };
        // 观测行:Unknown 无观测时不显示占位噪声
        let obs_line = modal.observation.as_ref().map(|o| {
            i18n::f(
                "egress.observation",
                &[
                    ("ip", o.ip.as_deref().unwrap_or("?")),
                    ("country", o.country_code.as_deref().unwrap_or("?")),
                    ("source", o.source),
                    ("time", &o.checked_at.format("%H:%M:%S").to_string()),
                ],
            )
        });
        let title = if modal.unknown {
            i18n::t("egress.body.unknown")
        } else {
            i18n::t("egress.body.blocked")
        };
        let mut body = column![text(i18n::t("egress.title")).size(15), text(title).size(12)]
            .spacing(design::SPACING_XS);
        if let Some(line) = obs_line {
            body = body.push(
                text(line)
                    .size(11)
                    .color(crate::ui_style::palette::TEXT_MUTED),
            );
        }
        body = body.push(if modal.unknown {
            text(modal.detail.clone())
                .size(11)
                .color(crate::ui_style::palette::TEXT_MUTED)
        } else {
            text("").size(11)
        });
        body = body.push(
            text(i18n::t("egress.applies_to"))
                .size(11)
                .color(crate::ui_style::palette::TEXT_MUTED),
        );
        body = body.spacing(design::SPACING_S);

        let mut buttons = row![
            button(
                text(if modal.checking {
                    i18n::t("egress.rechecking").to_string()
                } else {
                    i18n::t("egress.recheck").to_string()
                })
                .size(13)
            )
            .on_press_maybe(
                (!modal.checking).then_some(Message::EgressModal(EgressModalAction::Recheck,))
            )
            .style(design::quiet_button),
            button(text(i18n::t("egress.cancel")).size(13))
                .on_press(Message::EgressModal(EgressModalAction::Dismiss))
                .style(design::quiet_button),
        ]
        .spacing(design::SPACING_S);
        // 混合目标:显式移除全部 Anthropic 模型后发送剩余目标(A06)
        if modal.mixed {
            buttons = buttons.push(
                button(text(i18n::t("egress.remove_anthropic")).size(13))
                    .on_press(Message::EgressModal(
                        EgressModalAction::RemoveAnthropicAndSend,
                    ))
                    .style(design::accent_button),
            );
        }
        body = body.push(buttons);

        container(
            container(body)
                .padding(Padding::new(design::SPACING_L))
                .style(design::composer_style)
                .max_width(560),
        )
        .width(Length::Fill)
        .height(Length::Shrink)
        .center_x(Length::Fill)
        .padding(Padding::new(10.0))
        .into()
    }

    fn view_sidebar(&self) -> Element<'_, Message> {
        let tab = |p: Panels, icon: &'static str, label: &str| {
            button(row![icons::icon(icon), text(label.to_string()).size(13)].spacing(5))
                .on_press(Message::Panel(p))
                .style(if self.panel == p {
                    design::active_button
                } else {
                    design::quiet_button
                })
        };
        let tabs = row![
            tab(Panels::History, "history", i18n::t("panel.history")),
            tab(Panels::Models, "models", i18n::t("panel.models")),
            tab(Panels::Settings, "settings", i18n::t("panel.settings")),
        ]
        .spacing(design::SPACING_XS);

        let body = match self.panel {
            Panels::History => self.view_history(),
            Panels::Models => self.view_models(),
            Panels::Settings => self.view_settings(),
        };

        let content: Element<'_, Message> =
            column![row![tabs].spacing(design::SPACING_XS).padding(6), body]
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(design::sidebar_style)
            .into()
    }

    fn view_history(&self) -> Element<'_, Message> {
        let mut col = column![].spacing(design::SPACING_XS).padding(6);

        col = col.push(
            row![
                button(text(i18n::t("conversation.new_button")).size(12))
                    .on_press(Message::NewConversation),
                Space::new().width(Length::Fill),
            ]
            .spacing(design::SPACING_XS),
        );
        col = col.push(
            text_input(
                i18n::t("conversation.search_placeholder"),
                &self.conv_search,
            )
            .on_input(Message::ConvSearchChanged)
            .size(13)
            .style(design::text_input_style),
        );

        #[allow(clippy::redundant_closure)]
        let filter = |c: &Conversation| {
            self.conv_search.is_empty()
                || c.title.contains(&self.conv_search)
                || c.draft.contains(&self.conv_search)
        };
        for (name, group_id) in std::iter::once((i18n::t("conversation.group_ungrouped"), None))
            .chain(self.groups.iter().map(|g| (g.name.as_str(), Some(g.id))))
        {
            let convs: Vec<&Conversation> = self
                .conversations
                .iter()
                .filter(|c| c.group_id == group_id && filter(c))
                .collect();
            let mut g = row![
                icons::icon("chevron"),
                text(name.to_string())
                    .size(12)
                    .color(crate::ui_style::palette::TEXT_MUTED),
            ]
            .spacing(design::SPACING_XS)
            .padding(2);
            if let Some(id) = group_id {
                g = g.push(Space::new().width(Length::Fill));
                g = g.push(iced::widget::tooltip(
                    button(icons::icon("edit")).on_press(Message::RenameGroupStart(id)),
                    text(i18n::t("group.rename")).size(12),
                    iced::widget::tooltip::Position::Left,
                ));
                g = g.push(iced::widget::tooltip(
                    button(icons::icon("x"))
                        .on_press(Message::DeleteGroup(id))
                        .style(design::danger_button),
                    text(i18n::t("group.delete")).size(12),
                    iced::widget::tooltip::Position::Left,
                ));
            }
            col = col.push(g);
            for c in convs {
                let is_current = self.current == Some(c.id);
                let title = if c.title.trim().is_empty() {
                    i18n::f(
                        "conversation.fallback_date",
                        &[("date", &c.created_at.format("%m-%d %H:%M").to_string())],
                    )
                } else {
                    c.title.clone()
                };
                let label = text(title).size(13);
                let item = row![
                    button(row![icons::icon("folder"), label].spacing(6))
                        .on_press(Message::SelectConversation(c.id))
                        .style(if is_current {
                            design::active_button
                        } else {
                            design::quiet_button
                        }),
                    Space::new().width(Length::Fill),
                    iced::widget::tooltip(
                        button(icons::icon("edit")).on_press(Message::RenameStart(c.id)),
                        text(i18n::t("title.edit_tooltip")).size(12),
                        iced::widget::tooltip::Position::Left,
                    ),
                ]
                .spacing(2)
                .padding(1);
                col = col.push(item);
            }
        }

        // 分组管理
        col = col.push(Space::new().width(Length::Fill));
        col = col.push(
            text(i18n::t("group.new"))
                .size(12)
                .color(crate::ui_style::palette::TEXT_MUTED),
        );
        col = col.push(
            row![
                text_input(i18n::t("group.new_placeholder"), &self.new_group_name)
                    .on_input(Message::NewGroupInput)
                    .on_submit(Message::NewGroupApply)
                    .size(13)
                    .style(design::text_input_style),
                button(text(i18n::t("group.create")).size(12)).on_press(Message::NewGroupApply),
            ]
            .spacing(design::SPACING_XS),
        );

        if let Some((group_id, buf)) = &self.renaming_group {
            let group_id = *group_id;
            col = col.push(
                row![
                    text_input(i18n::t("title.placeholder"), buf)
                        .on_input(move |s| Message::RenameGroupInput(group_id, s))
                        .on_submit(Message::RenameGroupApply)
                        .size(13)
                        .style(design::text_input_style),
                    button(text(i18n::t("group.save")).size(11))
                        .on_press(Message::RenameGroupApply),
                ]
                .spacing(design::SPACING_XS),
            );
        }

        // 会话重命名(侧栏;主区标题同组件)
        if let Some((id, buf)) = &self.renaming_conv {
            if Some(*id) != self.current {
                col = col.push(
                    row![
                        text_input(i18n::t("title.placeholder"), buf)
                            .on_input(Message::RenameInput)
                            .on_submit(Message::RenameApply)
                            .size(13)
                            .style(design::text_input_style),
                        button(text(i18n::t("group.save")).size(12)).on_press(Message::RenameApply),
                    ]
                    .spacing(design::SPACING_XS),
                );
            }
        }
        if let Some(current) = self.current {
            let mut move_row =
                row![text(i18n::t("conversation.move_to")).size(11)].spacing(design::SPACING_XS);
            move_row = move_row.push(
                button(text(i18n::t("conversation.group_ungrouped")).size(10))
                    .on_press(Message::MoveToGroup(current, None)),
            );
            for g in &self.groups {
                move_row = move_row.push(
                    button(text(g.name.clone()).size(10))
                        .on_press(Message::MoveToGroup(current, Some(g.id))),
                );
            }
            col = col.push(move_row);
            col = col.push(
                button(text(i18n::t("conversation.delete")).size(11))
                    .on_press(Message::DeleteConversation(current))
                    .style(design::danger_button),
            );
        }

        scrollable(col)
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    }

    fn view_models(&self) -> Element<'_, Message> {
        let mut col = column![].spacing(6).padding(6);

        col = col.push(
            row![
                button(text(i18n::t("models.test_connection")).size(12))
                    .on_press(Message::TestConnection),
                button(text(i18n::t("models.refresh_catalog")).size(12))
                    .on_press(Message::RefreshCatalog),
            ]
            .spacing(design::SPACING_XS),
        );
        if !self.models_status.is_empty() {
            col = col.push(
                text(self.models_status.clone())
                    .size(11)
                    .color(crate::ui_style::palette::SUCCESS),
            );
        }

        col = col.push(
            text_input(i18n::t("models.search_placeholder"), &self.model_search)
                .on_input(Message::ModelSearchChanged)
                .size(13)
                .style(design::text_input_style),
        );

        col = col.push(
            text(i18n::t("models.mine"))
                .size(12)
                .color(crate::ui_style::palette::TEXT_MUTED),
        );
        let enabled_sorted: Vec<(String, ModelPreference)> = {
            let mut v: Vec<(String, ModelPreference)> = self
                .prefs
                .iter()
                .filter(|(_, p)| p.enabled)
                .map(|(m, p)| (m.clone(), p.clone()))
                .collect();
            v.sort_by_key(|(_, p)| p.sort_order);
            v
        };
        let no_enabled = enabled_sorted.is_empty();
        for (m, pref) in enabled_sorted {
            // 侧栏同一规范化名(6.2);ID 作为次级/详情行保留
            let cat_name = self
                .catalog
                .iter()
                .find(|e| e.model_id == m)
                .map(|e| e.name.clone())
                .unwrap_or_default();
            let pres = presentation(&m, &cat_name);
            let name = pres.full_label.clone();
            let mut r = row![
                text(ellipsize(&name, 20)).size(12),
                Space::new().width(Length::Fill),
            ]
            .spacing(design::SPACING_XS)
            .align_y(iced::Alignment::Center);
            r = r.push(iced::widget::tooltip(
                button(icons::icon("up")).on_press(Message::MoveModel(m.clone(), -1)),
                text(i18n::t("models.move_up_tooltip")).size(12),
                iced::widget::tooltip::Position::Left,
            ));
            r = r.push(iced::widget::tooltip(
                button(icons::icon("down")).on_press(Message::MoveModel(m.clone(), 1)),
                text(i18n::t("models.move_down_tooltip")).size(12),
                iced::widget::tooltip::Position::Left,
            ));
            col = col.push(r);
            col = col.push(
                text(m.clone())
                    .size(10)
                    .color(crate::ui_style::palette::TEXT_MUTED),
            );
            let m_toggle = m.clone();
            col = col.push(
                row![
                    checkbox(true)
                        .label(i18n::t("models.enabled"))
                        .on_toggle(move |_| Message::ToggleModel(m_toggle.clone()))
                        .size(14)
                        .text_size(13),
                    if pref.is_default {
                        row![
                            icons::icon("check"),
                            text(i18n::t("models.default"))
                                .size(11)
                                .color(crate::ui_style::palette::ACCENT),
                        ]
                        .spacing(3)
                        .into()
                    } else {
                        Element::<Message>::from(
                            button(text(i18n::t("models.set_default")).size(10))
                                .on_press(Message::SetDefault(m.clone())),
                        )
                    },
                ]
                .spacing(design::SPACING_S),
            );
        }
        if no_enabled {
            col = col.push(
                text(i18n::t("models.none_enabled"))
                    .size(11)
                    .color(crate::ui_style::palette::TEXT_MUTED),
            );
        }
        col = col.push(
            text(i18n::f(
                "models.catalog",
                &[("n", &self.catalog.len().to_string())],
            ))
            .size(12)
            .color(crate::ui_style::palette::TEXT_MUTED),
        );
        let search = self.model_search.to_lowercase();
        for e in self.catalog.iter().filter(|e| {
            if search.is_empty() {
                return true;
            }
            // 与选择器同一搜索规则:目录原名/raw ID/短名/完整名(6.2)
            let pres = presentation(&e.model_id, &e.name);
            e.model_id.to_lowercase().contains(&search)
                || e.name.to_lowercase().contains(&search)
                || pres.item_label.to_lowercase().contains(&search)
                || pres.full_label.to_lowercase().contains(&search)
        }) {
            let pref = self.prefs.get(&e.model_id).cloned().unwrap_or_default();
            let price = format_price(&e.pricing);
            let pres = presentation(&e.model_id, &e.name);
            let display_name = pres.full_label.clone();
            let raw_id = e.model_id.clone();
            let mut r = row![
                checkbox(pref.enabled)
                    .label(ellipsize(&display_name, 22))
                    .on_toggle(move |_| Message::ToggleModel(raw_id.clone()))
                    .size(14)
                    .text_size(13),
                Space::new().width(Length::Fill),
                iced::widget::tooltip(
                    text(i18n::t("models.price"))
                        .size(10)
                        .color(crate::ui_style::palette::TEXT_MUTED),
                    text(price).size(10),
                    iced::widget::tooltip::Position::FollowCursor,
                ),
            ]
            .spacing(design::SPACING_XS)
            .align_y(iced::Alignment::Center);
            if snapshot_primary_is(self, &e.model_id) {
                r = r.push(
                    row![
                        icons::icon("check"),
                        text(i18n::t("models.current"))
                            .size(10)
                            .color(crate::ui_style::palette::ACCENT),
                    ]
                    .spacing(3),
                );
            }
            col = col.push(r);
        }
        scrollable(col)
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    }

    fn view_settings(&self) -> Element<'_, Message> {
        let data_dir = storage::repo::data_dir().map_or_else(
            |e| {
                i18n::f(
                    "settings.data_dir_unavailable",
                    &[("error", &e.to_string())],
                )
            },
            |p| p.display().to_string(),
        );
        let has_key = self.vault.load("openrouter").ok().flatten().is_some() || self.key_saved;
        let col = column![
            text(i18n::t("settings.title")).size(14),
            text(i18n::t("settings.font_size")).size(12),
            row![
                slider(
                    crate::ui_style::BODY_SIZE_MIN..=crate::ui_style::BODY_SIZE_MAX,
                    self.style.body,
                    Message::FontSize,
                )
                .width(Length::Fill),
                text(i18n::f(
                    "settings.font_size_value",
                    &[("n", &format!("{}", self.style.body as i32))],
                ))
                .size(12),
            ]
            .spacing(design::SPACING_S)
            .align_y(iced::Alignment::Center),
            text(i18n::t("settings.theme"))
                .size(11)
                .color(crate::ui_style::palette::TEXT_MUTED),
            text(i18n::t("settings.api_key")).size(12),
            row![
                text_input(i18n::t("models.key_placeholder"), &self.key_input)
                    .on_input(Message::KeyInput)
                    .on_submit(Message::SaveKey)
                    .secure(true)
                    .size(13)
                    .style(design::text_input_style),
                button(text(i18n::t("settings.save")).size(12)).on_press(Message::SaveKey),
            ]
            .spacing(design::SPACING_XS),
            text(if has_key {
                i18n::t("settings.configured")
            } else {
                i18n::t("settings.not_configured")
            })
            .size(11),
            text(i18n::t("settings.auto_title"))
                .size(11)
                .color(crate::ui_style::palette::TEXT_MUTED),
            text(i18n::t("settings.data_dir")).size(12),
            text(data_dir)
                .size(11)
                .color(crate::ui_style::palette::TEXT_MUTED),
            text(i18n::t("settings.concurrency")).size(12),
            text(i18n::t("settings.assumed_output")).size(12),
            text(if cfg!(windows) {
                i18n::t("settings.credential_windows")
            } else {
                i18n::t("settings.credential_volatile")
            })
            .size(11)
            .color(crate::ui_style::palette::TEXT_MUTED),
            text(i18n::t("settings.export_note"))
                .size(11)
                .color(crate::ui_style::palette::TEXT_MUTED),
        ]
        .spacing(design::SPACING_S)
        .padding(6);
        scrollable(col)
            .height(Length::Fill)
            .width(Length::Fill)
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        if std::env::var("AICOUNCIL_NO_SUB").is_ok() {
            return Subscription::none();
        }
        let _bus = &self.bus;
        let bus = self.bus.clone();
        let events = Subscription::run_with(bus, |bus: &EventBus| {
            let rx = bus.0.subscribe();
            async_stream::stream! {
                let mut rx = rx;
                let mut reconcile_at: Option<tokio::time::Instant> = None;
                loop {
                    match rx.recv().await {
                        Ok(ev) => yield Message::Provider(ev),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(skipped = n, "事件积压(数据库为准)");
                            // 受节流的 DB 对账:不能跳过后永远等待丢失的终态(B14)
                            let now = tokio::time::Instant::now();
                            let fire = reconcile_at.is_none_or(|t| now >= t);
                            if fire {
                                reconcile_at = Some(now + std::time::Duration::from_millis(500));
                                yield Message::Reconcile;
                            } else {
                                continue;
                            }
                        }
                        Err(_) => break,
                    }
                }
            }
        });
        let title_bus = self.title_bus.clone();
        let titles = Subscription::run_with(title_bus, |bus: &TitleBus| {
            let rx = bus.0.subscribe();
            async_stream::stream! {
                let mut rx = rx;
                loop {
                    match rx.recv().await {
                        Ok(ev) => yield Message::TitleFinished(ev),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
            }
        });
        let keys = iced::keyboard::listen().map(|ev| match ev {
            iced::keyboard::Event::KeyPressed { key, modifiers, .. } => {
                Message::KeyPressed(key, modifiers)
            }
            _ => Message::Nop,
        });
        let win = iced::event::listen().map(|ev| match ev {
            iced::Event::Window(iced::window::Event::Resized(size)) => Message::WindowResized(size),
            _ => Message::Nop,
        });
        Subscription::batch(vec![
            events,
            titles,
            keys,
            win,
            iced::time::every(std::time::Duration::from_millis(250)).map(|_| Message::Tick),
        ])
    }
}

fn snapshot_primary_is(app: &App, model: &str) -> bool {
    app.normalized_selection().primary.as_deref() == Some(model)
}

fn format_price(p: &crate::domain::Pricing) -> String {
    let fmt = |v: Option<rust_decimal::Decimal>| {
        v.map(|d| {
            let per_k = d / rust_decimal::Decimal::from(1000);
            format!("${per_k}/1k")
        })
        .unwrap_or_else(|| "?".into())
    };
    match (p.prompt_per_mtok, p.completion_per_mtok) {
        (None, None) => i18n::t("models.price_unknown").to_string(),
        (Some(input), Some(output)) if input.is_zero() && output.is_zero() => {
            i18n::t("models.price_free").to_string()
        }
        (inp, outp) => format!("{} → {}", fmt(inp), fmt(outp)),
    }
}

/// 按字符数截断并加省略号(chip 显示用)。
fn ellipsize(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let head: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

/// 附件大小标签(字节 → B/KB)。
fn size_label(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

/// 长粘贴阈值(字符):达到即转为附件。
const PASTE_ATTACH_THRESHOLD: usize = 2500;

fn paste_should_attach(text: &str) -> bool {
    text.chars().count() >= PASTE_ATTACH_THRESHOLD
}

/// 拉取目录。
async fn fetch_catalog(provider: Arc<OpenRouter>) -> Result<Vec<CatalogEntry>, String> {
    use crate::providers::Provider as _;
    provider
        .list_models()
        .await
        .map(|s| s.entries)
        .map_err(|e| e.to_string())
}

/// 查询 Key 状态。
async fn test_key(provider: Arc<OpenRouter>) -> Result<KeyStatus, String> {
    use crate::providers::Provider as _;
    provider.query_key_status().await.map_err(|e| e.to_string())
}

async fn pick_save_path(default_name: String) -> Option<std::path::PathBuf> {
    tokio::task::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_file_name(&default_name)
            .save_file()
    })
    .await
    .ok()
    .flatten()
}

/// 文本附件文件多选(spawn_blocking + rfd)。
async fn pick_attachments() -> PickedAttachments {
    let paths = tokio::task::spawn_blocking(move || {
        rfd::FileDialog::new()
            .add_filter(
                "Text/Code",
                &[
                    "txt", "md", "markdown", "json", "toml", "yaml", "yml", "csv", "log", "rs",
                    "py", "js", "ts", "tsx", "jsx", "go", "java", "c", "h", "cpp", "hpp", "cs",
                    "rb", "php", "sh", "bat", "ps1", "sql", "html", "css", "xml",
                ],
            )
            .add_filter("All files", &["*"])
            .pick_files()
    })
    .await
    .unwrap_or_default();
    let Some(paths) = paths else {
        return PickedAttachments {
            files: Vec::new(),
            errors: Vec::new(),
        };
    };
    let mut out = PickedAttachments {
        files: Vec::new(),
        errors: Vec::new(),
    };
    for p in paths {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| i18n::t("attachment.name_default").to_string());
        match std::fs::read_to_string(&p) {
            Ok(content) => out.files.push(Attachment { name, content }),
            Err(e) => out.errors.push(format!("{name}:{e}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(models: &[&str]) -> Vec<String> {
        models.iter().map(|m| m.to_string()).collect()
    }

    #[test]
    fn resolver_single_mode() {
        let enabled = set(&["m/a", "m/b"]);
        let sel = ModelSelectionState {
            parallel: false,
            picker_open: false,
            checked: vec![],
            primary: Some("m/b".into()),
        };
        let n = resolve_selection(&enabled, &[], &sel);
        assert_eq!(n.targets, vec!["m/b"]);
        assert_eq!(n.primary.as_deref(), Some("m/b"));
        assert!(n.invalid_reason.is_none());
        let sel_bad = ModelSelectionState {
            primary: Some("m/gone".into()),
            ..sel
        };
        let n = resolve_selection(&enabled, &[], &sel_bad);
        assert_eq!(n.primary.as_deref(), Some("m/a"));
    }

    #[test]
    fn resolver_parallel_primary_must_be_in_checked() {
        let enabled = set(&["m/a", "m/b", "m/c"]);
        let sel = ModelSelectionState {
            parallel: true,
            picker_open: true,
            checked: set(&["m/a", "m/c"]),
            primary: Some("m/b".into()),
        };
        let ranked = set(&["m/c", "m/a"]);
        let n = resolve_selection(&enabled, &ranked, &sel);
        assert_eq!(n.primary.as_deref(), Some("m/c"));
        assert_eq!(n.targets, vec!["m/c", "m/a"]);
    }

    #[test]
    fn resolver_parallel_empty_is_invalid() {
        let sel = ModelSelectionState {
            parallel: true,
            checked: set(&["m/gone"]),
            ..Default::default()
        };
        let n = resolve_selection(&set(&["m/a"]), &[], &sel);
        assert!(n.targets.is_empty());
        assert_eq!(n.invalid_reason, Some("composer.send_disabled_no_targets"));
        let n = resolve_selection(&[], &[], &ModelSelectionState::default());
        assert_eq!(n.invalid_reason, Some("composer.send_disabled_no_model"));
    }

    #[test]
    fn default_pair_prefers_most_used_other() {
        let enabled = set(&["m/a", "m/b", "m/c"]);
        let rank = set(&["m/c", "m/b", "m/a"]);
        let (checked, primary) = default_parallel_pair(&enabled, &rank, Some("m/a"));
        assert_eq!(checked, vec!["m/a", "m/c"]);
        assert_eq!(primary.as_deref(), Some("m/a"));
        let (checked, _) = default_parallel_pair(&enabled, &[], Some("m/c"));
        assert_eq!(checked, vec!["m/c", "m/a"]);
    }

    #[test]
    fn collapse_heuristic() {
        assert!(!should_collapse("短问题"));
        assert!(should_collapse(&"很".repeat(COLLAPSE_CHARS + 1)));
        assert!(should_collapse(&"行\n".repeat(COLLAPSE_LINES + 1)));
    }

    #[test]
    fn paste_threshold_classification() {
        assert!(!paste_should_attach(
            &"x".repeat(PASTE_ATTACH_THRESHOLD - 1)
        ));
        assert!(paste_should_attach(&"x".repeat(PASTE_ATTACH_THRESHOLD)));
        assert!(paste_should_attach(&"中".repeat(PASTE_ATTACH_THRESHOLD)));
        assert!(!paste_should_attach(""));
    }

    #[test]
    fn ellipsize_and_size_label() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("abcdef", 3).chars().count(), 3);
        assert!(ellipsize("abcdef", 3).ends_with('…'));
        assert_eq!(size_label(512), "512 B");
        assert_eq!(size_label(2048), "2.0 KB");
    }
}
