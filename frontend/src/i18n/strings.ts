/**
 * UI strings — English, ported from `src/app/i18n.rs`.
 *
 * The native app keeps every user-visible string in one table with stable
 * keys; the frontend mirrors that table so both surfaces stay in sync.
 */

const TABLE: Record<string, string> = {
  "app.title": "AiCouncil",

  "conversation.new": "New chat",
  "conversation.untitled": "New chat",
  "conversation.new_button": "New chat",
  "conversation.search_placeholder": "Search chats",
  "conversation.delete": "Delete chat",
  "conversation.move_to": "Move current chat to:",
  "conversation.group_ungrouped": "Ungrouped",
  "conversation.fallback_date": "Chat {date}",
  "conversation.rename": "Rename chat",

  "title.edit_tooltip": "Rename chat",
  "title.save": "Save",
  "title.placeholder": "Chat title",
  "title.empty_rejected": "Title cannot be empty",
  "title.save_failed": "Failed to save title: {error}",
  "title.too_long": "Title is limited to 80 characters",
  "title.auto_failed_hint": "Auto title unavailable (will retry on next question)",

  "message.role.user": "You",
  "message.role.assistant": "Assistant",
  "message.copy": "Copy source",
  "message.copy_tooltip": "Copy full Markdown source of this message",
  "message.copy_question_tooltip": "Copy the question",
  "message.retry": "Retry",
  "message.continue": "Continue from here",
  "message.continue_tooltip": "Make this answer the main branch and continue",
  "message.stop": "Stop",
  "message.select_branch": "Branch",
  "message.select_branch_tooltip": "Ask follow-ups along multiple answers",
  "message.generating": "Generating…",
  "message.queued": "Queued",
  "message.connecting": "Connecting",
  "message.streaming": "Streaming…",
  "message.completed": "Completed",
  "message.failed": "Failed",
  "message.cancelled": "Cancelled",
  "message.interrupted": "Interrupted",
  "message.expand": "Expand question",
  "message.collapse": "Collapse question",
  "message.delete": "Delete message",
  "message.delete_tooltip": "Remove this message from the chat",
  "message.edit_question": "Edit question",
  "message.edit_question_tooltip": "Edit the latest question and send it again",

  "answers.split": "Split",
  "answers.split_tooltip": "Compare two answers side by side",
  "answers.attempt": "Attempt {n}",
  "answers.superseded": "Superseded",
  "answers.live": "Generating",
  "answers.failed": "Failed",

  "selection.copy": "Copy",
  "selection.copy_tooltip": "Copy selected text (plain text; formulas as source)",
  "selection.cleared_hint": "Selection cleared",

  "composer.placeholder": "Ask anything…",
  "composer.send": "Send",
  "composer.send_tooltip": "Send message (Enter)",
  "composer.stop_tooltip": "Stop generating",
  "composer.send_disabled_no_model": "Enable a model first",
  "composer.send_disabled_no_targets": "Select at least one model",
  "composer.attach": "Attach",
  "composer.attach_tooltip": "Attach text files; long pastes become attachments",
  "composer.usage": "Usage",
  "composer.usage_tooltip": "Show cost and key quota for this chat",
  "composer.models_tooltip": "Models: {models}",
  "composer.models_single_tooltip": "Only one model is enabled",
  "composer.model_picker": "Model",
  "composer.primary_badge": "Primary",
  "composer.primary_tooltip": "Primary model (owns the main branch)",
  "composer.branch_mode_hint":
    "Branch follow-up mode: each selected answer continues with its own model",
  "composer.cancel_edit": "Cancel edit",
  "composer.cancel_edit_tooltip": "Leave question editing and keep your normal draft",
  "composer.restore_edit": "Restore edit draft",
  "composer.restore_edit_tooltip":
    "The edited question was removed; bring its preserved draft back",
  "composer.editing": "Editing question",
  "composer.paste_converted": "Long paste attached ({chars} chars)",
  "composer.attachment_limit": "Attachment limit reached",
  "composer.attachment_too_large": "{name} is larger than 256KB",
  "composer.attachment_total": "Attachments are limited to 1MB in total",

  "selector.open": "Models",
  "selector.multi_hint": "Check the models to ask; the star sets the primary",
  "selector.select_all": "Select all",
  "selector.selected_count": "{n} selected",
  "selector.count.one": "{n} model",
  "selector.count.other": "{n} models",
  "selector.primary_plus": "{primary} +{n}",
  "selector.search_placeholder": "Search family, name or ID",
  "selector.no_results": "No models match \"{query}\"",
  "selector.empty": "No models enabled yet",
  "selector.empty_hint":
    "Enable a model in the Models panel to use it in this chat.",
  "selector.manage_models": "Manage models",
  "selector.my_models": "My models",

  "attachment.added": "Added {n} attachment(s)",
  "attachment.paste_attached": "Long paste attached ({chars} chars)",
  "attachment.name_default": "untitled",
  "attachment.paste_name": "Pasted text_{time}.txt",
  "attachment.remove": "Remove attachment",

  "panel.history": "Chats",
  "panel.models": "Models",
  "panel.settings": "Settings",
  "panel.sidebar_collapse_tooltip": "Toggle sidebar",
  "panel.export": "Export",
  "panel.export_main": "Main branch",
  "panel.export_all": "All branches",
  "panel.export_selected": "Selected branches ({n})",
  "panel.export_tooltip": "Export chat as Markdown",
  "toolbar.cost": "Cost",
  "toolbar.cost_tooltip": "Cost of this chat (all branches and retries)",
  "toolbar.cost_details": "Cost details",
  "toolbar.no_conversation": "—",
  "toolbar.cost_partial_hint":
    "Some costs of this chat are not settled yet (+ marks them).",
  "panel.back_bottom": "Back to bottom",
  "panel.back_bottom_tooltip": "Jump to the latest messages",

  "modal.close": "Close",
  "modal.close_tooltip": "Close this dialog",
  "modal.models_title": "Models",
  "modal.settings_title": "Settings",

  "dialog.db.title": "The chat database is not available",

  "egress.title": "Network region check",
  "egress.cancel": "Cancel",
  "egress.blocked":
    "Anthropic models cannot be used from this network region ({country}).",
  "egress.unknown":
    "The network region could not be verified, so Anthropic models are blocked.",
  "egress.detail": "Details: {detail}",
  "egress.recheck": "Check again",
  "egress.send_without": "Send without Anthropic",

  "group.new": "New group",
  "group.new_placeholder": "Group name",
  "group.create": "Create",
  "group.rename": "Rename",
  "group.delete": "Delete",
  "group.save": "Save",
  "group.delete_confirm": "Delete group \"{name}\"? Its chats become ungrouped.",

  "models.test_connection": "Test connection",
  "models.refreshing": "Refreshing…",
  "models.testing": "Testing…",
  "models.connect_failed": "Connection failed ({error})",
  "models.connected_no_support": "This provider does not report a quota",
  "models.connected_no_quota": "No quota information available",
  "models.refresh_catalog": "Refresh catalog",
  "models.search_placeholder": "Search models (name/ID)",
  "models.mine": "My models",
  "models.enabled": "Enabled",
  "models.default": "Default",
  "models.set_default": "Set default",
  "models.current": "Current",
  "models.catalog": "Catalog ({n})",
  "models.none_enabled": "No models enabled; check some in the catalog below",
  "models.not_in_catalog": "Not in catalog",
  "models.price": "Price",
  "models.id_tooltip": "ID: {id}",
  "models.price_unknown": "Price unknown",
  "models.price_free": "Free",
  "models.overrides": "Parameters",
  "models.overrides_hint":
    "JSON object sent as model parameters; empty means Auto (server defaults).",
  "models.overrides_invalid": "Parameters must be a JSON object",
  "models.move_up_tooltip": "Move up",
  "models.move_down_tooltip": "Move down",

  "settings.title": "Settings",
  "settings.provider_keys": "Provider keys",
  "settings.credentials_configured": "Configured",
  "settings.credentials_not_configured": "Not configured",
  "settings.credentials_reload": "Reload",
  "settings.credentials_reload_tooltip":
    "Re-read the provider keys from the local configuration file",
  "settings.credentials_reloading": "Reloading…",
  "settings.credentials_source": "Configuration source: {source}",
  "settings.credentials_env_hint":
    "Provider keys are read from the local .env file (the only source): ~/Library/Application Support/AiCouncil/.env. On the first start of the bundled app, a data/.env next to the application is imported into that folder (an existing file is never overwritten). AICOUNCIL_ENV_PATH selects a different file. Use \"Reload credentials\" to re-read it. The app never stores keys in the browser.",
  "settings.credentials_reload_credentials": "Reload credentials",
  "settings.credentials_missing": "Missing: {providers}",
  "settings.data_dir": "Data directory",
  "settings.about": "About",
  "settings.data_dir_unavailable": "(unavailable: {error})",
  "settings.concurrency": "Concurrency limit: {n} (default)",
  "settings.assumed_output": "Cost estimate assumes {n} output tokens",
  "settings.export_note": "Export: main branch / all branches (Markdown)",
  "settings.font_size": "Body text size",
  "settings.font_size_value": "{n} px",
  "settings.theme": "Theme: Soft Sage (built-in)",
  "settings.title_requests": "Title helper requests: {known} known · {unknown} unknown cost",

  "usage.known_pending":
    "Known cost ${known} · pending {pending} · unavailable {unavailable}",
  "usage.title_request_mark": " (title helper)",
  "usage.unknown": "Cost unknown",
  "usage.tokens": "{prompt} prompt · {completion} completion tokens",

  "empty.title": "Start a new chat",
  "empty.subtitle": "Type a question and pick an enabled model to begin.",
  "empty.configure": "Configure models",

  "export.none": "Nothing to export in this chat",
  "export.done": "Exported {sections} section(s) → {path}",
  "export.failed": "Export failed: {error}",

  "toast.no_model": "Enable a model in the Models panel first",
  "toast.model_unknown": "This model is not in the catalog",
  "toast.model_update_failed": "Could not update model: {error}",
  "toast.branch_incomplete":
    "The current answer is not finished; retry or wait before continuing",
  "toast.branch_active":
    "The current answer is still generating; stop it or wait before sending",
  "toast.parallel_empty": "Select at least one model",
  "toast.continue_not_completed": "Unfinished answers cannot be used to continue",
  "toast.send_failed": "Send failed (draft kept): {error}",
  "toast.retry_rejected": "Retry not available: {reason}",
  "toast.retry_in_flight": "This request is already being retried",
  "toast.storage_error": "Storage error (state may not be saved): {error}",
  "toast.draft_save_failed": "Draft could not be saved: {error}",
  "toast.draft_kept_after_send": "Your newer input was kept; the sent turn is saved",
  "toast.edit_target_removed":
    "The question you were editing was removed; its draft is preserved",
  "toast.rejected_no_dispatch": "Request blocked before sending: {reason}",
  "toast.exported": "Exported {sections} section(s)",
  "toast.conversation_deleted": "Chat deleted",
  "toast.group_created": "Group created",
  "toast.group_deleted": "Group deleted",
  "toast.conversation_moved": "Chat moved",

  "failure.egress_blocked": "Network region restricted (CN/HK)",
  "failure.egress_unknown": "Network region could not be verified",
  "failure.unauthorized": "Invalid API key (401)",
  "failure.forbidden": "Permission or policy denied (403)",
  "failure.payment_required": "Insufficient balance or credits (402)",
  "failure.rate_limited": "Rate limited",
  "failure.context_length": "Context length exceeded",
  "failure.invalid_request": "Request rejected (check input/parameters)",
  "failure.model_unavailable": "Model unavailable",
  "failure.network": "Network error",
  "failure.timeout": "Timed out",
  "failure.server": "Provider server error",
  "failure.stream_aborted": "Stream ended early (partial answer kept)",
  "failure.empty_response": "Empty response",
  "failure.protocol_failure": "Stream protocol failure",
  "failure.parse_failure": "Could not parse response",
  "failure.truncated": "Output truncated (content kept)",
  "failure.interrupted": "Interrupted by restart",
  "failure.cancelled": "Cancelled",
  "failure.unknown": "Failed",

  "retry.in_flight": "Still running",
  "retry.cooldown": "Retry available in {s}s",
  "retry.needs.switch_ip": "Switch IP or re-check network first",
  "retry.needs.credentials": "Fix your API key in Settings first",
  "retry.needs.balance": "Check your account balance first",
  "retry.needs.edit_input": "Edit your input or parameters first",
  "retry.needs.pick_model": "Pick another model first",
  "retry.needs.none": "Action needed before retrying",

  "message.partial": "Partial answer",
  "message.truncated": "Truncated",
  "message.content_filtered": "Filtered",
  "message.finish_other": "Finished ({reason})",
  "message.attempts": "Attempt {n}",
  "message.show_attempts": "Show {n} older attempt(s)",
  "message.hide_attempts": "Hide older attempts",
  "message.regenerate": "Regenerate",
  "message.may_be_billed": "Last attempt may already be billed",

  "divider.tooltip": "Drag to resize; double-click to reset",
  "a11y.sidebar": "Chat list",
  "a11y.groups": "Groups",
  "a11y.messages": "Conversation",
  "a11y.answer_tabs": "Answers",
  "a11y.composer": "Message composer",
  "a11y.model_list": "Model list",
  "a11y.toggle_sidebar": "Toggle sidebar",
};

/** Look up a string; unknown keys fall back to the key itself. */
export function t(key: string): string {
  const value = TABLE[key];
  return value === undefined ? key : value;
}

/** Interpolate `{name}` placeholders. */
export function f(key: string, args: Record<string, string | number>): string {
  let out = t(key);
  for (const [name, value] of Object.entries(args)) {
    out = out.split(`{${name}}`).join(String(value));
  }
  return out;
}

/** English pluralisation (`1 model` / `2 models`). */
export function plural(key: string, count: number): string {
  const template = t(count === 1 ? `${key}.one` : `${key}.other`);
  return template.split("{n}").join(String(count));
}

export const stringKeys = Object.keys(TABLE);
