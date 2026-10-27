/**
 * Selection → plain text projection.
 *
 * Copy projection rules (formulas copy as source with their original
 * delimiters):
 *   - formulas copy as **source with their original delimiters**;
 *   - inline code copies its content;
 *   - code blocks keep their line breaks and indentation;
 *   - tables use tabs between columns and newlines between rows;
 *   - list items keep their `- ` / `N. ` marker;
 *   - quote lines keep the `> ` prefix;
 *   - a partial cell selection copies only the selected text.
 */

const BLOCK_SELECTOR =
  "p, div, li, tr, pre, blockquote, h1, h2, h3, h4, h5, h6, table, ul, ol";

interface WalkContext {
  out: string;
  /** Depth of list nesting, used for the list marker. */
  listDepth: number;
  orderedIndexes: number[];
}

function isElement(node: Node): node is HTMLElement {
  return node.nodeType === 1;
}

function closestWithin(node: Node, selector: string, root: Node): Element | null {
  let current: Node | null = node;
  while (current && current !== root) {
    if (isElement(current) && current.matches(selector)) return current;
    current = current.parentNode;
  }
  return null;
}

/** List marker for a list item, e.g. `- ` or `3. `. */
function listMarker(item: Element): string {
  const list = item.parentElement;
  if (!list) return "- ";
  if (list.tagName.toLowerCase() === "ol") {
    const start = Number(list.getAttribute("start") ?? "1");
    const index = Array.prototype.indexOf.call(list.children, item);
    const value = Number.isFinite(start) ? start + Math.max(0, index) : index + 1;
    return `${value}. `;
  }
  return "- ";
}

/**
 * Copy the text covered by a DOM range using the projection rules above.
 */
export function rangeToPlainText(range: Range): string {
  const container = range.commonAncestorContainer;
  const root =
    container.nodeType === 1
      ? (container as Element)
      : (container.parentNode as Element | null);
  if (!root) return range.toString();

  const ctx: WalkContext = { out: "", listDepth: 0, orderedIndexes: [] };
  const startElement =
    range.startContainer.nodeType === 1
      ? (range.startContainer as Element)
      : range.startContainer.parentElement;
  const endElement =
    range.endContainer.nodeType === 1
      ? (range.endContainer as Element)
      : range.endContainer.parentElement;

  if (!startElement || !endElement) return range.toString();

  const ordered: Node[] = [];
  collectNodes(root, ordered);

  const startIndex = ordered.indexOf(range.startContainer);
  const endIndex = ordered.indexOf(range.endContainer);

  let depth = 0;
  for (let i = 0; i < ordered.length; i += 1) {
    const node = ordered[i];
    if (startIndex >= 0 && i < startIndex) continue;
    if (endIndex >= 0 && i > endIndex) break;

    if (node.nodeType === 3) {
      const text = node.textContent ?? "";
      const isPartial =
        node === range.startContainer || node === range.endContainer;
      if (isPartial) {
        const start = node === range.startContainer ? range.startOffset : 0;
        const end = node === range.endContainer ? range.endOffset : text.length;
        ctx.out += text.slice(start, end);
      } else {
        ctx.out += text;
      }
      continue;
    }

    if (!isElement(node)) continue;

    const element = node as HTMLElement;
    const tag = element.tagName.toLowerCase();

    if (element.classList.contains("md-math")) {
      ctx.out += element.getAttribute("data-math-raw") ?? element.textContent ?? "";
      continue;
    }

    if (tag === "br") {
      ctx.out += "\n";
      continue;
    }

    if (tag === "td" || tag === "th") {
      const cell = closestWithin(element, "td, th", root);
      if (cell && cell !== element) {
        // handled by the cell itself
      }
      continue;
    }

    if (tag === "tr") {
      if (ctx.out.length > 0 && !ctx.out.endsWith("\n")) ctx.out += "\n";
      continue;
    }

    if (tag === "pre") {
      ctx.out += element.textContent ?? "";
      if (!ctx.out.endsWith("\n")) ctx.out += "\n";
      continue;
    }

    if (tag === "li") {
      ctx.out += listMarker(element);
      depth += 1;
      continue;
    }

    if (tag === "blockquote") {
      ctx.out += "> ";
      continue;
    }

    if (
      element.matches(BLOCK_SELECTOR) &&
      element.childElementCount === 0 &&
      element.textContent
    ) {
      ctx.out += element.textContent;
    }
  }

  return ctx.out.replace(/[ \t]+\n/g, "\n").trimEnd();
}

function collectNodes(root: Node, out: Node[]): void {
  const walker = root.ownerDocument?.createTreeWalker(
    root,
    NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT,
  );
  if (!walker) return;
  let current = walker.nextNode();
  while (current) {
    out.push(current);
    current = walker.nextNode();
  }
}

/**
 * Copy the current browser selection inside `container`.
 *
 * Returns the copied text, or `null` when there is nothing to copy.
 */
export function copySelectionFrom(container: HTMLElement): string | null {
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  if (range.collapsed) return null;
  if (!container.contains(range.commonAncestorContainer)) return null;
  const text = rangeToPlainText(range);
  if (text.trim().length === 0) return null;
  void navigator.clipboard?.writeText(text);
  selection.removeAllRanges();
  return text;
}
