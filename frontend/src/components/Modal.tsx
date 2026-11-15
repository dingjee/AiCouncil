import {
  useCallback,
  useEffect,
  useId,
  useRef,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { t } from "../i18n/strings";
import { usePopupDismiss } from "../util/popup";
import { Icon } from "./Icon";

/**
 * Accessible modal dialog.
 *
 * Models and Settings used to live in a right-hand layout column, which
 * squeezed the chat and broke its scrolling. They are popups now:
 *
 *   - the dialog is rendered in a portal, so it can never influence the
 *     geometry of the chat (width, scroll position, composer);
 *   - it has a title, a close button and an accessible name;
 *   - `Escape` closes it, `Tab` is trapped inside it and the focus returns
 *     to the control that opened it;
 *   - the body scrolls on its own, so a long catalog stays inside the
 *     dialog at every window size.
 */

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

/** Put a saved text selection back into the document. */
function restoreRange(range: Range): void {
  const selection = window.getSelection();
  if (!selection) return;
  selection.removeAllRanges();
  selection.addRange(range);
}

export interface ModalProps {
  /** Visible title; also the accessible name of the dialog. */
  title: string;
  /** Called by the close button, the scrim and `Escape`. */
  onClose(): void;
  children: ReactNode;
  /** Stable hook for the geometry/native harnesses. */
  testId?: string;
  /** Width preset; both keep the dialog inside a 880x520 window. */
  size?: "default" | "wide";
}

export function Modal({ title, onClose, children, testId, size }: ModalProps) {
  const panelRef = useRef<HTMLDivElement | null>(null);
  const restoreRef = useRef<HTMLElement | null>(null);
  const titleId = useId();

  // The dialog closes when it is not the active surface any more: the focus
  // leaves it or the window loses the focus. The scrim, the close button and
  // `Escape` are handled below (the dialog restores the focus itself).
  usePopupDismiss({
    open: true,
    onDismiss: onClose,
    popupRef: panelRef,
    restoreFocus: false,
  });

  // Move the focus into the dialog and give it back on close.
  useEffect(() => {
    restoreRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    // A text selection in the chat must survive the dialog: focusing an
    // element clears the selection, so it is captured and restored.
    const selection = window.getSelection();
    const savedRange =
      selection && selection.rangeCount > 0 && !selection.isCollapsed
        ? selection.getRangeAt(0).cloneRange()
        : null;
    const panel = panelRef.current;
    panel?.focus();
    if (savedRange) restoreRange(savedRange);
    return () => {
      const restore = restoreRef.current;
      restoreRef.current = null;
      if (restore && document.contains(restore)) restore.focus();
      if (savedRange) restoreRange(savedRange);
    };
  }, []);

  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape") {
        event.stopPropagation();
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== "Tab") return;
      const panel = panelRef.current;
      if (!panel) return;
      const focusable = Array.from(
        panel.querySelectorAll<HTMLElement>(FOCUSABLE),
      ).filter((element) => element.getClientRects().length > 0);
      if (focusable.length === 0) {
        event.preventDefault();
        panel.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      const active = document.activeElement;
      if (event.shiftKey) {
        if (active === first || active === panel || !panel.contains(active)) {
          event.preventDefault();
          last.focus();
        }
        return;
      }
      if (active === last || !panel.contains(active)) {
        event.preventDefault();
        first.focus();
      }
    },
    [onClose],
  );

  const dialog = (
    <div className="modal-layer" data-testid={testId ? `${testId}-layer` : undefined}>
      <div
        className="modal-scrim"
        data-testid={testId ? `${testId}-scrim` : undefined}
        onClick={onClose}
      />
      <div
        className={`modal${size === "wide" ? " modal--wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        ref={panelRef}
        onKeyDown={onKeyDown}
        data-testid={testId ? `${testId}-dialog` : undefined}
      >
        <header className="modal__header">
          <h2 className="modal__title" id={titleId}>
            {title}
          </h2>
          <button
            type="button"
            className="btn btn--icon"
            onClick={onClose}
            aria-label={t("modal.close")}
            title={t("modal.close")}
            data-testid="modal-close"
          >
            <Icon name="close" size={14} />
          </button>
        </header>
        <div
          className="modal__body"
          data-testid={testId ? `${testId}-body` : undefined}
        >
          {children}
        </div>
      </div>
    </div>
  );

  if (typeof document === "undefined") return null;
  return createPortal(dialog, document.body);
}
