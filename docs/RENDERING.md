# Rendering Conclusions (P0)

## 0.1.5: Home Page Line Spacing, Table, and Horizontal Rule Regression Fixes (2026-10-19)

The root cause was in the native layout coordinates, not Markdown parsing: `MathFlow::layout` applied the row's vertical offset on both the line node and the text child node. iced's `Layout::children` accumulates parent coordinates, so line n was actually drawn at twice the offset, while the paragraph reported only a single offset of height. The result was inflated intra-paragraph line spacing, paragraph tails overrunning and covering the next paragraph, and horizontal rules cutting through text; selection hit testing still used the normal line coordinates, so it too mismatched the display.

After the fix, child nodes store only intra-line local coordinates while the line node handles vertical positioning; text measurement explicitly uses the block's font size and line height. Headings apply their level's font size when generating flow layout data, and text and formulas use the same font size.

Tables had a separate issue: each cell container shrank to fit its own content, exposing the grid background color beneath short cells and producing large gray patches and broken vertical lines. Now the content layer decides the full row height, and the background layer beneath fills the entire row with the same column widths, preserving a continuous 1px table grid. Center/right alignment is applied on each row's atom coordinates, and drawing, hit testing, and highlighting share those coordinates; empty cells keep one line of height, and the header is measured with bold metrics. English text is split into words on whitespace, allowing normal line wrapping.

Regression entry points:

```powershell
cargo test --release --lib --test render_layout --test math_coverage
cargo run --release --example render_probe -- --spacing
```

You can also generate whole-document PNGs directly with the tiny-skia renderer the app uses, without operating a window:

```powershell
$env:AICOUNCIL_RENDER_CAPTURE_DIR = "$env:TEMP\aicouncil-render"
cargo test --release --test render_layout
```

This option outputs 9 actual `view_message` renders (3 font sizes × 3 widths, 1.5× pixel ratio), sharing the same layout as the cell and text coordinate assertions.

`tests/fixtures/render_spacing.md` reproduces the defect scenario with a three-column table, and also contains an empty table row, long paragraphs, bold text, center/right alignment, two horizontal rules, headings, hard line breaks, and mixed formulas. The layout regression covers 12/15/22px body text and 360/760/1100px message widths; paragraph unit tests additionally cover narrower widths and check that actual draw coordinates, line box extents, and hit positions agree.

Date: 2026-10-17 · Test machine: Windows 10 19045 x64 · Rust 1.99.1 (x86_64-pc-windows-msvc)

## Actual Dependency Versions

| Dependency | Version | Purpose |
| --- | --- | --- |
| iced | 0.14.0 (features: markdown, highlighter, svg, tokio, advanced, wgpu, tiny-skia, thread-pool) | UI framework |
| iced_math | 0.5.0 | LaTeX → SVG (pure Rust, bundled Latin Modern Math) |
| iced_highlighter | 0.14.0 | Code block syntax highlighting (same engine as the iced markdown component) |
| pulldown-cmark | 0.12.2 | Markdown parsing (delimiter recognition is done in the preprocessing layer) |

The lockfile is committed (Cargo.lock).

## Architecture

1. **Parsing layer `render::md`**: before pulldown-cmark runs, a **placeholder preprocessing** pass replaces `$$…$$`, `\[…\]`, `$…$`, `\(…\)`, and `\$` with Private Use Area characters (U+E000..U+E002) as placeholders, then parses; the event layer restores them into `Part::Math` / `Part::DisplayMath`. Reason: pulldown splits escapes such as `\[` and `\,` into multiple Text events and unescapes them, so delimiters cannot be recognized directly on event text.
   - Fenced code blocks are protected as whole blocks; inline code spans (equal-length backtick pairs) are protected; indented code blocks restore placeholders to the original source at the event layer.
   - Currency rules (KaTeX/pandoc compatible): an opening `$` must be followed by non-whitespace; a closing `$` must be preceded by non-whitespace and not followed by a digit; line breaks are not allowed inside inline math; unclosed delimiters stay literal (streaming-output fallback, R2).
   - The environment names `align`/`align*`/`gather`/`gather*` map to `aligned`/`gathered` (iced_math does not support numbered environments; numbering was never in first-release scope, and the rendering semantics are identical).
