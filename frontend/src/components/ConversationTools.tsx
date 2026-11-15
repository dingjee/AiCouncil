import { useEffect, useMemo, useRef, useState } from "react";

import { t } from "../i18n/strings";
import { conversationCost } from "../state/selectors";
import { useStore } from "../state/store";
import { formatTokens } from "../util/money";
import { usePopupDismiss } from "../util/popup";
import { Icon } from "./Icon";

type OpenMenu = "cost" | "export" | null;

/**
 * Conversation tools (top right).
 *
 * Two entries, in this order:
 *
 *   1. the **cost of this chat** — the exact sum of the known costs of every
 *      usage record of the conversation (all branches, all retries), never
 *      the quota of the API key;
 *   2. **Export** — the same three scopes as before (main branch, the
 *      selected branches, all branches).
 *
 * The popovers belong to the entries: they open below the button that owns
 * them, `Escape` and a click outside close them, and switching to another
 * chat closes the open one.
 */
export function ConversationTools() {
  const { state, actions } = useStore();
  const [open, setOpen] = useState<OpenMenu>(null);
  const costButtonRef = useRef<HTMLButtonElement | null>(null);
  const exportButtonRef = useRef<HTMLButtonElement | null>(null);
  const costPopoverRef = useRef<HTMLDivElement | null>(null);
  const exportPopoverRef = useRef<HTMLDivElement | null>(null);

  const conversationId = state.currentConversationId;
  const records = conversationId ? state.usage[conversationId] : undefined;

  const cost = useMemo(() => conversationCost(records), [records]);

  const tokens = useMemo(() => {
    if (!records) return null;
    const prompt = records.reduce(
      (sum, record) => sum + (record.prompt_tokens ?? 0),
      0,
    );
    const completion = records.reduce(
      (sum, record) => sum + (record.completion_tokens ?? 0),
      0,
    );
    const known = records.some((record) => record.total_tokens !== null);
    return { prompt, completion, known };
  }, [records]);

  // Another chat means another ledger: the open popover is closed.
  useEffect(() => {
    setOpen(null);
  }, [conversationId]);

  // The popover closes as soon as it is not the active surface any more: a
  // press outside, the focus leaving it, the window losing the focus or
  // `Escape` (which hands the focus back to the button that owns it).
  usePopupDismiss({
    open: open !== null,
    onDismiss: () => setOpen(null),
    popupRef: open === "export" ? exportPopoverRef : costPopoverRef,
    trigger: () =>
      open === "export" ? exportButtonRef.current : costButtonRef.current,
    restoreFocus: true,
  });

  // Without a chat (or before it was loaded) there is no amount to show.
  const label =
    conversationId && records ? cost.label : t("toolbar.no_conversation");

  return (
    <div className="main__toolbar">
      <button
        ref={costButtonRef}
        type="button"
        className="btn"
        data-testid="toolbar-cost"
        title={t("toolbar.cost_tooltip")}
        aria-haspopup="dialog"
        aria-expanded={open === "cost"}
        onClick={() => setOpen((current) => (current === "cost" ? null : "cost"))}
      >
        <Icon name="chats" size={14} />
        <span className="toolbar__cost">{label}</span>
      </button>

      <button
        ref={exportButtonRef}
        type="button"
        className="btn"
        data-testid="toolbar-export"
        title={t("panel.export_tooltip")}
        aria-haspopup="menu"
        aria-expanded={open === "export"}
        onClick={() => setOpen((current) => (current === "export" ? null : "export"))}
      >
        <Icon name="export" size={14} />
        {t("panel.export")}
      </button>

      {open === "cost" ? (
        <div
          ref={costPopoverRef}
          className="toolbar-popover"
          role="dialog"
          aria-label={t("toolbar.cost_details")}
          data-testid="cost-popover"
        >
          <p className="small" style={{ margin: "0 0 4px" }}>
            {cost.knownTotal !== null
              ? t("usage.known_pending")
                  .replace("{known}", cost.knownTotal)
                  .replace("{pending}", String(cost.pending))
                  .replace("{unavailable}", String(cost.unavailable))
              : t("usage.unknown")}
          </p>
          <p className="small muted" style={{ margin: 0 }}>
            {tokens?.known
              ? t("usage.tokens")
                  .replace("{prompt}", formatTokens(tokens.prompt))
                  .replace("{completion}", formatTokens(tokens.completion))
              : t("usage.unknown")}
          </p>
          {cost.partial ? (
            <p className="small muted" style={{ margin: "4px 0 0" }}>
              {t("toolbar.cost_partial_hint")}
            </p>
          ) : null}
        </div>
      ) : null}

      {open === "export" ? (
        <div
          ref={exportPopoverRef}
          className="toolbar-popover"
          role="menu"
          aria-label={t("panel.export_tooltip")}
          data-testid="export-menu"
        >
          <button
            type="button"
            className="btn"
            role="menuitem"
            style={{ width: "100%", justifyContent: "flex-start" }}
            onClick={() => {
              setOpen(null);
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
                setOpen(null);
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
              setOpen(null);
              void actions.exportMarkdown("all_branches");
            }}
          >
            {t("panel.export_all")}
          </button>
        </div>
      ) : null}
    </div>
  );
}
