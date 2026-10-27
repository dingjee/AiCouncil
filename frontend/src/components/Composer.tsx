import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type KeyboardEvent,
} from "react";

import { t } from "../i18n/strings";
import { presentation } from "../model/presentation";
import { enabledModels, resolveSelection, usageSummary } from "../state/selectors";
import { useStore } from "../state/store";
import { shouldConvertPaste } from "../util/attachments";
import { formatTokens } from "../util/money";
import { composerKeyAction } from "../util/keyboard";
import { Icon } from "./Icon";
import { ModelPicker } from "./ModelPicker";

/**
 * Composer.
 *
 * Keyboard contract (documented in the README and in docs/MMF_STATUS.md):
 *   - `Enter` sends the message;
 *   - `Shift+Enter` inserts a newline;
 *   - `Cmd+Enter` (and `Ctrl+Enter`) sends the message as well;
 *   - while an IME composition is active, `Enter` belongs to the IME and
 *     never sends.
 */
export function Composer() {
  const { state, actions, dispatch } = useStore();
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const fileRef = useRef<HTMLInputElement | null>(null);
  const composingRef = useRef(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [usageOpen, setUsageOpen] = useState(false);

  const editing = state.editingLatest;
  const value = editing ? state.editText : state.composerText;
  const attachments = editing ? state.editAttachments : state.composerAttachments;

  const enabled = useMemo(() => enabledModels(state), [state]);
  const selection = useMemo(
    () => resolveSelection(enabled, [], state.modelSelection),
    [enabled, state.modelSelection],
  );

  const streaming = useMemo(() => {
    const conversationId = state.currentConversationId;
    if (!conversationId) return false;
    return (state.generations[conversationId] ?? []).some((generation) =>
      ["queued", "connecting", "streaming"].includes(generation.status),
    );
  }, [state.currentConversationId, state.generations]);

  const usage = useMemo(() => {
    const conversationId = state.currentConversationId;
    if (!conversationId) return null;
    const records = state.usage[conversationId] ?? [];
    const summary = usageSummary(records);
    const promptTokens = records.reduce(
      (sum, record) => sum + (record.prompt_tokens ?? 0),
      0,
    );
    const completionTokens = records.reduce(
      (sum, record) => sum + (record.completion_tokens ?? 0),
      0,
    );
    const knownTokens = records.some((record) => record.total_tokens !== null);
    return { ...summary, promptTokens, completionTokens, knownTokens };
  }, [state.currentConversationId, state.usage]);

  const modelLabel = useMemo(() => {
    if (!selection.primary) return t("composer.model_picker");
    const entry = state.catalog.find((item) => item.id === selection.primary);
    if (!entry) return selection.primary;
    return presentation(entry.id, entry.name).full_label;
  }, [selection.primary, state.catalog]);

  const setValue = useCallback(
    (next: string) => {
      if (editing) dispatch({ type: "edit/text", text: next });
      else actions.setComposerText(next);
    },
    [actions, dispatch, editing],
  );

  const handleKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTextAreaElement>) => {
      const action = composerKeyAction({
        key: event.key,
        shiftKey: event.shiftKey,
        metaKey: event.metaKey,
        ctrlKey: event.ctrlKey,
        isComposing: event.nativeEvent.isComposing || composingRef.current,
        keyCode: event.keyCode,
      });
      if (action === "ignore") return;
      if (action === "newline") return; // Shift+Enter inserts a newline
      event.preventDefault();
      void actions.send();
    },
    [actions],
  );

  const handlePaste = useCallback(
    (event: ClipboardEvent<HTMLTextAreaElement>) => {
      const text = event.clipboardData.getData("text/plain");
      if (!shouldConvertPaste(text)) return;
      // Long pastes become attachments instead of a huge composer body.
      event.preventDefault();
      actions.addPastedText(text);
    },
    [actions],
  );

  const handleFiles = useCallback(
    (files: FileList | null) => {
      if (!files || files.length === 0) return;
      void actions.addAttachments(Array.from(files));
    },
    [actions],
  );

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [value]);

  const canSend = !selection.invalidReason && (value.trim().length > 0 || attachments.length > 0);

  return (
    <div className="composer">
      <div className="composer__inner">
        {editing ? (
          <div className="composer__edit-banner">
            <Icon name="pencil" size={13} />
            <span style={{ flex: 1 }}>{t("composer.editing")}</span>
            <button
              type="button"
              className="btn"
              title={t("composer.cancel_edit_tooltip")}
              onClick={() => actions.cancelEdit()}
            >
              {t("composer.cancel_edit")}
            </button>
          </div>
        ) : null}

        {state.branchSelection.length > 0 ? (
          <p className="hint">{t("composer.branch_mode_hint")}</p>
        ) : null}

        {attachments.length > 0 ? (
          <div className="composer__chips">
            {attachments.map((attachment, index) => (
              <span className="chip" key={`${attachment.name}-${index}`}>
                <Icon name="paperclip" size={11} />
                <span className="chip__name">{attachment.name}</span>
                <button
                  type="button"
                  className="btn btn--icon"
                  style={{ minHeight: 18, minWidth: 18 }}
                  aria-label={`${t("attachment.remove")}: ${attachment.name}`}
                  onClick={() => {
                    if (editing) {
                      dispatch({
                        type: "edit/attachments",
                        attachments: state.editAttachments.filter(
                          (_, i) => i !== index,
                        ),
                      });
                    } else {
                      actions.removeAttachment(index);
                    }
                  }}
                >
                  <Icon name="close" size={10} />
                </button>
              </span>
            ))}
          </div>
        ) : null}

        <div className="composer__box">
          <textarea
            ref={textareaRef}
            className="composer__input"
            value={value}
            placeholder={t("composer.placeholder")}
            aria-label={t("a11y.composer")}
            rows={1}
            onCompositionStart={() => {
              composingRef.current = true;
            }}
            onCompositionEnd={() => {
              composingRef.current = false;
            }}
            onChange={(event) => setValue(event.target.value)}
            onKeyDown={handleKeyDown}
            onPaste={handlePaste}
          />

          <div className="composer__row">
            <button
              type="button"
              className="btn"
              title={t("composer.attach_tooltip")}
              aria-label={t("composer.attach")}
              onClick={() => fileRef.current?.click()}
            >
              <Icon name="paperclip" size={14} />
            </button>
            <input
              ref={fileRef}
              type="file"
              multiple
              accept="text/*,.txt,.md,.json,.csv,.log,.ts,.tsx,.js,.jsx,.py,.rs,.toml,.yaml,.yml"
              className="visually-hidden"
              onChange={(event) => {
                handleFiles(event.target.files);
                event.target.value = "";
              }}
            />

            <div className="composer__models">
              <button
                type="button"
                className="btn"
                title={
                  selection.targets.length > 1
                    ? t("composer.models_tooltip").replace(
                        "{models}",
                        selection.targets.join(", "),
                      )
                    : t("composer.models_single_tooltip")
                }
                aria-haspopup="dialog"
                aria-expanded={state.pickerOpen}
                onClick={() => dispatch({ type: "picker/toggle" })}
              >
                <Icon name="models" size={14} />
                <span className="composer__model-name">{modelLabel}</span>
                {selection.primary ? (
                  <span className="badge">{t("composer.primary_badge")}</span>
                ) : null}
              </button>

              <button
                type="button"
                className={state.modelSelection.parallel ? "btn btn--active" : "btn"}
                title={t("composer.parallel_tooltip")}
                aria-pressed={state.modelSelection.parallel}
                onClick={() => actions.setParallel(!state.modelSelection.parallel)}
              >
                {t("composer.parallel")}
              </button>
            </div>

            <div className="composer__row composer__row--end" style={{ marginLeft: "auto" }}>
              <button
                type="button"
                className="btn"
                title={t("panel.export_tooltip")}
                aria-haspopup="menu"
                aria-expanded={exportOpen}
                onClick={() => setExportOpen((open) => !open)}
              >
                <Icon name="export" size={14} />
              </button>
              <button
                type="button"
                className="btn"
                title={t("composer.usage_tooltip")}
                aria-expanded={usageOpen}
                onClick={() => setUsageOpen((open) => !open)}
              >
                {t("composer.usage")}
              </button>

              {streaming ? (
                <button
                  type="button"
                  className="btn btn--danger"
                  title={t("composer.stop_tooltip")}
                  onClick={() => {
                    const conversationId = state.currentConversationId;
                    if (!conversationId) return;
                    const live = (state.generations[conversationId] ?? []).filter(
                      (generation) =>
                        ["queued", "connecting", "streaming"].includes(
                          generation.status,
                        ),
                    );
                    live.forEach((generation) =>
                      void actions.cancel(generation.request_id),
                    );
                  }}
                >
                  <Icon name="stop" size={14} />
                  {t("message.stop")}
                </button>
              ) : (
                <button
                  type="button"
                  className="btn btn--accent"
                  disabled={!canSend}
                  title={
                    selection.invalidReason
                      ? t(selection.invalidReason)
                      : t("composer.send_tooltip")
                  }
                  onClick={() => void actions.send()}
                >
                  <Icon name="send" size={14} />
                  {t("composer.send")}
                </button>
              )}
            </div>
          </div>
        </div>

        <div className="composer__status">
          {selection.invalidReason ? (
            <span>{t(selection.invalidReason)}</span>
          ) : null}
          {usage ? (
            <span>
              {usage.knownTotal !== null
                ? t("usage.known_pending")
                    .replace("{known}", usage.knownTotal)
                    .replace("{pending}", String(usage.pending))
                    .replace("{unavailable}", String(usage.unavailable))
                : t("usage.unknown")}
            </span>
          ) : null}
          <span className="muted">
            {t("composer.send_tooltip")} · Shift+Enter: newline · Cmd+Enter: send
          </span>
        </div>
      </div>

      {usageOpen && usage ? (
        <div
          className="card"
          role="dialog"
          aria-label={t("composer.usage")}
          style={{
            position: "absolute",
            right: "var(--space-l)",
            bottom: "160px",
            zIndex: 30,
            padding: "var(--space-s)",
            minWidth: "220px",
          }}
        >
          <p className="small" style={{ margin: "0 0 4px" }}>
            {usage.knownTotal !== null
              ? t("usage.known_pending")
                  .replace("{known}", usage.knownTotal)
                  .replace("{pending}", String(usage.pending))
                  .replace("{unavailable}", String(usage.unavailable))
              : t("usage.unknown")}
          </p>
          <p className="small muted" style={{ margin: 0 }}>
            {usage.knownTokens
              ? t("usage.tokens")
                  .replace("{prompt}", formatTokens(usage.promptTokens))
                  .replace("{completion}", formatTokens(usage.completionTokens))
              : t("usage.unknown")}
          </p>
        </div>
      ) : null}

      {exportOpen ? (
        <div
          className="card"
          role="menu"
          aria-label={t("panel.export_tooltip")}
          style={{
            position: "absolute",
            right: "var(--space-l)",
            bottom: "160px",
            zIndex: 30,
            padding: "var(--space-xs)",
          }}
        >
          <button
            type="button"
            className="btn"
            role="menuitem"
            style={{ width: "100%", justifyContent: "flex-start" }}
            onClick={() => {
              setExportOpen(false);
              void actions.exportMarkdown("main");
            }}
          >
            {t("panel.export_main")}
          </button>
          {state.branchSelection.length > 0 ? (
            <button
              type="button"
              className="btn"
              role="menuitem"
              style={{ width: "100%", justifyContent: "flex-start" }}
              onClick={() => {
                setExportOpen(false);
                void actions.exportMarkdown("selected");
              }}
            >
              {t("panel.export_selected").replace(
                "{n}",
                String(state.branchSelection.length),
              )}
            </button>
          ) : null}
          <button
            type="button"
            className="btn"
            role="menuitem"
            style={{ width: "100%", justifyContent: "flex-start" }}
            onClick={() => {
              setExportOpen(false);
              void actions.exportMarkdown("all_branches");
            }}
          >
            {t("panel.export_all")}
          </button>
        </div>
      ) : null}

      {state.pickerOpen ? (
        <ModelPicker onClose={() => dispatch({ type: "picker/toggle", open: false })} />
      ) : null}
    </div>
  );
}
