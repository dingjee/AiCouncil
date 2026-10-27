/**
 * Composer keyboard contract.
 *
 * The rule set is pure so it can be unit tested without a browser:
 *
 *   - while an IME composition is running, `Enter` belongs to the IME and
 *     must never send (the classic Chinese/Japanese input bug);
 *   - `Shift+Enter` inserts a newline;
 *   - `Enter` sends;
 *   - `Cmd+Enter` / `Ctrl+Enter` send as well (documented shortcut).
 */

export type ComposerKeyAction = "send" | "newline" | "ignore";

export interface ComposerKeyEvent {
  key: string;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  /** True while an IME composition is active. */
  isComposing?: boolean;
  /** Legacy IME signal (keyCode 229) used by some WebKit builds. */
  keyCode?: number;
}

export const IME_KEY_CODE = 229;

export function composerKeyAction(event: ComposerKeyEvent): ComposerKeyAction {
  if (event.isComposing) return "ignore";
  if (event.keyCode === IME_KEY_CODE) return "ignore";
  if (event.key !== "Enter") return "ignore";
  if (event.shiftKey) return "newline";
  return "send";
}

/** True when the event should be swallowed (never reaches the IME). */
export function shouldPreventDefault(event: ComposerKeyEvent): boolean {
  return composerKeyAction(event) === "send";
}
