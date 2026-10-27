/**
 * Markdown rendering with math, code highlighting and safe HTML.
 *
 * Safety model
 * ------------
 * Model output is untrusted. The renderer therefore:
 *   - disables raw HTML in markdown-it (`html: false`), so any tag in the
 *     answer is escaped and shown as text instead of being executed;
 *   - rejects link/image targets that are not http(s)/mailto/relative, so
 *     `javascript:` and `data:` URLs can never be activated;
 *   - marks external links `rel="noopener noreferrer nofollow"`.
 *
 * Math
 * ----
 * Delimiter handling follows the Rust renderer (`src/render/md.rs`):
 *   - `$$…$$` and `\[…\]` are display math, `$…$` and `\(…\)` are inline;
 *   - an opening `$` must be followed by a non-whitespace, non-`$` character;
 *     a closing `$` must be preceded by a non-whitespace character and must
 *     not be followed by a digit (so `$20,000 and $30` stays literal);
 *   - inline math may not span a line break; unclosed delimiters stay
 *     literal, which is what streaming output produces most of the time;
 *   - `\$` is a literal dollar sign;
 *   - code spans and fenced blocks are protected: delimiters inside them are
 *     never treated as math.
 *
 * Streaming
 * ---------
 * A streamed answer arrives in many small deltas. Re-running KaTeX and the
 * syntax highlighter for every delta is expensive enough to starve the
 * webview's JS thread (which also stops timers and pending IPC promises —
 * the native harness stalled exactly there). The renderer therefore has two
 * profiles:
 *   - `full`    — KaTeX + highlighting (finished messages);
 *   - `light`  — formulas and code are shown as source, everything else is
 *     identical (used while a message is still streaming).
 * The full profile is applied once, when the message completes.
 */

import MarkdownIt from "markdown-it";
import type { StateBlock, StateInline, Token } from "markdown-it";
import katex from "katex";
import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import c from "highlight.js/lib/languages/c";
import cpp from "highlight.js/lib/languages/cpp";
import css from "highlight.js/lib/languages/css";
import go from "highlight.js/lib/languages/go";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import markdown from "highlight.js/lib/languages/markdown";
import plaintext from "highlight.js/lib/languages/plaintext";
import python from "highlight.js/lib/languages/python";
import rust from "highlight.js/lib/languages/rust";
import sql from "highlight.js/lib/languages/sql";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

/**
 * Only the languages the app actually highlights are registered: the
 * highlighter is bundled locally (never fetched at runtime) and stays small.
 */
const LANGUAGES: Record<string, Parameters<typeof hljs.registerLanguage>[1]> = {
  bash,
  c,
  cpp,
  css,
  go,
  java,
  javascript,
  json,
  markdown,
  plaintext,
  python,
  rust,
  sql,
  typescript,
  xml,
  yaml,
};
for (const [name, definition] of Object.entries(LANGUAGES)) {
  hljs.registerLanguage(name, definition);
}
hljs.registerAliases(["ts", "tsx"], { languageName: "typescript" });
hljs.registerAliases(["js", "jsx", "mjs"], { languageName: "javascript" });
hljs.registerAliases(["sh", "shell", "zsh"], { languageName: "bash" });
hljs.registerAliases(["py"], { languageName: "python" });
hljs.registerAliases(["rs"], { languageName: "rust" });
hljs.registerAliases(["yml"], { languageName: "yaml" });
hljs.registerAliases(["html"], { languageName: "xml" });

export interface MathRenderResult {
  html: string;
  /** True when the formula could not be parsed and the source is shown. */
  fallback: boolean;
}

