/**
 * Shared dismissal rules for the transient popups.
 *
 * Every popup of the app (the composer picker, the toolbar popovers, the
 * sidebar menus, the delete confirmation) follows the same contract:
 *
 *   - a **pointer press outside** the popup (and outside the control that
 *     opened it) closes it immediately — the press is not swallowed, so the
 *     click still reaches the control underneath;
 *   - the **keyboard focus leaving** the popup and its trigger closes it
 *     (Tab out of the picker, focus moved to another widget);
 *   - the **window losing the focus** (the user left the app) closes it;
 *   - `Escape` closes it and gives the focus back to the trigger.
 *
 * Two rules keep the interaction sane:
 *
 *   1. A press on the **trigger** never closes the popup: the trigger's own
 *      click handler toggles it, so the popup must not close and reopen.
 *   2. A dismissal caused by a pointer press **never steals the focus**: the
 *      focus stays where the user put it.
 *
 * Content inside the popup never dismisses it — a click on a row, a star or
 * a menu entry is *internal navigation* and must not close the surface.
 */

import { useEffect, useRef, type RefObject } from "react";

export interface PopupDismissOptions {
  /** Whether the popup is open right now. */
  open: boolean;
  /** Called when the popup has to close. */
  onDismiss(): void;
  /** The popup element (the panel that holds the content). */
  popupRef: RefObject<HTMLElement | null>;
  /**
   * The control that opened the popup.
   *
   * It is a getter (not a ref) because a popup may have more than one
   * trigger — the toolbar switches between the cost and the export entry.
   */
  trigger?: () => HTMLElement | null;
  /** Give the focus back to the trigger when `Escape` closes the popup. */
  restoreFocus?: boolean;
}

/**
 * Dismiss a transient popup the moment it is no longer the active surface.
 *
 * The listeners are registered once per open/close cycle; the callbacks are
 * read from a ref so a re-render never re-registers them (and never drops a
 * pending dismissal).
 */
export function usePopupDismiss(options: PopupDismissOptions): void {
  const latest = useRef(options);
  latest.current = options;
  const open = options.open;

  useEffect(() => {
    if (!open) return;

    /** Is the node inside the popup or inside the control that opened it? */
    const inside = (node: EventTarget | null): boolean => {
      if (!node || typeof node !== "object" || !("nodeType" in node)) return false;
      const popup = latest.current.popupRef.current;
      if (popup && popup.contains(node as Node)) return true;
      const trigger = latest.current.trigger?.();
      if (trigger && trigger.contains(node as Node)) return true;
      return false;
    };

    const dismiss = () => latest.current.onDismiss();

    /**
     * A press **inside** the popup may move the focus to the body (the user
     * pressed a group title, a hint or any non-focusable element). That blur
     * is internal — the popup must stay open. The flag is cleared in the next
     * task, so a later, real focus loss still closes the popup.
     */
    let internalPress = false;
    let internalPressTimer: ReturnType<typeof setTimeout> | null = null;

    // 1) A pointer press outside the popup closes it. The press is not
    //    prevented: the click still belongs to the control underneath.
    const onPointerDown = (event: Event) => {
      if (inside(event.target)) {
        internalPress = true;
        if (internalPressTimer !== null) clearTimeout(internalPressTimer);
        internalPressTimer = setTimeout(() => {
          internalPress = false;
          internalPressTimer = null;
        }, 0);
        return;
      }
      dismiss();
    };

    // 2) The focus left the popup (and its trigger): the user works
    //    somewhere else now.
    const onFocusIn = (event: FocusEvent) => {
      if (inside(event.target)) return;
      // The focus fell back to the body after a press inside the popup.
      if (internalPress) {
        internalPress = false;
        return;
      }
      dismiss();
    };
    const onFocusOut = (event: FocusEvent) => {
      const next = event.relatedTarget;
      // The focus moved to another element of the popup: nothing to do.
      if (next && inside(next)) return;
      // The blur was caused by a press inside the popup (a non-focusable
      // element took the focus): the popup stays open.
      if (internalPress) {
        internalPress = false;
        return;
      }
      dismiss();
    };

    // 3) The window lost the focus (the user left the app).
    const onWindowBlur = () => dismiss();

    // 4) Escape closes the popup and hands the focus back to the trigger.
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      dismiss();
      if (latest.current.restoreFocus === false) return;
      const trigger = latest.current.trigger?.();
      if (trigger && typeof trigger.focus === "function") trigger.focus();
    };

    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    window.addEventListener("blur", onWindowBlur);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      if (internalPressTimer !== null) clearTimeout(internalPressTimer);
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
      window.removeEventListener("blur", onWindowBlur);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);
}
