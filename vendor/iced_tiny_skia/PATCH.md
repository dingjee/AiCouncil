# iced_tiny_skia local patch

This is the upstream `iced_tiny_skia` 0.14.1 crate (MIT, `iced-rs/iced`) with local changes in `src/engine.rs`, `src/text.rs`, and the diagnostic exports in `src/lib.rs`.

## Editor glyph clipping

Cosmic Text lays out the **whole** buffer, including lines that are scrolled out of the editor viewport, so `draw_editor` receives glyph runs that reach far below the visible area. Upstream clipped those glyphs to the widget's *content area* (`clip_bounds`, i.e. the editor bounds minus padding) — or not at all, when the content area happened to contain the editor bounds.

Two problems followed:

1. Glyphs from scrolled-out lines painted into the editor padding and below the widget.
2. The renderer's damage region for an editor is `editor.bounds` (the editor *viewport*, see `iced_graphics::text::Text::visible_bounds`), which is smaller than the content area. A partial redraw therefore only repainted the viewport while the escaped pixels stayed on screen, so a retained (damage-driven) frame no longer matched a full redraw of the same tree.

The patch clips editor glyphs to the editor viewport instead:

```rust
let Some(viewport) = physical_bounds.intersection(&clip_bounds) else { return; };
adjust_clip_mask(clip_mask, viewport);
let clip_mask = Some(clip_mask as &_);
```

`physical_bounds` is `Rectangle::new(position, editor.bounds) * transformation` — the same rectangle the damage calculation reports — so the painted pixels and the repainted region now agree by construction.

## Verification

`tests/editor_render.rs` compares a retained tiny-skia damage update against a fresh full render after scrolling CJK editor text at 1.5× DPI, over several editor heights and scroll steps.

- Upstream `iced_tiny_skia` 0.14.1: **fails** — 9,121 differing pixels at editor height 44, scroll step 0.
- With this patch: **passes** — 0 differing pixels, and no glyph ink below the text viewport.

## Normal text coverage

`src/text.rs` applies a cached exponent-1.6 tone curve to grayscale coverage for
font weights below 600. The font weight is already part of Cosmic Text's glyph
cache key. Endpoints 0 and 255, glyph placement, semibold/bold, and color glyphs
remain unchanged. The lookup avoids a floating-point power per glyph pixel.
This is a visual compensation for lighter reading text, not an exact native
text-renderer model. `src/lib.rs` exposes the immutable coverage function and
constants for the offline probe and regression checks.

Verification: `tests/text_raster.rs` checks real CJK masks at six size/DPI
combinations, unchanged bold/color glyphs, and the actual production backend's
ink coverage. `examples/text_raster_probe.rs` generates before/after captures.

## Whitespace glyph cache

`GlyphCache::allocate` returned early for glyphs without ink — whitespace
mainly — **without inserting the cache entry**. The key therefore never became
known to the cache, and every frame re-ran the scaler for every space on
screen. A Chinese paragraph of ~40 characters contains dozens of spaces (and
the same happens for any Latin word boundary), so a steady frame kept paying
the rasterizer for glyphs that cannot produce any ink.

Measured on a 20,000-character Chinese document (860×900 viewport): the glyph
cache missed 85 glyphs **per frame**, and re-rasterizing them cost ~20 ms of
the ~25 ms frame budget.

The patch caches the empty result instead:

```rust
if glyph_size == 0 {
    let _ = entry.insert((Vec::new(), EMPTY_PLACEMENT));
    return None;
}
```

and the lookup treats an empty buffer as "nothing to draw". A missing outline
(`get_image_uncached` → `None`) is cached the same way.

`src/lib.rs` exports `GLYPH_CACHE_MISSES`, a counter of scaler invocations,
so the regression can be asserted directly.

Verification: `tests/render_regressions.rs`
(`steady_frame_does_not_rasterize_whitespace_again`) renders the same text
three times with one renderer and requires the miss counter to stay flat after
the first frame.

With the patch, the same document's frame time drops from ~25 ms to ~8 ms of
rasterization, and the per-frame cache misses drop to 0.
