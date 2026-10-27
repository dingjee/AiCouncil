import { describe, expect, test } from "bun:test";

import { matchInlineDollar, normalizeTex } from "../src/markdown/render";

/**
 * The math delimiter rules are a port of `src/render/md.rs`:
 *   - an opening `$` must be followed by a non-whitespace, non-`$` character;
 *   - a closing `$` must be preceded by a non-whitespace character;
 *   - a closing `$` followed by a digit belongs to a currency amount;
 *   - inline math may not span a line break;
 *   - unclosed delimiters stay literal (the streaming fallback).
 */
describe("matchInlineDollar", () => {
  test("matches a simple formula", () => {
    const result = matchInlineDollar("$E = mc^2$ and more", 0);
    expect(result).not.toBeNull();
    expect(result?.tex).toBe("E = mc^2");
    expect(result?.end).toBe(10);
  });

  test("rejects an opening dollar followed by whitespace", () => {
    expect(matchInlineDollar("$ x$", 0)).toBeNull();
    expect(matchInlineDollar("$\tx$", 0)).toBeNull();
  });

  test("rejects an opening dollar followed by another dollar", () => {
    expect(matchInlineDollar("$$x$$", 0)).toBeNull();
  });

  test("rejects a closing dollar preceded by whitespace", () => {
    expect(matchInlineDollar("$x $", 0)).toBeNull();
  });

  test("treats a dollar before a digit as currency", () => {
    // "$20,000 and $30" must stay literal.
    expect(matchInlineDollar("$20,000 and $30", 0)).toBeNull();
    expect(matchInlineDollar("$5$6", 0)).toBeNull();
  });

  test("rejects a formula that spans a line break", () => {
    expect(matchInlineDollar("$a\nb$", 0)).toBeNull();
  });

  test("keeps an unclosed delimiter literal", () => {
    expect(matchInlineDollar("$x without close", 0)).toBeNull();
  });

  test("skips escaped characters inside the formula", () => {
    const result = matchInlineDollar("$a\\$b$", 0);
    expect(result?.tex).toBe("a\\$b");
  });

  test("rejects an empty formula", () => {
    expect(matchInlineDollar("$$", 0)).toBeNull();
  });

  test("does not treat CJK text as a delimiter problem", () => {
    const source = "价格是 $x^2$ 元";
    const result = matchInlineDollar(source, source.indexOf("$"));
    expect(result?.tex).toBe("x^2");
  });
});

describe("normalizeTex", () => {
  test("maps numbered environments to unnumbered equivalents", () => {
    expect(normalizeTex("\\begin{align}a\\end{align}")).toBe(
      "\\begin{aligned}a\\end{aligned}",
    );
    expect(normalizeTex("\\begin{align*}a\\end{align*}")).toBe(
      "\\begin{aligned}a\\end{aligned}",
    );
    expect(normalizeTex("\\begin{gather}a\\end{gather}")).toBe(
      "\\begin{gathered}a\\end{gathered}",
    );
    expect(normalizeTex("\\begin{gather*}a\\end{gather*}")).toBe(
      "\\begin{gathered}a\\end{gathered}",
    );
  });

  test("trims the source", () => {
    expect(normalizeTex("  x  ")).toBe("x");
  });
});
