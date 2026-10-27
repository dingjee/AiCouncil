/**
 * Attachment limits and the long-paste rule.
 *
 * Ported from `src/domain/attachment.rs`:
 *   - single file ≤ 256 KB;
 *   - at most 10 attachments;
 *   - at most 1 MB in total;
 *   - a paste of at least 2500 characters becomes an attachment.
 */

export const MAX_ATTACHMENT_BYTES = 256 * 1024;
export const MAX_ATTACHMENTS = 10;
export const MAX_TOTAL_BYTES = 1024 * 1024;
export const LONG_PASTE_CHARS = 2500;

export interface Attachment {
  name: string;
  content: string;
}

export type AttachmentRejection =
  | "too_many"
  | "file_too_large"
  | "total_too_large";

export interface AttachmentCheck {
  ok: boolean;
  reason?: AttachmentRejection;
}

/** Can another attachment be added to the collection? */
export function canAddAttachment(
  existing: Attachment[],
  content: string,
): AttachmentCheck {
  if (existing.length >= MAX_ATTACHMENTS) return { ok: false, reason: "too_many" };
  if (content.length > MAX_ATTACHMENT_BYTES) {
    return { ok: false, reason: "file_too_large" };
  }
  const total = existing.reduce((sum, item) => sum + item.content.length, 0);
  if (total + content.length > MAX_TOTAL_BYTES) {
    return { ok: false, reason: "total_too_large" };
  }
  return { ok: true };
}

/** A paste this long is converted into an attachment instead of body text. */
export function shouldConvertPaste(text: string): boolean {
  return text.length >= LONG_PASTE_CHARS;
}

/** Default file name for a converted paste. */
export function pasteAttachmentName(timestamp: number): string {
  const iso = new Date(timestamp).toISOString();
  return `Pasted text_${iso.slice(0, 19).replace(/[:T]/g, "-")}.txt`;
}