/** Environment names the Rust renderer maps to unnumbered equivalents. */
export function normalizeTex(tex: string): string {
  return tex
    .trim()
    .replace(/\\begin\{align\}/g, "\\begin{aligned}")
    .replace(/\\begin\{align\*\}/g, "\\begin{aligned}")
    .replace(/\\end\{align\}/g, "\\end{aligned}")
    .replace(/\\end\{align\*\}/g, "\\end{aligned}")
    .replace(/\\begin\{gather\}/g, "\\begin{gathered}")
    .replace(/\\begin\{gather\*\}/g, "\\begin{gathered}")
    .replace(/\\end\{gather\}/g, "\\end{gathered}")
    .replace(/\\end\{gather\*\}/g, "\\end{gathered}");
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Render TeX to HTML; on failure fall back to the raw source. */
export function renderMath(tex: string, displayMode: boolean): MathRenderResult {
  const source = normalizeTex(tex);
  try {
    const html = katex.renderToString(source, {
      displayMode,
      throwOnError: true,
      strict: "ignore",
      output: "html",
      trust: false,
      maxSize: 64,
      maxExpand: 512,
    });
    return { html, fallback: false };
  } catch {
    const raw = displayMode ? `$$${tex}$$` : `$${tex}$`;
    return {
      html: `<span class="math-fallback" role="img" aria-label="unrendered formula">${escapeHtml(raw)}</span>`,
      fallback: true,
    };
  }
}

const SAFE_URL_RE = /^(?:https?:|mailto:|tel:|#|\/|\.\/|\.\.\/)/i;

export function isSafeUrl(url: string): boolean {
  const trimmed = url.trim();
  if (trimmed.length === 0) return false;
  // Reject control characters used to smuggle "java\nscript:".
  if (/[ -]/.test(trimmed)) return false;
  if (trimmed.startsWith("#") || trimmed.startsWith("/")) return true;
  if (/^[a-z][a-z0-9+.-]*:/i.test(trimmed)) {
    return SAFE_URL_RE.test(trimmed);
  }
  // Relative URL without a scheme.
  return !trimmed.startsWith(":");
}

function nextIndex(src: string, index: number): number {
  const code = src.charCodeAt(index);
  if (code >= 0xd800 && code <= 0xdbff && index + 1 < src.length) {
    const next = src.charCodeAt(index + 1);
    if (next >= 0xdc00 && next <= 0xdfff) return index + 2;
  }
  return index + 1;
}

function isAsciiWhitespace(code: number): boolean {
  return (
    code === 0x20 ||
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d
  );
}

function isDigit(code: number): boolean {
  return code >= 0x30 && code <= 0x39;
}

/**
 * Match an inline `$…$` run.
 *
 * Returns the index just after the closing delimiter, or `null` when the
 * delimiter does not open a formula (currency, whitespace, or unclosed).
 */
export function matchInlineDollar(
  src: string,
  open: number,
): { end: number; tex: string } | null {
  if (open + 1 >= src.length) return null;
  const first = src.charCodeAt(open + 1);
  if (isAsciiWhitespace(first) || first === 0x24 /* $ */) return null;

  let j = open + 1;
  while (j < src.length) {
    const code = src.charCodeAt(j);
    if (code === 0x0a) return null; // no line break inside inline math
    if (code === 0x24 /* $ */) {
      const prev = src.charCodeAt(j - 1);
      if (isAsciiWhitespace(prev)) return null;
      const next = j + 1 < src.length ? src.charCodeAt(j + 1) : -1;
      if (isDigit(next)) {
        // "$20,000 and $30" — the dollar belongs to a currency amount.
        j = nextIndex(src, j);
        continue;
      }
      const tex = src.slice(open + 1, j);
      if (tex.trim().length === 0) return null;
      return { end: j + 1, tex };
    }
    if (code === 0x5c /* \ */) {
      j = nextIndex(src, j);
      if (j < src.length) j = nextIndex(src, j);
      continue;
    }
    j = nextIndex(src, j);
  }
  return null;
}

/** Find a closing delimiter, honouring backslash escapes. */
function findClosing(src: string, from: number, needle: string): number {
  let i = from;
  while (i < src.length) {
    if (src.startsWith(needle, i)) {
      // A backslash before the needle escapes it.
      let backslashes = 0;
      let k = i - 1;
      while (k >= 0 && src[k] === "\\") {
        backslashes += 1;
        k -= 1;
      }
      if (backslashes % 2 === 0) return i;
    }
    i = nextIndex(src, i);
  }
  return -1;
}

export interface RenderProfile {
  /** Render formulas with KaTeX. */
  math: boolean;
  /** Highlight fenced code. */
  highlight: boolean;
}

/** Build a markdown-it instance for one render profile. */
function createRenderer(profile: RenderProfile): MarkdownIt {
  const md = MarkdownIt({
    html: false,
    linkify: true,
    breaks: false,
    typographer: false,
    highlight: (code: string, language: string): string => {
      if (!profile.highlight) {
        return `<pre class="md-code"><code class="md-plain">${escapeHtml(code)}</code></pre>`;
      }
      const lang = language && hljs.getLanguage(language) ? language : "";
      try {
        const result =
          lang === ""
            ? hljs.highlightAuto(code)
            : hljs.highlight(code, { language: lang, ignoreIllegals: true });
        const classes = lang ? `hljs language-${escapeHtml(lang)}` : "hljs";
        return `<pre class="md-code"><code class="${classes}">${result.value}</code></pre>`;
      } catch {
        return `<pre class="md-code"><code class="hljs">${escapeHtml(code)}</code></pre>`;
      }
    },
  });

  // ----- markdown-it overrides for safe links / images -----

  const defaultLinkOpen =
    md.renderer.rules.link_open ??
    ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));

  md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
    const token = tokens[idx];
    const href = token.attrGet("href") ?? "";
    if (!isSafeUrl(href)) {
      token.attrSet("href", "#");
      token.attrSet("aria-disabled", "true");
    } else {
      token.attrSet("rel", "noopener noreferrer nofollow");
      token.attrSet("target", "_blank");
    }
    return defaultLinkOpen(tokens, idx, options, env, self);
  };

  const defaultImage =
    md.renderer.rules.image ??
    ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));

  md.renderer.rules.image = (tokens, idx, options, env, self) => {
    const token = tokens[idx];
    const src = token.attrGet("src") ?? "";
    if (!isSafeUrl(src)) {
      // Drop the image entirely rather than emitting an unsafe URL.
      return "";
    }
    token.attrSet("loading", "lazy");
    token.attrSet("referrerpolicy", "no-referrer");
    return defaultImage(tokens, idx, options, env, self);
  };

  // ----- math rules -----

  function mathInline(state: StateInline, silent: boolean): boolean {
    const src = state.src;
    const start = state.pos;

    if (src[start] === "\\") {
      const next = src[start + 1];
      if (next === "(") {
        const close = findClosing(src, start + 2, "\\)");
        if (close < 0) return false;
        const tex = src.slice(start + 2, close);
        if (tex.includes("\n")) return false;
        if (!silent) {
          state.pushPending();
          const token = state.push("math_inline", "span", 0);
          token.content = tex;
          token.markup = "\\(";
          token.meta = { raw: `\\(${tex}\\)` };
        }
        state.pos = close + 2;
        return true;
      }
      if (next === "[") {
        const close = findClosing(src, start + 2, "\\]");
        if (close < 0) return false;
        const tex = src.slice(start + 2, close);
        if (!silent) {
          state.pushPending();
          const token = state.push("math_inline", "span", 0);
          token.content = tex;
          token.markup = "\\[";
          token.meta = { raw: `\\[${tex}\\]`, display: true };
        }
        state.pos = close + 2;
        return true;
      }
      return false;
    }

    if (src[start] !== "$") return false;
    // `$$` is handled by the block rule; not inline.
    if (src[start + 1] === "$") return false;

    const matched = matchInlineDollar(src, start);
    if (!matched) return false;
    if (!silent) {
      state.pushPending();
      const token = state.push("math_inline", "span", 0);
      token.content = matched.tex;
      token.markup = "$";
      token.meta = { raw: `$${matched.tex}$` };
    }
    state.pos = matched.end;
    return true;
  }

  function mathBlock(
    state: StateBlock,
    startLine: number,
    endLine: number,
    silent: boolean,
  ): boolean {
    const start = state.bMarks[startLine] + state.tShift[startLine];
    const max = state.eMarks[startLine];
    const line = state.src.slice(start, max);

    let openLength = 0;

    if (line.startsWith("$$")) {
      openLength = 2;
    } else if (line.startsWith("\\[")) {
      openLength = 2;
    } else {
      return false;
    }

    // Single-line form: `$$ x $$`
    const rest = line.slice(openLength);
    const singleClose = rest.indexOf("$$");
    if (singleClose >= 0) {
      const tex = rest.slice(0, singleClose);
      if (tex.trim().length === 0) return false;
      if (!silent) {
        const token = state.push("math_block", "div", 0);
        token.content = tex;
        token.markup = line.slice(0, 2);
        token.meta = { raw: `$$${tex}$$` };
        token.block = true;
      }
      state.line = startLine + 1;
      return true;
    }

    // Multi-line form: search for the closing delimiter on later lines.
    let content = rest;
    let closed = -1;
    for (let i = startLine + 1; i < endLine; i += 1) {
      const lineStart = state.bMarks[i] + state.tShift[i];
      const lineEnd = state.eMarks[i];
      const candidate = state.src.slice(lineStart, lineEnd);
      const at = candidate.indexOf("$$");
      if (at >= 0) {
        content += "\n" + candidate.slice(0, at);
        closed = i;
        state.line = i + 1;
        break;
      }
      content += "\n" + candidate;
    }
    if (closed < 0) {
      // Unclosed: keep the source literal (streaming fallback).
      return false;
    }
    if (content.trim().length === 0) return false;
    if (!silent) {
      const token = state.push("math_block", "div", 0);
      token.content = content;
      token.markup = "$$";
      token.meta = { raw: `$$${content}$$` };
      token.block = true;
    }
    return true;
  }

  md.inline.ruler.before("escape", "math_inline", mathInline);
  md.block.ruler.before("fence", "math_block", mathBlock, {
    alt: ["paragraph", "reference", "blockquote", "list"],
  });

  const renderMathToken = (token: Token, display: boolean): string => {
    const raw = String(token.meta?.raw ?? `$${token.content}$`);
    if (!profile.math) {
      // Streaming profile: show the source, no KaTeX work.
      return `<span class="md-math md-math-plain${display ? " md-math-display" : ""}" data-math-raw="${escapeHtml(raw)}">${escapeHtml(raw)}</span>`;
    }
    const { html } = renderMath(token.content, display);
    return `<span class="md-math${display ? " md-math-display" : ""}" data-math-raw="${escapeHtml(raw)}">${html}</span>`;
  };

  md.renderer.rules.math_inline = (tokens, idx) =>
    renderMathToken(tokens[idx], Boolean(tokens[idx].meta?.display));
  md.renderer.rules.math_block = (tokens, idx) =>
    renderMathToken(tokens[idx], true);

  return md;
}

