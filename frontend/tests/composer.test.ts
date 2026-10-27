import { describe, expect, test } from "bun:test";

import { composerKeyAction, IME_KEY_CODE } from "../src/util/keyboard";
import {
  canAddAttachment,
  LONG_PASTE_CHARS,
  MAX_ATTACHMENTS,
  MAX_ATTACHMENT_BYTES,
  MAX_TOTAL_BYTES,
  pasteAttachmentName,
  shouldConvertPaste,
} from "../src/util/attachments";

describe("composer keyboard contract", () => {
  test("Enter sends the message", () => {
    expect(composerKeyAction({ key: "Enter" })).toBe("send");
  });

  test("Shift+Enter inserts a newline", () => {
    expect(composerKeyAction({ key: "Enter", shiftKey: true })).toBe("newline");
  });

  test("Cmd+Enter sends the message", () => {
    expect(composerKeyAction({ key: "Enter", metaKey: true })).toBe("send");
    expect(composerKeyAction({ key: "Enter", ctrlKey: true })).toBe("send");
  });

  test("Enter during an IME composition never sends", () => {
    expect(composerKeyAction({ key: "Enter", isComposing: true })).toBe("ignore");
  });

  test("the legacy IME key code is honoured", () => {
    expect(composerKeyAction({ key: "Enter", keyCode: IME_KEY_CODE })).toBe("ignore");
    expect(composerKeyAction({ key: "Process", keyCode: IME_KEY_CODE })).toBe("ignore");
  });

  test("other keys are ignored", () => {
    expect(composerKeyAction({ key: "a" })).toBe("ignore");
    expect(composerKeyAction({ key: "Escape" })).toBe("ignore");
    expect(composerKeyAction({ key: "Tab" })).toBe("ignore");
  });

  test("a composition with Shift+Enter still does not send", () => {
    expect(
      composerKeyAction({ key: "Enter", shiftKey: true, isComposing: true }),
    ).toBe("ignore");
  });
});

describe("attachment limits", () => {
  const attachment = (size: number) => ({ name: "f.txt", content: "x".repeat(size) });
  const text = (size: number) => "x".repeat(size);

  test("accepts a normal attachment", () => {
    expect(canAddAttachment([], text(10)).ok).toBe(true);
  });

  test("rejects a file larger than 256 KiB", () => {
    const result = canAddAttachment([], text(MAX_ATTACHMENT_BYTES + 1));
    expect(result.ok).toBe(false);
    expect(result.reason).toBe("file_too_large");
    expect(canAddAttachment([], text(MAX_ATTACHMENT_BYTES)).ok).toBe(true);
  });

  test("rejects more than 10 attachments", () => {
    const existing = Array.from({ length: MAX_ATTACHMENTS }, () => attachment(1));
    const result = canAddAttachment(existing, text(1));
    expect(result.ok).toBe(false);
    expect(result.reason).toBe("too_many");
  });

  test("rejects more than 1 MiB in total", () => {
    const existing = [attachment(MAX_TOTAL_BYTES - 10)];
    const result = canAddAttachment(existing, text(20));
    expect(result.ok).toBe(false);
    expect(result.reason).toBe("total_too_large");
  });
});

describe("long paste conversion", () => {
  test("a short paste stays in the composer", () => {
    expect(shouldConvertPaste("a".repeat(LONG_PASTE_CHARS - 1))).toBe(false);
  });

  test("a long paste becomes an attachment", () => {
    expect(shouldConvertPaste("a".repeat(LONG_PASTE_CHARS))).toBe(true);
  });

  test("the attachment name is derived from the timestamp", () => {
    const name = pasteAttachmentName(Date.UTC(2026, 0, 2, 3, 4, 5));
    expect(name).toBe("Pasted text_2026-01-02-03-04-05.txt");
  });
});