2. **Formula rendering `render::math`**: the iced_math `MathRenderer` outputs SVG; intrinsic size is parsed from the SVG header (vertical center = math axis). Cache key = (normalized source, font size bits, display mode, color bits), deduplicated across messages. Parse failures fall back to the red monospace source text; no panic.
3. **View layer `render::view`**: paragraphs without formulas take the `rich_text` fast path (full bold/italic/strikethrough/link/inline code styling); code blocks use iced_highlighter highlighting + horizontal scrolling + rounded background; tables use grid; display formulas are centered and scroll horizontally when overly wide.
4. **Mixed-content component `render::flow` (MathFlow)**: iced has no layout for "inline components embedded in text", so a custom greedy line-breaking flow typographer was built:
   - Atoms = Latin words / single CJK characters / inline formulas (indivisible);
   - Text atoms are measured and drawn with the renderer's cosmic-text Paragraph (same stack as the iced text component, identical CJK system font fallback);
   - Formula SVG vertical centers align to the math axis, baseline = axis + 0.25em; the text baseline is approximated as ascent ≈ 0.8× line height (cosmic-text does not expose the baseline);
   - Measurement cache key = (font style, text); intrinsic width is independent of container width, and re-measurement happens only when the content version changes; the layout node is the line box, and draw places it directly from the Layout.

## Verification Results

- `cargo test --lib`: 27 parsing tests pass (four delimiter classes, currency protection, code protection, block-level splitting, tables/lists/quotes, streaming unclosed fallback).
- `cargo test --test math_coverage`: all R1 formula categories (fractions, roots, integrals, series, matrices, cases, aligned, align mapping, set brackets) render as SVG graphics without fallback; the known `\LaTeX` macro falls back to source text (documented).
- Real window (`cargo run --example render_probe`, 1100×800, Light):
  - Mixed Chinese/English text renders crisply, no garbled glyphs/tofu (cosmic-text system font fallback);
  - Inline formulas render as graphics with baselines aligned well to the text;
  - Matrix/cases/integral/fraction display formulas render as graphics;
  - Rust code block syntax highlighting + rounded background work correctly;
  - The 20,000-character + 50-formula stress sample pages and scrolls normally (PGDN×20 without freezing);
  - Typing in the input box works normally while the 30ms streaming tick runs (visual check of the typed input "liushiceshi 123" and the send button);
  - Long paragraphs wrap automatically, with no horizontal overflow and no content squeezing out the input box.
  - Verified visually in a real window (full page + zoomed detail captures were taken locally during acceptance and are not kept in the repo).

## Known Gaps (not blocking the P0 exit; recorded as TODOs)

> **2026-10-19 update**: item 1 (partial body-text selection) landed in this round; see the "Selectable Document and Copy Projection (2026-10-19)" section below. The remaining gaps still stand.

