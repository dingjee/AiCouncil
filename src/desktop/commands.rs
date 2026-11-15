//! Tauri 命令实现(线约定见 docs/TAURI_IPC.md)。
//!
//! 所有命令 `rename_all = "snake_case"`:前端 invoke 的参数字典键
//! 与本文档参数名完全一致(snake_case)。

use std::sync::Mutex;

use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::composer::{ComposerMode, DraftState};
use crate::domain::plan::{ContinuationSelection, ModelTarget, Planner};
use crate::domain::recovery::{retry_decision, Remedy, RetryDecision};
use crate::domain::{attachment, Conversation, ConversationId};
use crate::providers::catalog::ModelPreference;
use crate::providers::Provider;
use crate::services::egress::{EgressGuard, EgressVerdict};
use crate::services::export::{self, ExportInput, ExportScope};
use crate::storage::repo::AppConfig;

use super::dto::*;
use super::{
    edit_latest_for, model_target_with, parse_conversation_id, parse_generation_id,
    parse_message_id, parse_provider_id, tree_of, Desktop,
};

type CmdResult<T> = Result<T, IpcError>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------- Bootstrap / settings ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn bootstrap(d: State<'_, Desktop>) -> CmdResult<BootstrapStateDto> {
    let cfg = lock(&d.config);
    let conversations =
        d.db.conversations()?
            .iter()
            .map(ConversationSummaryDto::from)
            .collect();
    let groups = d.db.groups()?.iter().map(GroupDto::from).collect();
    let credentials = credentials_status_internal();
    Ok(BootstrapStateDto {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        settings: settings_dto(&cfg),
        conversations,
        groups,
        pending_new_conversation: lock(&d.drafts).pending_new().map(|c| c.as_str()),
        catalog: catalog_state(&d),
        preferences: preference_entries(&d)?,
        credentials,
        egress: None,
    })
}

