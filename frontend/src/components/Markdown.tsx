import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { copySelectionFrom } from "../markdown/copy";
import { renderMarkdown } from "../markdown/render";
import { Icon } from "./Icon";

export interface MarkdownProps {
  source: string;
  /** Accessible label of the rendered region. */
  label?: string;
  className?: string;
  /** True while the message is still streaming (cheap render profile). */
  streaming?: boolean;
}

/**
 * Renders model output as safe HTML.
 *
 * The HTML comes from the local renderer (`html: false`, safe URL allow
 * list), never from the model verbatim.
 */
export function Markdown({
  source,
  label,
  className,
  streaming,
}: MarkdownProps) {
  const html = useMemo(
    () => renderMarkdown(source ?? "", { streaming }),
    [source, streaming],
  );
  return (
    <div
      className={className ? `md ${className}` : "md"}
      dangerouslySetInnerHTML={{ __html: html }}
      aria-label={label}
    />
  );
}

export interface SelectableMarkdownProps extends MarkdownProps {
  messageId: string;
  onCopied?: (text: string) => void;
}

/**
 * Markdown with in-message selection and a copy affordance.
 *
 * The copy projection keeps formulas as source (with their original
 * delimiters) and inline code as plain text.
 */
export function SelectableMarkdown({
  messageId,
  source,
  label,
  onCopied,
  streaming,
}: SelectableMarkdownProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [selectionRect, setSelectionRect] = useState<{
    top: number;
    left: number;
  } | null>(null);
  const [copied, setCopied] = useState(false);

  const html = useMemo(
    () => renderMarkdown(source ?? "", { streaming }),
    [source, streaming],
  );

  const updateSelection = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    const selection = window.getSelection();
    if (!selection || selection.rangeCount === 0 || selection.isCollapsed) {
      setSelectionRect(null);
      return;
    }
    const range = selection.getRangeAt(0);
    if (!container.contains(range.commonAncestorContainer)) {
      setSelectionRect(null);
      return;
    }
    const rect = range.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) {
      setSelectionRect(null);
      return;
    }
    const host = container.getBoundingClientRect();
    setSelectionRect({
      top: rect.top - host.top + rect.height + 6,
      left: Math.max(0, rect.left - host.left + rect.width / 2),
    });
  }, []);

  useEffect(() => {
    document.addEventListener("selectionchange", updateSelection);
    return () => document.removeEventListener("selectionchange", updateSelection);
  }, [updateSelection]);

  const handleCopy = useCallback(() => {
    const container = containerRef.current;
    if (!container) return;
    const text = copySelectionFrom(container);
    if (!text) return;
    setCopied(true);
    onCopied?.(text);
    window.setTimeout(() => setCopied(false), 1500);
  }, [onCopied]);

  return (
    <div className="selectable-markdown" style={{ position: "relative" }}>
      <div
        ref={containerRef}
        className="md"
        data-message-id={messageId}
        dangerouslySetInnerHTML={{ __html: html }}
        aria-label={label}
      />
      {selectionRect ? (
        <button
          type="button"
          className="btn selection-copy"
          style={{
            position: "absolute",
            top: selectionRect.top,
            left: selectionRect.left,
            transform: "translateX(-50%)",
            background: "var(--surface)",
            border: "1px solid var(--line-interactive)",
            boxShadow: "0 4px 12px rgb(0 0 0 / 12%)",
            zIndex: 5,
          }}
          onClick={handleCopy}
          aria-label="Copy selected text"
        >
          <Icon name="copy" size={12} />
          {copied ? "Copied" : "Copy"}
        </button>
      ) : null}
    </div>
  );
}
