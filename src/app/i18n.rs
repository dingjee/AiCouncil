//! UI 文案资源(PLAN 3.3):默认 en;所有自有文字经此抽离。
//!
//! - 稳定 key + 带命名参数模板(`{name}` 占位);
//! - key 缺失运行时安全回退为 key 本身(开发检查在测试中报错);
//! - provider 原始错误、模型 ID、用户内容、自动标题不翻译。

/// 取文案(缺 key 回退 key 本身)。
pub fn t(key: &str) -> &str {
    TABLE
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| *v)
        .unwrap_or(key)
}

/// 带命名参数插值:`f("generation.failed.model", &[("model", m)])`。
pub fn f(key: &str, args: &[(&str, &str)]) -> String {
    let mut s = t(key).to_string();
    for (name, value) in args {
        s = s.replace(&format!("{{{name}}}"), value);
    }
    s
}

/// 复数:`n models` / `1 model`(英文规则;后续 locale 再扩展)。
pub fn plural(key: &str, n: usize) -> String {
    let one = format!("{key}.one");
    let other = format!("{key}.other");
    let template = if n == 1 { t(&one) } else { t(&other) };
    template.replace("{n}", &n.to_string())
}

static TABLE: &[(&str, &str)] = &[
    // 应用与窗口
    ("app.title", "AiCouncil"),
    ("dialog.data_dir.title", "AiCouncil data directory unavailable"),
    ("dialog.data_dir.body", "Move the program to a writable directory and retry.\n{error}"),
    ("dialog.db.title", "AiCouncil database failed to open"),
    // 会话
    ("conversation.new", "New chat"),
    ("conversation.untitled", "New chat"),
    ("conversation.new_button", "New chat"),
    ("conversation.search_placeholder", "Search chats"),
    ("conversation.delete", "Delete chat"),
    ("conversation.move_to", "Move current chat to:"),
    ("conversation.group_ungrouped", "Ungrouped"),
    ("conversation.fallback_date", "Chat {date}"),
    ("conversation.rename", "Rename chat"),
    // 标题编辑
    ("title.edit_tooltip", "Rename chat"),
    ("title.save", "Save"),
    ("title.placeholder", "Chat title"),
    ("title.empty_rejected", "Title cannot be empty"),
    ("title.save_failed", "Failed to save title: {error}"),
    ("title.too_long", "Title is limited to 80 characters"),
    ("title.auto_failed_hint", "Auto title unavailable (will retry on next question)"),
    // 消息与操作
    ("message.role.user", "You"),
    ("message.role.assistant", "Assistant"),
    ("message.copy", "Copy source"),
    ("message.copy_tooltip", "Copy full Markdown source of this message"),
    ("message.retry", "Retry"),
    ("message.continue", "Continue from here"),
    ("message.continue_tooltip", "Make this answer the main branch and continue"),
    ("message.stop", "Stop"),
    ("message.select_branch", "Branch"),
    ("message.select_branch_tooltip", "Ask follow-ups along multiple answers"),
    ("message.generating", "Generating…"),
    ("message.queued", "Queued"),
    ("message.connecting", "Connecting"),
    ("message.streaming", "Streaming…"),
    ("message.failed", "Failed"),
    ("message.cancelled", "Cancelled"),
    ("message.interrupted", "Interrupted"),
    ("message.expand", "Expand question"),
    ("message.collapse", "Collapse question"),
    // 选择(局部选择)
    ("selection.copy", "Copy"),
    ("selection.copy_tooltip", "Copy selected text (plain text; formulas as source)"),
    ("selection.cleared_hint", "Selection cleared"),
    // 输入区
    ("composer.placeholder", "Ask anything…"),
    ("composer.send", "Send"),
    ("composer.send_tooltip", "Send message (Enter)"),
    ("composer.stop_tooltip", "Stop generating"),
    ("composer.send_disabled_no_model", "Enable a model first"),
    ("composer.send_disabled_no_targets", "Select at least one model"),
    ("composer.attach", "Attach"),
    ("composer.attach_tooltip", "Attach text files; long pastes become attachments"),
    ("composer.usage", "Usage"),
    ("composer.usage_tooltip", "Show cost and key quota for this chat"),
    ("composer.parallel", "Parallel"),
    ("composer.parallel_tooltip", "Ask the same question with multiple models"),
    ("composer.models_tooltip", "Models: {models}"),
    ("composer.models_single_tooltip", "Only one model is enabled"),
    ("composer.model_picker", "Model"),
    ("composer.primary_badge", "Primary"),
    ("composer.primary_tooltip", "Primary model (owns the main branch)"),
    ("composer.branch_mode_hint", "Branch follow-up mode: each selected answer continues with its own model"),
    // 选择器
    ("selector.open", "Models"),
    ("selector.single_hint", "Click a model to choose it"),
    ("selector.multi_hint", "Check models; star sets the primary"),
    ("selector.select_all", "Select all"),
    ("selector.count.one", "{n} model"),
    ("selector.count.other", "{n} models"),
    ("selector.primary_plus", "{primary} +{n}"),
    // 附件
    ("attachment.added", "Added {n} attachment(s)"),
    ("attachment.paste_attached", "Long paste attached ({chars} chars)"),
    ("attachment.name_default", "untitled"),
    ("attachment.paste_name", "Pasted text_{time}.txt"),
    // 面板
    ("panel.history", "Chats"),
    ("panel.models", "Models"),
    ("panel.settings", "Settings"),
    ("panel.sidebar_collapse_tooltip", "Toggle sidebar"),
    ("panel.export", "Export"),
    ("panel.export_main", "Main branch"),
    ("panel.export_all", "All branches"),
    ("panel.export_tooltip", "Export chat as Markdown"),
    ("panel.back_bottom", "Back to bottom"),
    ("panel.back_bottom_tooltip", "Jump to the latest messages"),
    // 分组
    ("group.new", "New group"),
    ("group.new_placeholder", "Group name"),
    ("group.create", "Create"),
    ("group.rename", "Rename"),
    ("group.delete", "Delete"),
    ("group.save", "Save"),
    // 模型面板
    ("models.test_connection", "Test connection"),
    ("models.refresh_catalog", "Refresh catalog"),
    ("models.search_placeholder", "Search models (name/ID)"),
    ("models.mine", "My models"),
    ("models.enabled", "Enabled"),
    ("models.default", "Default"),
    ("models.set_default", "Set default"),
    ("models.current", "Current"),
    ("models.catalog", "Catalog ({n})"),
    ("models.none_enabled", "No models enabled; check some in the catalog below"),
    ("models.not_in_catalog", "Not in catalog"),
    ("models.price", "Price"),
    ("models.key_placeholder", "sk-or-…"),
    ("models.key_saved", "Key saved to encrypted credential store"),
    ("models.key_saved_volatile", "Key kept for this session only (store unavailable)"),
    ("models.key_empty", "Key is empty"),
    ("models.testing", "Testing…"),
    ("models.refreshing", "Refreshing catalog…"),
    ("models.connected", "Connected"),
    ("models.connected_no_quota", "Connected (quota query not permitted)"),
    ("models.connected_no_support", "Connected (quota query unsupported)"),
    ("models.connect_failed", "Connection failed: {error}"),
    ("models.key_valid", "Key valid"),
    ("models.key_used", " · used ${usage}"),
    ("models.key_remaining", " · remaining ${remaining}"),
    ("models.key_check_failed", "Key check failed: {error}"),
    ("models.catalog_updated", "Catalog updated ({time})"),
    ("models.catalog_failed", "Catalog refresh failed; using cache: {error}"),
    ("models.move_up_tooltip", "Move up"),
    ("models.move_down_tooltip", "Move down"),
    ("models.id_tooltip", "ID: {id}"),
    ("models.key_not_checked", "Key quota not checked yet"),
    ("models.price_unknown", "Price unknown"),
    ("models.price_free", "Free"),
    // 设置面板
    ("settings.title", "Settings"),
    ("settings.api_key", "OpenRouter API key"),
    ("settings.save", "Save"),
    ("settings.configured", "Configured"),
    ("settings.not_configured", "Not configured"),
    ("settings.data_dir", "Data directory"),
    ("settings.data_dir_unavailable", "(unavailable: {error})"),
    ("settings.concurrency", "Concurrency limit: 4 (default)"),
    ("settings.assumed_output", "Cost estimate assumes 1000 output tokens"),
    ("settings.credential_windows", "Credentials: DPAPI-encrypted for current Windows user"),
    ("settings.credential_volatile", "Credentials: this session only"),
    ("settings.export_note", "Export: main branch / all branches (Markdown, atomic write)"),
    ("settings.font_size", "Body text size"),
    ("settings.font_size_value", "{n} px"),
    ("settings.theme", "Theme: Soft Sage (built-in)"),
    ("settings.auto_title", "Automatic chat titles: DeepSeek (reads DEEPSEEK_API_KEY from .env next to the program)"),
    ("settings.title_requests", "Title helper requests: {known} known · {unknown} unknown cost"),
    // 用量
    ("usage.known_pending", "Known cost ${known} · pending {pending} · unavailable {unavailable}"),
    ("usage.title_request_mark", " (title helper)"),
    // 空态
    ("empty.title", "Start a new chat"),
    ("empty.subtitle", "Type a question and pick an enabled model to begin."),
    ("empty.configure", "Configure models"),
    // 导出
    ("export.none", "Nothing to export in this chat"),
    ("export.done", "Exported {sections} section(s) → {path}"),
    ("export.failed", "Export failed: {error}"),
    ("export.default_title", "chat"),
    // 提示与错误
    ("toast.no_model", "Enable a model in the Models panel first"),
    ("toast.branch_incomplete", "The current answer is not finished; retry or wait before continuing"),
    ("toast.branch_active", "The current answer is still generating; stop it or wait before sending"),
    ("toast.parallel_empty", "Select at least one model"),
    ("toast.continue_not_completed", "Unfinished answers cannot be used to continue"),
    ("toast.params_rejected", "Ignored unsupported parameters: {params}"),
    ("toast.send_failed", "Send failed (draft kept): {error}"),
    ("toast.retry_rejected", "Retry not available: {reason}"),
    ("toast.retry_in_flight", "This request is already being retried"),
    ("toast.storage_error", "Storage error (state may not be saved): {error}"),
    ("toast.rejected_no_dispatch", "Request blocked before sending: {reason}"),
    // 失败原因(卡片短标签;详情见 tooltip)
    ("failure.egress_blocked", "Network region restricted (CN/HK)"),
    ("failure.egress_unknown", "Network region could not be verified"),
    ("failure.unauthorized", "Invalid API key (401)"),
    ("failure.forbidden", "Permission or policy denied (403)"),
    ("failure.payment_required", "Insufficient balance or credits (402)"),
    ("failure.rate_limited", "Rate limited"),
    ("failure.context_length", "Context length exceeded"),
    ("failure.invalid_request", "Request rejected (check input/parameters)"),
    ("failure.model_unavailable", "Model unavailable"),
    ("failure.network", "Network error"),
    ("failure.timeout", "Timed out"),
    ("failure.server", "Provider server error"),
    ("failure.stream_aborted", "Stream ended early (partial answer kept)"),
    ("failure.empty_response", "Empty response"),
    ("failure.protocol_failure", "Stream protocol failure"),
    ("failure.parse_failure", "Could not parse response"),
    ("failure.truncated", "Output truncated (content kept)"),
    ("failure.interrupted", "Interrupted by restart"),
    ("failure.cancelled", "Cancelled"),
    ("failure.unknown", "Failed"),
    // 重试资格提示
    ("retry.in_flight", "Still running"),
    ("retry.cooldown", "Retry available in {s}s"),
    ("retry.needs.switch_ip", "Switch IP or re-check network first"),
    ("retry.needs.credentials", "Fix your API key in Settings first"),
    ("retry.needs.balance", "Check your account balance first"),
    ("retry.needs.edit_input", "Edit your input or parameters first"),
    ("retry.needs.pick_model", "Pick another model first"),
    ("retry.needs.none", "Action needed before retrying"),
    // 消息卡状态补充
    ("message.partial", "Partial answer"),
    ("message.truncated", "Truncated"),
    ("message.content_filtered", "Filtered"),
    ("message.finish_other", "Finished ({reason})"),
    ("message.attempts", "Attempt {n}"),
    ("message.show_attempts", "Show {n} older attempt(s)"),
    ("message.hide_attempts", "Hide older attempts"),
    ("message.regenerate", "Regenerate"),
    ("message.may_be_billed", "Last attempt may already be billed"),
    // 出口门禁弹窗(Anthropic 全模型)
    ("egress.title", "Network region check required"),
    ("egress.body.blocked", "Anthropic models cannot be sent from your current network exit (mainland China / Hong Kong). Switch to another exit IP, then re-check."),
    ("egress.body.unknown", "The network region could not be verified, so Anthropic models are blocked for this send. Check your connection or proxy, then re-check."),
    ("egress.observation", "Exit: {ip} ({country}) via {source} at {time}"),
    ("egress.recheck", "Re-check"),
    ("egress.rechecking", "Checking…"),
    ("egress.cancel", "Cancel"),
    ("egress.remove_anthropic", "Remove Anthropic models and send"),
    ("egress.applies_to", "Applies to all Anthropic models, not only Claude."),
    // 选择器(分组)
    ("selector.search_placeholder", "Search family, name or ID"),
    ("selector.no_results", "No models match \"{query}\""),
    // 标题服务
    ("title.service.system_prompt", "Generate a short chat title for the user's question. Reply with ONLY the title text: same language as the question, no answer, no explanation, no quotes, no Markdown. Chinese: 8-16 characters. Other languages: 3-8 words."),
    // 侧栏分隔条
    ("divider.tooltip", "Drag to resize; double-click to reset"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keys_resolve() {
        for (k, v) in TABLE {
            assert!(!k.is_empty());
            assert!(!v.is_empty(), "empty value for {k}");
        }
        // 抽样
        assert_eq!(t("app.title"), "AiCouncil");
        assert_ne!(t("__missing__"), ""); // 回退为 key 本身
        assert_eq!(t("__missing__"), "__missing__");
    }

    #[test]
    fn interpolation_and_plural() {
        let s = f("attachment.added", &[("n", "3")]);
        assert!(s.contains('3'), "{s}");
        assert_eq!(plural("selector.count", 1), "1 model");
        assert_eq!(plural("selector.count", 5), "5 models");
    }
}
