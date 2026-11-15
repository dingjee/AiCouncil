import { useEffect } from "react";

import { t } from "./i18n/strings";
import { Composer } from "./components/Composer";
import { ConversationTools } from "./components/ConversationTools";
import { EgressModal } from "./components/EgressModal";
import { Icon } from "./components/Icon";
import { MessageThread } from "./components/MessageThread";
import { Modal } from "./components/Modal";
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
      <div className="app" data-sidebar="expanded" data-testid="app">
        <div className="empty">
          <div className="empty__title">AiCouncil</div>
          <p>Loading…</p>
        </div>
      </div>
    );
  }

  if (state.status === "error") {
    return (
      <div className="app" data-sidebar="expanded" data-testid="app">
        <div className="empty">
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
    <div
      className="app"
      data-sidebar={state.sidebarCollapsed ? "collapsed" : "expanded"}
      data-testid="app"
    >
      {state.sidebarCollapsed ? null : (
        <>
          <Sidebar />
          <SidebarResizer />
        </>
      )}

      <main className="main" data-testid="main">
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
          {/* Only a **materialized** failure shows the hint: fallback source +
              needs_generation + at least one real attempt. A pending first
              attempt (needs_generation with 0 attempts, or in_flight) is not a
              failure and keeps the quiet subtitle. */}
          {conversation?.title_source === "fallback" &&
          conversation.title_status === "needs_generation" &&
          conversation.title_attempt_count > 0 ? (
            <span className="main__subtitle">{t("title.auto_failed_hint")}</span>
          ) : null}

          <ConversationTools />
        </header>

        <div className="main__body">
          <MessageThread />
        </div>

        <Composer />
      </main>

      {state.modal === "models" ? (
        <Modal
          title={t("modal.models_title")}
          testId="modal-models"
          onClose={() => actions.closeModal()}
        >
          <ModelsPanel />
        </Modal>
      ) : null}

      {state.modal === "settings" ? (
        <Modal
          title={t("modal.settings_title")}
          testId="modal-settings"
          onClose={() => actions.closeModal()}
        >
          <SettingsPanel />
        </Modal>
      ) : null}

      <EgressModal />

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