fn settings_dto(cfg: &AppConfig) -> SettingsDto {
    let g = &cfg.general;
    SettingsDto {
        max_concurrent_requests: g.max_concurrent_requests,
        assumed_output_tokens: g.assumed_output_tokens,
        connect_timeout_secs: g.connect_timeout_secs,
        read_idle_timeout_secs: g.read_idle_timeout_secs,
        body_text_size: g.body_text_size,
        locale: g.locale.clone(),
        theme_id: g.theme_id.clone(),
        sidebar_width: cfg.window.sidebar_width,
        sidebar_collapsed: cfg.window.sidebar_collapsed,
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_settings(d: State<'_, Desktop>) -> CmdResult<SettingsDto> {
    Ok(settings_dto(&lock(&d.config)))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn update_settings(
    d: State<'_, Desktop>,
    patch: SettingsPatchDto,
) -> CmdResult<SettingsDto> {
    let mut cfg = lock(&d.config);
    let g = &mut cfg.general;
    if let Some(v) = patch.max_concurrent_requests {
        g.max_concurrent_requests = v.max(1);
    }
    if let Some(v) = patch.assumed_output_tokens {
        g.assumed_output_tokens = v;
    }
    if let Some(v) = patch.connect_timeout_secs {
        g.connect_timeout_secs = v.max(1);
    }
    if let Some(v) = patch.read_idle_timeout_secs {
        g.read_idle_timeout_secs = v.max(1);
    }
    if let Some(v) = patch.body_text_size {
        g.body_text_size = if (12.0..=22.0).contains(&v) {
            Some(v)
        } else {
            None
        };
    }
    if let Some(v) = patch.locale {
        g.locale = v;
    }
    if let Some(v) = patch.theme_id {
        g.theme_id = v;
    }
    if let Some(v) = patch.sidebar_width {
        cfg.window.sidebar_width = v.clamp(240.0, 480.0);
    }
    if let Some(v) = patch.sidebar_collapsed {
        cfg.window.sidebar_collapsed = v;
    }
    cfg.save()?;
    Ok(settings_dto(&cfg))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn check_egress(d: State<'_, Desktop>) -> CmdResult<EgressStateDto> {
    use crate::providers::cancel::CancellationToken;
    let verdict = d.egress.check(CancellationToken::new()).await;
    let dto = match &verdict {
        EgressVerdict::Allowed(obs) => EgressStateDto {
            verdict: "allowed".into(),
            country_code: obs.country_code.clone(),
            detail: format!("egress check ok;{}", obs.evidence()),
        },
        EgressVerdict::Blocked(obs) => EgressStateDto {
            verdict: "blocked".into(),
            country_code: obs.country_code.clone(),
            detail: format!(
                "egress blocked in region;{};GeoIP/按域名分流可能与上游实际路由不一致",
                obs.evidence()
            ),
        },
        EgressVerdict::Unknown(why) => EgressStateDto {
            verdict: "unknown".into(),
            country_code: None,
            detail: why.clone(),
        },
    };
    if matches!(verdict, EgressVerdict::Allowed(_)) {
        let families = match &verdict {
            EgressVerdict::Allowed(obs) => obs.families,
            _ => crate::services::network::RouteFamilies::DUAL,
        };
        *lock(&d.permit) = Some(crate::services::egress::EgressPermit::fresh(
            d.egress.network_revision(),
            families,
        ));
    } else {
        *lock(&d.permit) = None;
    }
    Ok(dto)
}

// ---------- 会话与分组 ----------

#[tauri::command(rename_all = "snake_case")]
pub async fn list_conversations(d: State<'_, Desktop>) -> CmdResult<Vec<ConversationSummaryDto>> {
    Ok(d.db
        .conversations()?
        .iter()
        .map(ConversationSummaryDto::from)
        .collect())
}

fn conversation_detail(d: &Desktop, id: ConversationId) -> CmdResult<ConversationDetailDto> {
    let conv =
        d.db.conversation_of(id)?
            .ok_or_else(|| IpcError::not_found("conversation"))?;
    let messages = d.db.messages_of(id)?;
    let tree = tree_of(&d.db, id);
    let generations = d.db.generations_of(id)?;
    let usage = d.db.usage_of(id)?;
    let edit_latest = edit_latest_for(&conv, &tree, &generations);
    // 草稿镜像与权威行同步(get_draft 等命令依赖)。
    lock(&d.drafts).sync_from_row(&conv);
    Ok(ConversationDetailDto {
        conversation: ConversationSummaryDto::from(&conv),
        messages: messages.iter().map(MessageDto::from).collect(),
        generations: generations.iter().map(GenerationDto::from).collect(),
        usage: usage.iter().map(UsageRecordDto::from).collect(),
        edit_latest,
    })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn load_conversation(
    d: State<'_, Desktop>,
    conversation_id: String,
) -> CmdResult<ConversationDetailDto> {
    conversation_detail(&d, parse_conversation_id(&conversation_id)?)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_conversation(d: State<'_, Desktop>) -> CmdResult<ConversationSummaryDto> {
    let mut drafts = lock(&d.drafts);
    // pending-new 槽位复用:未发送的槽位直接返回(连点 New 不制造空壳)。
    if let Some(pending) = drafts.pending_new() {
        match d.db.conversation_of(pending) {
            Ok(Some(conv)) => {
                if d.db
                    .messages_of(pending)
                    .map(|m| m.is_empty())
                    .unwrap_or(false)
                {
                    return Ok(ConversationSummaryDto::from(&conv));
                }
                let _ = drafts.clear_pending_new(Some(pending));
            }
            _ => {
                let _ = drafts.clear_pending_new(Some(pending));
            }
        }
    }
    let conv = Conversation::new(ConversationId::new(), crate::storage::now_dt());
    d.db.upsert_conversation(&conv)?;
    drafts.set_pending_new(conv.id)?;
    drafts.sync_from_row(&conv);
    let dto = ConversationSummaryDto::from(&conv);
    drop(drafts);
    d.emit_conversations_changed();
    Ok(dto)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rename_conversation(
    d: State<'_, Desktop>,
    conversation_id: String,
    title: String,
    expected_title_revision: i64,
) -> CmdResult<ConversationSummaryDto> {
    let id = parse_conversation_id(&conversation_id)?;
    let updated =
        d.db.rename_conversation_title(id, &title, expected_title_revision)?
            .ok_or_else(|| {
                IpcError::with_detail(
                    "invalid_input",
                    "title revision conflict",
                    "title_revision_conflict",
                )
            })?;
    d.emit_conversations_changed();
    Ok(ConversationSummaryDto::from(&updated))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_conversation(d: State<'_, Desktop>, conversation_id: String) -> CmdResult<()> {
    let id = parse_conversation_id(&conversation_id)?;
    d.db.delete_conversation(id)?;
    let mut drafts = lock(&d.drafts);
    drafts.forget(id);
    let _ = drafts.clear_pending_new(Some(id));
    drop(drafts);
    d.emit_conversations_changed();
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_conversation_group(
    d: State<'_, Desktop>,
    conversation_id: String,
    group_id: Option<String>,
) -> CmdResult<()> {
    let conv = parse_conversation_id(&conversation_id)?;
    let group = group_id
        .as_deref()
        .map(|s| {
            uuid::Uuid::parse_str(s)
                .map(|_| crate::domain::GroupId::from_uuid_str(s))
                .map_err(|e| IpcError::invalid_input(format!("bad group id: {e}")))
        })
        .transpose()?;
    d.db.set_conversation_group(conv, group)?;
    d.emit_conversations_changed();
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_system_prompt(
    d: State<'_, Desktop>,
    conversation_id: String,
    system_prompt: String,
) -> CmdResult<()> {
    let conv = parse_conversation_id(&conversation_id)?;
    d.db.set_conversation_system_prompt(conv, &system_prompt)?;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_main_leaf(
    d: State<'_, Desktop>,
    conversation_id: String,
    message_id: String,
) -> CmdResult<()> {
    let conv = parse_conversation_id(&conversation_id)?;
    let leaf = parse_message_id(&message_id)?;
    let messages = d.db.messages_of(conv)?;
    if !messages.iter().any(|m| m.id == leaf) {
        return Err(IpcError::not_found("message"));
    }
    d.db.set_conversation_main_leaf(conv, Some(leaf))?;
    d.emit_conversation_changed(conv);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn create_group(d: State<'_, Desktop>, name: String) -> CmdResult<GroupDto> {
    let existing = d.db.groups()?;
    let sort_order = existing.iter().map(|g| g.sort_order).max().unwrap_or(0) + 1;
    let group = crate::domain::Group {
        id: crate::domain::GroupId::new(),
        name,
        sort_order,
    };
    d.db.upsert_group(&group)?;
    d.emit_conversations_changed();
    Ok(GroupDto::from(&group))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn rename_group(d: State<'_, Desktop>, group_id: String, name: String) -> CmdResult<()> {
    let groups = d.db.groups()?;
    let Some(mut group) = groups.iter().find(|g| g.id.as_str() == group_id).cloned() else {
        return Err(IpcError::not_found("group"));
    };
    group.name = name;
    d.db.upsert_group(&group)?;
    d.emit_conversations_changed();
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_group(d: State<'_, Desktop>, group_id: String) -> CmdResult<()> {
    let id = crate::domain::GroupId::from_uuid_str(&group_id);
    d.db.delete_group(id)?;
    d.emit_conversations_changed();
    Ok(())
}

// ---------- 草稿 ----------

fn draft_state_to_dto(s: &DraftState) -> DraftDto {
    DraftDto {
        text: s.text.clone(),
        attachments: s.attachments.iter().map(AttachmentDto::from).collect(),
        edit_text: s.edit_text.clone(),
        edit_attachments: s.edit_attachments.iter().map(AttachmentDto::from).collect(),
        edit_target_id: s.edit_target.map(|t| t.as_str()),
        revision: s.revision,
    }
}

fn validate_attachments(
    items: Vec<AttachmentDto>,
) -> Result<Vec<attachment::Attachment>, IpcError> {
    let mut out: Vec<attachment::Attachment> = Vec::new();
    for item in items {
        let att: attachment::Attachment = item.into();
        attachment::try_push(&mut out, att).map_err(IpcError::invalid_input)?;
    }
    Ok(out)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_draft(d: State<'_, Desktop>, conversation_id: String) -> CmdResult<DraftDto> {
    let conv = parse_conversation_id(&conversation_id)?;
    let mut drafts = lock(&d.drafts);
    if drafts.state(conv).is_none() {
        if let Ok(Some(row)) = d.db.conversation_of(conv) {
            drafts.sync_from_row(&row);
        }
    }
    let state = drafts.state(conv).cloned().unwrap_or_default();
    Ok(draft_state_to_dto(&state))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_draft(
    d: State<'_, Desktop>,
    conversation_id: String,
    text: String,
    attachments: Vec<AttachmentDto>,
    changed: bool,
) -> CmdResult<RevisionDto> {
    let conv = parse_conversation_id(&conversation_id)?;
    let atts = validate_attachments(attachments)?;
    let revision = lock(&d.drafts).save_normal(conv, &text, &atts, changed)?;
    Ok(RevisionDto { revision })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn save_edit_buffer(
    d: State<'_, Desktop>,
    conversation_id: String,
    text: String,
    attachments: Vec<AttachmentDto>,
    target_id: Option<String>,
    changed: bool,
) -> CmdResult<RevisionDto> {
    let conv = parse_conversation_id(&conversation_id)?;
    let atts = validate_attachments(attachments)?;
    let target = target_id.as_deref().map(parse_message_id).transpose()?;
    let revision = lock(&d.drafts).save_edit(conv, &text, &atts, target, changed)?;
    Ok(RevisionDto { revision })
}

#[tauri::command(rename_all = "snake_case")]
pub async fn clear_edit_buffer(
    d: State<'_, Desktop>,
    conversation_id: String,
) -> CmdResult<RevisionDto> {
    let conv = parse_conversation_id(&conversation_id)?;
    let revision = lock(&d.drafts).clear_edit(conv)?;
    Ok(RevisionDto { revision })
}

// ---------- 目录与偏好 ----------

pub fn catalog_state(d: &Desktop) -> CatalogStateDto {
    let entries = d
        .catalog
        .entries()
        .iter()
        .map(|e| CatalogEntryDto::from_entry(&super::provider_id(), e))
        .collect();
    CatalogStateDto {
        updated_at_ms: d.catalog.last_success().map(|t| t.timestamp_millis()),
        entries,
    }
}

fn preference_entries(d: &Desktop) -> CmdResult<Vec<ModelPreferenceEntryDto>> {
    let mut out = Vec::new();
    for (model, pref) in d.db.preferences(super::provider_id())? {
        out.push(ModelPreferenceEntryDto {
            provider_id: super::PROVIDER_ID.to_string(),
            model_id: model,
            preference: ModelPreferenceDto {
                enabled: pref.enabled,
                sort_order: pref.sort_order,
                is_default: pref.is_default,
                overrides: pref.overrides,
            },
        });
    }
    Ok(out)
}

/// refresh_catalog 命令与 reload 后台刷新的共用实现。
pub(crate) async fn refresh_catalog_inner(d: &Desktop) -> CmdResult<CatalogStateDto> {
    let Some(key) = super::env_keys::load_named_key(super::env_keys::OPENROUTER_ENV_NAME) else {
        return Err(IpcError::new(
            "credentials_missing",
            "OPENROUTER_API_KEY not configured",
        ));
    };
    let provider = super::Desktop::build_provider(&key);
    d.catalog
        .refresh(
            || async move { provider.list_models().await },
            true,
            crate::storage::now_dt(),
        )
        .await;
    if let Some(snap) = d.catalog.snapshot() {
        let _ =
            d.db.replace_catalog(super::provider_id(), &snap.entries, snap.fetched_at);
    }
    d.emit_catalog_changed();
    Ok(catalog_state(d))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn refresh_catalog(d: State<'_, Desktop>) -> CmdResult<CatalogStateDto> {
    refresh_catalog_inner(&d).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_preferences(d: State<'_, Desktop>) -> CmdResult<Vec<ModelPreferenceEntryDto>> {
    preference_entries(&d)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_model_preference(
    d: State<'_, Desktop>,
    provider_id: String,
    model_id: String,
    preference: ModelPreferenceDto,
) -> CmdResult<()> {
    let pid = parse_provider_id(&provider_id)?;
    let pref = ModelPreference {
        enabled: preference.enabled,
        sort_order: preference.sort_order,
        is_default: preference.is_default,
        overrides: preference.overrides,
    };
    d.db.upsert_preference(pid, &model_id, &pref)?;
    lock(&d.prefs).set(pid, &model_id, pref);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_primary_model(
    d: State<'_, Desktop>,
    provider_id: String,
    model_id: String,
) -> CmdResult<()> {
    let pid = parse_provider_id(&provider_id)?;
    let current = lock(&d.prefs).get(pid, &model_id);
    let pref = ModelPreference {
        is_default: true,
        ..current
    };
    d.db.upsert_preference(pid, &model_id, &pref)?;
    lock(&d.prefs).set(pid, &model_id, pref);
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn check_key_status() -> CmdResult<KeyStatusDto> {
    let Some(key) = super::env_keys::load_named_key(super::env_keys::OPENROUTER_ENV_NAME) else {
        return Err(IpcError::new(
            "credentials_missing",
            "OPENROUTER_API_KEY not configured",
        ));
    };
    let provider = super::Desktop::build_provider(&key);
    match provider.query_key_status().await {
        Ok(crate::providers::KeyStatus::Ok {
            label,
            usage,
            limit,
            limit_remaining,
        }) => Ok(KeyStatusDto {
            status: "ok".into(),
            label,
            usage: usage.map(|d| d.to_string()),
            limit: limit.map(|d| d.to_string()),
            limit_remaining: limit_remaining.map(|d| d.to_string()),
            error: None,
        }),
        Ok(crate::providers::KeyStatus::Unsupported) => Ok(KeyStatusDto {
            status: "unsupported".into(),
            label: None,
            usage: None,
            limit: None,
            limit_remaining: None,
            error: None,
        }),
        Ok(crate::providers::KeyStatus::PermissionDenied) => Ok(KeyStatusDto {
            status: "permission_denied".into(),
            label: None,
            usage: None,
            limit: None,
            limit_remaining: None,
            error: None,
        }),
        Err(e) => Ok(KeyStatusDto {
            status: "error".into(),
            label: None,
            usage: None,
            limit: None,
            limit_remaining: None,
            error: Some(e.to_string()),
        }),
    }
}

// ---------- 发送 / 取消 / 重试 / 移除 ----------

fn build_selection(
    d: &Desktop,
    req: &SendMessageRequestDto,
) -> Result<ContinuationSelection, IpcError> {
    let target = |t: &ModelTargetRefDto| -> Result<ModelTarget, IpcError> {
        let t_pid = parse_provider_id(&t.provider_id)?;
        Ok(model_target_with(
            &d.catalog,
            &lock(&d.prefs),
            t_pid,
            &t.model_id,
        ))
    };
    match &req.selection {
        SelectionDto::Single { leaf, targets, .. } => {
            let t0 = targets
                .first()
                .ok_or_else(|| IpcError::invalid_input("empty targets"))?;
            Ok(ContinuationSelection::Single {
                leaf: leaf.as_deref().map(parse_message_id).transpose()?,
                provider_id: parse_provider_id(&t0.provider_id)?,
                model_id: t0.model_id.clone(),
            })
        }
        SelectionDto::SameQuestionMulti {
            leaf,
            targets,
            primary,
        } => {
            let ts: Result<Vec<ModelTarget>, IpcError> = targets.iter().map(target).collect();
            Ok(ContinuationSelection::SameQuestionMulti {
                leaf: leaf.as_deref().map(parse_message_id).transpose()?,
                targets: ts?,
                primary: *primary,
            })
        }
        SelectionDto::MultiBranch { branches, primary } => {
            let bs: Result<Vec<(crate::domain::MessageId, ModelTarget)>, IpcError> = branches
                .iter()
                .map(|b| Ok((parse_message_id(&b.leaf)?, target(&b.target)?)))
                .collect();
            Ok(ContinuationSelection::MultiBranch {
                branches: bs?,
                primary: *primary,
            })
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn send_message(
    d: State<'_, Desktop>,
    request: SendMessageRequestDto,
) -> CmdResult<SendReceiptDto> {
    let conv_id = parse_conversation_id(&request.conversation_id)?;
    let mode = match request.mode.as_str() {
        "normal" => ComposerMode::Normal,
        "edit" => ComposerMode::Edit(crate::domain::MessageId::new()), // 目标稍后从编辑缓冲取
        other => return Err(IpcError::invalid_input(format!("bad mode `{other}`"))),
    };
    // selection 只借用 request;attachments 移动在后(避免部分移动冲突)。
    let selection = build_selection(&d, &request)?;
    let atts = validate_attachments(request.attachments)?;
    let conv =
        d.db.conversation_of(conv_id)?
            .ok_or_else(|| IpcError::not_found("conversation"))?;
    let tree = tree_of(&d.db, conv_id);
    let system_prompt = if conv.system_prompt.trim().is_empty() {
        None
    } else {
        Some(conv.system_prompt.clone())
    };

    // “编辑最新问题”:目标必须等于当前资格化的 edit_latest。
    let (mode, replacement) = if request.mode == "edit" {
        let generations = d.db.generations_of(conv_id)?;
        let latest = edit_latest_for(&conv, &tree, &generations).ok_or_else(|| {
            IpcError::with_detail(
                "admission.invalid_branch",
                "edit target not eligible",
                "edit_latest_unavailable",
            )
        })?;
        let target_id = parse_message_id(&latest.target_id)?;
        let current_leaf = d.db.main_leaf_of(conv_id)?;
        (
            ComposerMode::Edit(target_id),
            Some((target_id, current_leaf)),
        )
    } else {
        (mode, None)
    };

    let plan = Planner::prepare(
        conv_id,
        &selection,
        &request.prompt,
        &atts,
        system_prompt,
        &tree,
        crate::storage::now_dt(),
    )
    .map_err(|e| IpcError::invalid_input(e.to_string()))?;

    // 草稿消费按**前端冻结的版本**做 CAS:快照之后用户又输入了新内容
    // (草稿版本前进)时,事务内消费不生效 → 新输入被保留。
    let consumption =
        lock(&d.drafts).consumption_at(conv_id, mode, request.expected_draft_revision);
    // permit:同 revision 且 TTL 内的近期许可(服务侧仍完整校验)。
    let permit = lock(&d.permit).filter(|p| p.still_valid(d.egress.network_revision()));

    let chat = lock(&d.chat).clone();
    let receipt = chat
        .submit_with_permit_replacing(plan, permit, replacement, consumption)
        .await?;

    // 成功落库:fallback 标题 + 异步标题申请(重试/编辑替换不触发新标题)。
    let title = lock(&d.title).clone();
    let turn_user = receipt
        .assistant_of
        .get(&receipt.primary_request_id)
        .copied();
    let _ = turn_user;
    // turn_id = 本批主 user 消息 id(标题台账按 turn 去重)。
    let plan_users = d.db.generations_of(conv_id).ok();
    let turn_id = plan_users
        .as_ref()
        .and_then(|gs| {
            gs.iter()
                .find(|g| {
                    g.batch_id == receipt.batch_id && g.request_id == receipt.primary_request_id
                })
                .map(|g| g.user_message_id.as_str())
        })
        .unwrap_or_default();
    title.on_user_turn_persisted(conv_id, turn_id, &request.prompt, &atts);

    // 草稿镜像与权威行对账(消费后 revision/缓冲已变)。
    if let Ok(Some(row)) = d.db.conversation_of(conv_id) {
        lock(&d.drafts).sync_from_row(&row);
    }
    d.emit_conversation_changed(conv_id);
    d.emit_conversations_changed();
    Ok(SendReceiptDto::from(receipt))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_request(d: State<'_, Desktop>, request_id: String) -> CmdResult<()> {
    let id = parse_generation_id(&request_id)?;
    let chat = lock(&d.chat).clone();
    chat.cancel(id).await;
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn retry_request(d: State<'_, Desktop>, request_id: String) -> CmdResult<SendReceiptDto> {
    let id = parse_generation_id(&request_id)?;
    let original =
        d.db.generation(id)?
            .ok_or_else(|| IpcError::not_found("generation"))?;
    let conv_id =
        d.db.conversation_of_message(original.user_message_id)?
            .ok_or_else(|| IpcError::not_found("conversation"))?;
    let tree = tree_of(&d.db, conv_id);
    let user_msg = tree
        .get(original.user_message_id)
        .ok_or_else(|| IpcError::not_found("user message"))?;

    // UI 资格先行(服务层以最新持久数据兜底)。
    let root = original.retry_root_id.unwrap_or(original.request_id);
    let generations = d.db.generations_of(conv_id)?;
    let lineage_active = generations
        .iter()
        .any(|g| g.retry_root_id == Some(root) && !g.status.is_terminal());
    let latest_failure = generations
        .iter()
        .filter(|g| g.retry_root_id == Some(root) && g.failure.is_some())
        .max_by_key(|g| g.finished_at.unwrap_or_else(crate::storage::now_dt))
        .and_then(|g| g.failure.clone());
    match retry_decision(
        original.status,
        latest_failure.as_ref(),
        crate::domain::recovery::now_ms(),
        lineage_active,
    ) {
        RetryDecision::Allowed => {}
        RetryDecision::InFlight => {
            return Err(IpcError::with_detail(
                "admission.duplicate",
                "An attempt is already in flight",
                "retry_in_flight",
            ));
        }
        RetryDecision::Cooldown(until) => {
            return Err(IpcError::with_detail(
                "admission.cooldown",
                "Retry cooldown active",
                until.to_string(),
            ));
        }
        RetryDecision::NeedsAction(remedy) => {
            let remedy = match remedy {
                Remedy::SwitchIp => "switch_ip",
                Remedy::Credentials => "credentials",
                Remedy::Balance => "balance",
                Remedy::EditInput => "edit_input",
                Remedy::PickModel => "pick_model",
                Remedy::None => "none",
            };
            return Err(IpcError::with_detail(
                "admission.needs_action",
                "Action required before retry",
                remedy,
            ));
        }
    }

    let prompt = attachment::compose_prompt(&user_msg.content, &user_msg.attachments);
    let history = Planner::history_before(&tree, original.user_message_id);
    let chat = lock(&d.chat).clone();
    let receipt = chat.retry(conv_id, &original, &prompt, history).await?;
    d.emit_conversation_changed(conv_id);
    Ok(SendReceiptDto::from(receipt))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn remove_message(d: State<'_, Desktop>, message_id: String) -> CmdResult<()> {
    let id = parse_message_id(&message_id)?;
    let conv =
        d.db.conversation_of_message(id)?
            .ok_or_else(|| IpcError::not_found("message"))?;
    d.db.remove_message(id)?;
    d.emit_conversation_changed(conv);
    Ok(())
}

// ---------- 导出 ----------

/// 导出落盘结果(原生保存对话框 + 原子写)。
#[derive(Debug, Clone, Serialize)]
pub struct ExportSavedDto {
    pub path: String,
    pub sections: usize,
}

/// 自有所有权的导出输入(便于跨 await 持有)。
struct ExportInputOwned {
    title: String,
    tree: crate::domain::MessageTree,
    generations: Vec<crate::domain::Generation>,
    main_leaf: Option<crate::domain::MessageId>,
}

impl ExportInputOwned {
    fn as_input(&self) -> ExportInput<'_> {
        ExportInput {
            title: &self.title,
            tree: &self.tree,
            generations: &self.generations,
            main_leaf: self.main_leaf,
        }
    }
}

/// 组装导出输入(主分支 / 选中叶子 / 全分支)。
fn export_input(d: &Desktop, conv_id: ConversationId) -> CmdResult<ExportInputOwned> {
    let conv =
        d.db.conversation_of(conv_id)?
            .ok_or_else(|| IpcError::not_found("conversation"))?;
    Ok(ExportInputOwned {
        title: conv.title.clone(),
        tree: tree_of(&d.db, conv_id),
        generations: d.db.generations_of(conv_id)?,
        main_leaf: conv.main_leaf_id,
    })
}

fn parse_scope(scope: &str, message_ids: Option<Vec<String>>) -> CmdResult<ExportScope> {
    match scope {
        "main" => Ok(ExportScope::Main),
        "all_branches" => Ok(ExportScope::AllBranches),
        "selected" => {
            let ids = message_ids
                .ok_or_else(|| IpcError::invalid_input("selected scope requires message_ids"))?;
            let mut leaves = Vec::new();
            for s in ids {
                leaves.push(parse_message_id(&s)?);
            }
            Ok(ExportScope::Selected(leaves))
        }
        other => Err(IpcError::invalid_input(format!("bad scope `{other}`"))),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn export_markdown(
    d: State<'_, Desktop>,
    conversation_id: String,
    scope: String,
    message_ids: Option<Vec<String>>,
) -> CmdResult<ExportResultDto> {
    let conv_id = parse_conversation_id(&conversation_id)?;
    let owned = export_input(&d, conv_id)?;
    let scope = parse_scope(&scope, message_ids)?;
    let content = export::render_markdown(&owned.as_input(), scope.clone())
        .map_err(|e| IpcError::with_detail("storage", "export failed", e.to_string()))?;
    let filename = export::default_filename(&owned.title, scope);
    Ok(ExportResultDto { filename, content })
}

/// 原生保存对话框(NSSavePanel)+ 原子落盘。
/// 前端应等待本命令成功返回后再提示“已导出”;用户取消 → code `cancelled`。
#[tauri::command(rename_all = "snake_case")]
pub async fn export_markdown_to_file(
    d: State<'_, Desktop>,
    app: tauri::AppHandle,
    conversation_id: String,
    scope: String,
    message_ids: Option<Vec<String>>,
) -> CmdResult<ExportSavedDto> {
    let conv_id = parse_conversation_id(&conversation_id)?;
    let owned = export_input(&d, conv_id)?;
    let scope = parse_scope(&scope, message_ids)?;
    let filename = export::default_filename(&owned.title, scope.clone());

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(&filename)
        .add_filter("Markdown", &["md"])
        .save_file(move |picked| {
            let _ = tx.send(picked);
        });
    let picked = rx
        .await
        .map_err(|_| IpcError::new("invalid_input", "save dialog closed unexpectedly"))?;
    let Some(picked) = picked else {
        return Err(IpcError::new("cancelled", "save cancelled"));
    };
    let path = picked
        .into_path()
        .map_err(|e| IpcError::with_detail("storage", "invalid save path", e.to_string()))?;

    let result = export::export(&owned.as_input(), scope, &path)
        .map_err(|e| IpcError::with_detail("storage", "export failed", e.to_string()))?;
    Ok(ExportSavedDto {
        path: result.path.to_string_lossy().to_string(),
        sections: result.sections,
    })
}

// ---------- 凭据(`.env` 唯一权威源;只暴露布尔状态) ----------

fn credentials_status_internal() -> CredentialsStatusDto {
    CredentialsStatusDto {
        openrouter: super::env_keys::has_named_key(super::env_keys::OPENROUTER_ENV_NAME),
        deepseek: super::env_keys::has_named_key(super::env_keys::DEEPSEEK_ENV_NAME),
    }
}

#[tauri::command(rename_all = "snake_case")]
pub async fn credential_status() -> CmdResult<CredentialsStatusDto> {
    Ok(credentials_status_internal())
}

#[tauri::command(rename_all = "snake_case")]
pub async fn reload_credentials(d: State<'_, Desktop>) -> CmdResult<CredentialsStatusDto> {
    d.reload_providers()
        .map_err(|e| IpcError::with_detail("storage", "reload failed", e))?;
    let status = credentials_status_internal();
    // 新 key 到位后立即尝试目录刷新(不阻塞)。
    if status.openrouter {
        let app = d.app.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(state) = app.try_state::<Desktop>() {
                let _ = refresh_catalog_inner(&state).await;
            }
        });
    }
    Ok(status)
}