1. ~~**Arbitrary text selection/copy**~~ → implemented this round via the `render::document` selection pipeline (drag selection across blocks within a single message / double-click word selection / triple-click whole-block selection / formula atoms copied as whole units). Continuous drag selection across messages is not yet supported (see the new gaps below).
2. Individual text macros such as `\LaTeX` fall back to source text (outside iced_math's Tier-1 set of roughly 60 commands).
3. Strikethrough inside MathFlow paragraphs is not rendered yet (a rare combination; the rich_text fast path fully supports it).
4. HTML entities (e.g. `&amp;`) display literally in paragraphs containing formulas (pulldown decoding on the quick path is unaffected).
5. The baseline is a 0.8 approximation; mixed content with extreme fonts (very high ascent) may show ±1px deviation.

## Selectable Document and Copy Projection (2026-10-19)

This round landed the document rendering foundation per section 4 of `UI_FOUNDATION_PLAN.md`:

### New Architecture

```
Raw Markdown (never rewritten)
  → render::md (Strict / LlmCompatible dual policy, placeholder preprocessing)
  → Document IR (Block/Inlines/Part; formulas carry raw source + tex-rendered text)
  → render::flow (MathFlow unified component for all paragraphs; CodeFlow for code blocks)
  → render::document (FlatDoc leaf flattening + PlainCopyProjection)
  → display drawing + selection highlight (render::view)
```

- **Unified paragraph component MathFlow**: paragraphs/headings/table cells all render and select through the same component (replacing the old rich_text fast path + MathFlow dual track); the block path (`PathSeg`) replaces the old `paras + para_idx` ordinal association, so heading formulas are no longer misattributed to body text. Heading formulas render at their level's font size.
- **CodeFlow**: code blocks select at (line, col) granularity with monospace character-width conversion; highlight spans are generated by iced_highlighter.
- **Copy projection (T12 pure function tests)**: bold/links take the decoded text, inline code takes its content, formulas return the **raw source including the original delimiters** (all four forms `$…$`/`\(…\)`/`$$…$$`/`\[…\]` verbatim), blank lines between paragraphs, code preserving line breaks and indentation, tables with tabs between columns and newlines between rows, list items carrying the `- `/`N. ` marker, quote lines carrying the `> ` prefix; a partial cell selection copies only the selected text. With no body selection, the clipboard is not cleared.
- **Streaming selection stability (T14)**: selections are anchored by block path + grapheme, so appends at the end do not change the projection of the already-selected prefix; block re-parsing is driven by a content version number, and selections persist with their IR semantic positions.

### LlmCompatible Parsing (T09)

- `ParsePolicy::Strict` = the CommonMark baseline (default for user-authored text); `ParsePolicy::LlmCompatible` is enabled only for assistant body text.
- The single relaxation: `**…**` that remains literal after parsing constructs an emphasis node when it is **within the same Text part** and the opening/closing delimiters sit directly against Chinese punctuation/quote combinations (e.g. `借用**"X"**的机制`); adjacent same-style parts are merged first and then paired, with a bounded scan per character — no global regex.
- All counterexamples stay strict: multiplication `3 * 4`, single asterisks, snake_case, `##标题`, `#hashtag`, unclosed `**`, and no pairing across parts/inside code/inside links; in ambiguous scenarios Strict and LlmCompatible produce identical output (differential testing).

### Tables (4.4 / T11)

- Empty cells are no longer lost (the old code only pushed at `TagEnd::TableCell` when non-empty, shifting subsequent columns left); rows anomalously missing columns are padded with empty cells to the header column count.
- Row height = content height + 6px top/bottom padding (eliminating the "huge three-column gaps" caused by iced Grid's default `AspectRatio(1.0)` square cells); column margin 10px.
- The 1px `line_subtle` hairlines are drawn in one pass through background-colored gaps (1px gaps between rows + 1px outer padding, no double-dark lines); the header gets a light background (code_surface) and bold text.
- Alignment applies to the text itself (Center hugs the content in a centered container / Right right-aligns).

### Type Scale (3.2)

- Heading scale factors are interpolated along the Fibonacci limit of φ: H1=φ²≈2.618 / H2=φ^1.5≈2.058 / H3=φ≈1.618 / H4=√φ / H5=φ^0.25 / H6=1 (versus the old 2/1.7/1.45/1.25/1.1/1, the top-level difference goes 18% → 27%). H1–H4 are bold, H6 uses the secondary color; heading line height 1.22, body 1.55; block spacing is computed by rules (0.9× font size before headings / 0.35× font size after, 0.55b between paragraphs, 0.65b before and after tables), replacing the old fixed spacing(8).
- Body font size is adjustable 12–22 (Settings slider, persisted as `general.body_text_size`); render and app share `ui_style.rs` (render → app dependencies are forbidden).

### New Known Gaps This Round

1. **Cross-message drag selection**: selection currently stays within a single message (message cards are the selection boundary); connecting selections across messages in reading order is future work (G0-S global coordinator).
2. **Collapse decisions** use a character/line heuristic (`should_collapse`) instead of measuring actual layout heights; with extreme narrow-card line-wrap density differences, collapsing may fire early/late.
3. **Selection geometry** is an approximation in table horizontal scrolling and Reading anchor restoration (offset relative to the bottom rather than node-level anchors); scroll position restoration error can reach line level.
4. The `Ctrl+C` editor-focus decision depends on the `focusable::is_focused` operation and takes effect one async frame later; copying inside the input box is always handled by the editor itself and is not preempted by the global shortcut.

## Conclusion

The P0 exit criteria are met: ordinary Markdown and common formulas are readable, Chinese has no missing glyphs, 20,000 characters + 50 formulas scroll, and streaming appends do not freeze input. Proceeding to P1.