const MD = createRenderer({ math: true, highlight: true });
const MD_STREAMING = createRenderer({ math: false, highlight: false });

export type MarkdownRenderer = MarkdownIt;

export function createMarkdownRenderer(): MarkdownIt {
  return MD;
}

export interface RenderOptions {
  /** True while the message is still streaming (cheap profile). */
  streaming?: boolean;
}

/** Render markdown source to safe HTML. */
export function renderMarkdown(source: string, options?: RenderOptions): string {
  const md = options?.streaming ? MD_STREAMING : MD;
  return md.render(source ?? "");
}

/**
 * Render markdown source for **plain text** projection (copy to clipboard).
 *
 * Formulas keep their original delimiters and source, inline code keeps its
 * content, link text is preserved, and list/quote markers are kept, matching
 * the Rust copy projection.
 */
export function markdownToPlainText(source: string): string {
  const tokens = MD.parse(source ?? "", {});
  const parts: string[] = [];
  for (const token of tokens) {
    switch (token.type) {
      case "math_block":
        parts.push(String(token.meta?.raw ?? `$$${token.content}$$`));
        break;
      case "paragraph_open":
        parts.push("");
        break;
      case "paragraph_close":
        parts.push("\n\n");
        break;
      case "bullet_list_open":
      case "ordered_list_open":
        parts.push("");
        break;
      case "list_item_open":
        parts.push("");
        break;
      case "fence":
      case "code_block":
        parts.push(token.content.endsWith("\n") ? token.content : token.content + "\n");
        break;
      case "inline":
        parts.push(inlineToPlainText(token.children ?? [], token.content));
        break;
      default:
        break;
    }
  }
  return parts.join("").replace(/\n{3,}/g, "\n\n").trimEnd();
}

function inlineToPlainText(children: Token[], fallback: string): string {
  if (children.length === 0) return fallback;
  let out = "";
  for (const child of children) {
    switch (child.type) {
      case "math_inline":
        out += String(child.meta?.raw ?? `$${child.content}$`);
        break;
      case "text":
        out += child.content;
        break;
      case "code_inline":
        out += child.content;
        break;
      case "softbreak":
      case "hardbreak":
        out += "\n";
        break;
      case "image":
        out += child.content;
        break;
      default:
        if (child.children && child.children.length > 0) {
          out += inlineToPlainText(child.children, child.content);
        } else {
          out += child.content;
        }
        break;
    }
  }
  return out;
}
