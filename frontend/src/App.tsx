import { useEffect } from "react";

import { t } from "./i18n/strings";
import { Composer } from "./components/Composer";
import { Icon } from "./components/Icon";
import { MessageThread } from "./components/MessageThread";
import { ModelsPanel } from "./components/ModelsPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { Sidebar } from "./components/Sidebar";
import { SidebarResizer } from "./components/SidebarResizer";
import { useStore } from "./state/store";

export function App() {
  const { state, actions } = useStore();

  // Keep the body size and the sidebar width in sync with the settings.
  useEffect(() => {
    const size = state.settings.body_text_size ?? 15;
    document.documentElement.style.setProperty("--body-size", `${size}px`);
  }, [state.settings.body_text_size]);

  useEffect(() => {
    // The contract allows 240–480 px; a backend default of 0 (or a stale
    // value) must not collapse the sidebar to zero width.
    const raw = state.settings.sidebar_width ?? 0;
    const width = raw >= 220 && raw <= 460 ? raw : 300;
    document.documentElement.style.setProperty("--sidebar-width", `${width}px`);
  }, [state.settings.sidebar_width]);

  if (state.status === "booting") {
    return (
      <div className="app" data-sidebar="expanded">
        <div className="empty" style={{ gridColumn: "1 / -1" }}>
          <div className="empty__title">AiCouncil</div>
          <p>Loading…</p>
        </div>
      </div>
    );
  }

  if (state.status === "error") {
    return (
      <div className="app" data-sidebar="expanded">
        <div className="empty" style={{ gridColumn: "1 / -1" }}>
          <div className="empty__title">{t("dialog.db.title")}</div>
          <p>{state.bootError}</p>
          <button type="button" className="btn btn--accent" onClick={() => void actions.bootstrap()}>
            Retry
          </button>
        </div>
      </div>
    );
  }

  const conversation = state.conversations.find(
    (entry) => entry.id === state.currentConversationId,
  );

  return (
    <div className="app" data-sidebar={state.sidebarCollapsed ? "collapsed" : "expanded"}>
      {state.sidebarCollapsed ? null : (
        <>
          <Sidebar />
          <SidebarResizer />
        </>
      )}

      <main className="main">
        <header className="main__header">
          {state.sidebarCollapsed ? (
            <button
              type="button"
              className="btn btn--icon"
              title={t("panel.sidebar_collapse_tooltip")}
              aria-label={t("a11y.toggle_sidebar")}
              onClick={() => actions.toggleSidebar(false)}
            >
              <Icon name="sidebar" />
            </button>
          ) : null}

          <h2 className="main__title">
            {conversation?.title.trim() || t("conversation.untitled")}
          </h2>
          {conversation?.title_source === "fallback" ? (
            <span className="main__subtitle">{t("title.auto_failed_hint")}</span>
          ) : null}

          <div style={{ marginLeft: "auto", display: "flex", gap: "var(--space-xs)" }}>
            <button
              type="button"
              className="btn"
              aria-current={state.panel === "models"}
              onClick={() =>
                actions.setPanel(state.panel === "models" ? "history" : "models")
              }
            >
              <Icon name="models" size={14} />
              {t("panel.models")}
            </button>
            <button
              type="button"
              className="btn"
              aria-current={state.panel === "settings"}
              onClick={() =>
                actions.setPanel(state.panel === "settings" ? "history" : "settings")
              }
            >
              <Icon name="settings" size={14} />
              {t("panel.settings")}
            </button>
          </div>
        </header>

        <div className="main__body">
          <MessageThread />
        </div>

        <Composer />
      </main>

      {state.panel === "models" ? (
        <aside className="right-panel" aria-label={t("panel.models")}>
          <ModelsPanel />
        </aside>
      ) : null}

      {state.panel === "settings" ? (
        <aside className="right-panel" aria-label={t("panel.settings")}>
          <SettingsPanel />
        </aside>
      ) : null}

      {state.toast ? (
        <div
          className="toast"
          data-kind={state.toast.kind}
          role={state.toast.kind === "error" ? "alert" : "status"}
          onClick={() => actions.dismissToast()}
        >
          {state.toast.text}
        </div>
      ) : null}

      <div className="sr-only-live" role="status" aria-live="polite">
        {state.toast?.text ?? ""}
      </div>
    </div>
  );
}
