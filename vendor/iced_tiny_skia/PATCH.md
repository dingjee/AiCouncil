# iced_tiny_skia local patch

This is the upstream `iced_tiny_skia` 0.14.1 crate (MIT, `iced-rs/iced`) with one local change in `src/engine.rs`.

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
