import { t } from "../i18n/strings";
import { useStore } from "../state/store";
import { Modal } from "./Modal";

/**
 * The Anthropic egress gate.
 *
 * When the network region blocks Anthropic models, the send is held back
 * and the user decides what happens next: cancel, check the region again
 * or send the question to the other models only. It is a modal like the
 * other popups, so it never resizes the chat.
 */
export function EgressModal() {
  const { state, actions } = useStore();
  const modal = state.egressModal;
  if (!modal) return null;

  return (
    <Modal
      title={t("egress.title")}
      testId="modal-egress"
      onClose={() => actions.dismissEgressModal()}
    >
      <p style={{ margin: 0 }}>
        {modal.unknown
          ? t("egress.unknown")
          : t("egress.blocked").replace("{country}", modal.country ?? "?")}
      </p>
      {modal.detail ? (
        <p className="hint" style={{ margin: 0 }}>
          {t("egress.detail").replace("{detail}", modal.detail)}
        </p>
      ) : null}
      <div className="modal__actions">
        <button
          type="button"
          className="btn"
          onClick={() => actions.dismissEgressModal()}
        >
          {t("egress.cancel")}
        </button>
        <button
          type="button"
          className="btn"
          disabled={modal.checking}
          onClick={() => void actions.recheckEgress()}
        >
          {t("egress.recheck")}
        </button>
        {modal.mixed ? (
          <button
            type="button"
            className="btn btn--accent"
            onClick={() => void actions.sendWithoutAnthropic()}
          >
            {t("egress.send_without")}
          </button>
        ) : null}
      </div>
    </Modal>
  );
}
