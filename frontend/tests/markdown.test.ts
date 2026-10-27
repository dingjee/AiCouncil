import { describe, expect, test } from "bun:test";

import {
  isSafeUrl,
  markdownToPlainText,
  renderMarkdown,
} from "../src/markdown/render";

describe("markdown safety", () => {
  test("does not execute raw HTML from the model", () => {
    const html = renderMarkdown('<img src=x onerror="alert(1)">');
    expect(html).not.toContain("<img src=x");
    expect(html).toContain("&lt;img");
  });

  test("escapes script tags", () => {
    const html = renderMarkdown("<script>alert(1)</script>");
    expect(html).not.toContain("<script>");
    expect(html).toContain("&lt;script&gt;");
  });

  test("rejects javascript: link targets", () => {
    const html = renderMarkdown("[click](javascript:alert(1))");
    expect(html).not.toContain('href="javascript:');
  });

  test("rejects data: image targets", () => {
    const html = renderMarkdown("![x](data:text/html;base64,PHNjcmlwdD4=)");
    expect(html).not.toContain('src="data:');
    expect(html).not.toContain("<img");
  });

  test("keeps ordinary links and marks them safe", () => {
    const html = renderMarkdown("[docs](https://example.com/page)");
    expect(html).toContain('href="https://example.com/page"');
    expect(html).toContain('rel="noopener noreferrer nofollow"');
  });

  test("isSafeUrl accepts relative and http(s) targets", () => {
    expect(isSafeUrl("https://example.com")).toBe(true);
    expect(isSafeUrl("http://example.com")).toBe(true);
    expect(isSafeUrl("mailto:a@b.c")).toBe(true);
    expect(isSafeUrl("/local/path")).toBe(true);
    expect(isSafeUrl("./relative")).toBe(true);
    expect(isSafeUrl("#anchor")).toBe(true);
    expect(isSafeUrl("javascript:alert(1)")).toBe(false);
    expect(isSafeUrl("vbscript:x")).toBe(false);
    expect(isSafeUrl("")).toBe(false);
  });
});

describe("markdown rendering", () => {
  test("renders headings, lists and code fences", () => {
    const html = renderMarkdown("# Title\n\n- one\n- two\n\n```ts\nconst a = 1;\n```");
    expect(html).toContain("<h1>Title</h1>");
    expect(html).toContain("<li>one</li>");
    expect(html).toContain("md-code");
    expect(html).toContain("hljs");
  });

  test("highlights code with the requested language", () => {
    const html = renderMarkdown("```python\ndef f():\n    return 1\n```");
    expect(html).toContain("language-python");
    expect(html).toContain("hljs-keyword");
  });

  test("renders inline and display math", () => {
    const inline = renderMarkdown("Value $E = mc^2$ here");
    expect(inline).toContain("md-math");
    expect(inline).toContain("katex");
    // The raw source with its delimiters is kept for the copy projection.
    expect(inline).toContain('data-math-raw="$E = mc^2$"');

    const display = renderMarkdown("$$\n\\frac{1}{2}\n$$");
    expect(display).toContain("md-math-display");
  });

  test("keeps an unclosed formula literal (streaming fallback)", () => {
    const html = renderMarkdown("$\\notacommand{");
    expect(html).not.toContain("md-math");
    expect(html).toContain("$\\notacommand{");
  });

  test("falls back to the source when a closed formula cannot be parsed", () => {
    const html = renderMarkdown("$\\undefinedmacro$");
    expect(html).toContain("math-fallback");
    expect(html).toContain("data-math-raw");
  });

  test("does not treat currency as math", () => {
    const html = renderMarkdown("It costs $20,000 and $30 total");
    expect(html).not.toContain("md-math");
    expect(html).toContain("$20,000");
  });

  test("keeps code spans free of math", () => {
    const html = renderMarkdown("`$x$`");
    expect(html).not.toContain("md-math");
  });

  test("supports CJK text", () => {
    const html = renderMarkdown("这是一个**加粗**的中文句子。");
    expect(html).toContain("这是一个");
    expect(html).toContain("<strong>加粗</strong>");
  });
});

describe("plain text projection", () => {
  test("keeps formulas as source with their delimiters", () => {
    const text = markdownToPlainText("Value $E = mc^2$ end");
    expect(text).toContain("$E = mc^2$");
  });

  test("keeps inline code content", () => {
    const text = markdownToPlainText("run `cargo test` now");
    expect(text).toContain("cargo test");
  });

  test("keeps list markers", () => {
    const text = markdownToPlainText("- one\n- two");
    expect(text).toContain("one");
    expect(text).toContain("two");
  });
});
