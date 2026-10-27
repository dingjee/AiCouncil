use crate::core::alignment;
use crate::core::text::{Alignment, Shaping};
use crate::core::{Color, Font, Pixels, Point, Rectangle, Transformation};
use crate::graphics::text::cache::{self, Cache};
use crate::graphics::text::editor;
use crate::graphics::text::font_system;
use crate::graphics::text::paragraph;

use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::hash_map;
use std::sync::OnceLock;

/// Gamma applied to grayscale text coverage before it is composited.
///
/// Upstream uses Swash's grayscale area coverage directly as alpha. This tone
/// curve reduces partially covered edges for lighter normal text at the app's
/// small reading sizes, where dense CJK glyphs can look crowded. Font selection,
/// glyph placement, and fully covered ink are preserved.
///
/// This is an empirical visual compensation, not linear-light compositing or a
/// model of DirectWrite/Typedown. It does not claim pixel equivalence with them.
pub const TEXT_COVERAGE_GAMMA: f32 = 1.6;

/// Weight (CSS numeric) at or above which the coverage correction is not
/// applied.
///
/// Explicit emphasis (bold and heavier) must keep the weight the user asked
/// for, so those runs keep the raw coverage of the scaler untouched.
pub const TEXT_COVERAGE_BOLD_THRESHOLD: u16 = 600;

/// Number of entries in a coverage lookup table (one per alpha value).
const COVERAGE_LUT_LEN: usize = 256;

/// Builds a coverage -> alpha lookup table.
///
/// `gamma` of `None` yields the identity table (raw coverage).
fn build_coverage_lut(gamma: Option<f32>) -> [u8; COVERAGE_LUT_LEN] {
    let mut lut = [0u8; COVERAGE_LUT_LEN];

    for (alpha, slot) in lut.iter_mut().enumerate() {
        // Endpoints are preserved exactly: fully transparent stays
        // transparent, fully covered ink stays fully covered.
        if alpha == 0 || alpha == 255 {
            *slot = alpha as u8;
            continue;
        }

        let Some(gamma) = gamma else {
            *slot = alpha as u8;
            continue;
        };

        let coverage = alpha as f32 / 255.0;

        // Apply a coverage tone curve with an exponent > 1. This reduces
        // partial coverage (thinner fringe) while keeping the mapping
        // monotonic and strictly inside the endpoints.
        let corrected = coverage.powf(gamma);

        *slot = (corrected * 255.0).round().clamp(1.0, 254.0) as u8;
    }

    lut
}

/// Coverage -> alpha lookup table for [`TEXT_COVERAGE_GAMMA`].
///
/// Built once, lazily: the table is pure data, so it can be shared between
/// threads without any locking beyond the one-time initialization.
fn coverage_lut() -> &'static [u8; COVERAGE_LUT_LEN] {
    static LUT: OnceLock<[u8; COVERAGE_LUT_LEN]> = OnceLock::new();

    LUT.get_or_init(|| build_coverage_lut(Some(TEXT_COVERAGE_GAMMA)))
}

/// Identity coverage -> alpha lookup table (raw scaler coverage).
fn raw_coverage_lut() -> &'static [u8; COVERAGE_LUT_LEN] {
    static LUT: OnceLock<[u8; COVERAGE_LUT_LEN]> = OnceLock::new();

    LUT.get_or_init(|| build_coverage_lut(None))
}

/// Coverage -> alpha table used for a run of the given font weight.
fn coverage_table(weight: u16) -> &'static [u8; COVERAGE_LUT_LEN] {
    if weight >= TEXT_COVERAGE_BOLD_THRESHOLD {
        raw_coverage_lut()
    } else {
        coverage_lut()
    }
}

/// Adjusts a grayscale coverage sample to the alpha used for compositing.
///
/// `weight` is the numeric font weight of the run the sample belongs to.
/// Runs at or above [`TEXT_COVERAGE_BOLD_THRESHOLD`] keep the raw coverage so
/// that explicit bold is not weakened.
pub fn coverage_to_alpha(coverage: u8, weight: u16) -> u8 {
    coverage_table(weight)[coverage as usize]
}

/// Number of glyph-cache misses since process start.
///
/// A miss means the scaler ran for that glyph. Whitespace has no ink and is
/// cached as an empty result (see [`GlyphCache::allocate`]), so a steady
/// frame must not grow this counter.
pub static GLYPH_CACHE_MISSES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[derive(Debug)]
pub struct Pipeline {
    glyph_cache: GlyphCache,
    cache: RefCell<Cache>,
}

impl Pipeline {
    pub fn new() -> Self {
        Pipeline {
            glyph_cache: GlyphCache::new(),
            cache: RefCell::new(Cache::new()),
        }
    }

    // TODO: Shared engine
    #[allow(dead_code)]
    pub fn load_font(&mut self, bytes: Cow<'static, [u8]>) {
        font_system()
            .write()
            .expect("Write font system")
            .load_font(bytes);

        self.cache = RefCell::new(Cache::new());
    }

    pub fn draw_paragraph(
        &mut self,
        paragraph: &paragraph::Weak,
        position: Point,
        color: Color,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: Option<&tiny_skia::Mask>,
        transformation: Transformation,
    ) {
        let Some(paragraph) = paragraph.upgrade() else {
            return;
        };

        let mut font_system = font_system().write().expect("Write font system");

        draw(
            font_system.raw(),
            &mut self.glyph_cache,
            paragraph.buffer(),
            position,
            color,
            pixels,
            clip_mask,
            transformation,
        );
    }

    pub fn draw_editor(
        &mut self,
        editor: &editor::Weak,
        position: Point,
        color: Color,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: Option<&tiny_skia::Mask>,
        transformation: Transformation,
    ) {
        let Some(editor) = editor.upgrade() else {
            return;
        };

        let mut font_system = font_system().write().expect("Write font system");

        draw(
            font_system.raw(),
            &mut self.glyph_cache,
            editor.buffer(),
            position,
            color,
            pixels,
            clip_mask,
            transformation,
        );
    }

    pub fn draw_cached(
        &mut self,
        content: &str,
        bounds: Rectangle,
        color: Color,
        size: Pixels,
        line_height: Pixels,
        font: Font,
        align_x: Alignment,
        align_y: alignment::Vertical,
        shaping: Shaping,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: Option<&tiny_skia::Mask>,
        transformation: Transformation,
    ) {
        let line_height = f32::from(line_height);

        let mut font_system = font_system().write().expect("Write font system");
        let font_system = font_system.raw();

        let key = cache::Key {
            bounds: bounds.size(),
            content,
            font,
            size: size.into(),
            line_height,
            shaping,
            align_x,
        };

        let (_, entry) = self.cache.get_mut().allocate(font_system, key);

        let width = entry.min_bounds.width;
        let height = entry.min_bounds.height;

        let x = match align_x {
            Alignment::Default | Alignment::Left | Alignment::Justified => {
                bounds.x
            }
            Alignment::Center => bounds.x - width / 2.0,
            Alignment::Right => bounds.x - width,
        };

        let y = match align_y {
            alignment::Vertical::Top => bounds.y,
            alignment::Vertical::Center => bounds.y - height / 2.0,
            alignment::Vertical::Bottom => bounds.y - height,
        };

        draw(
            font_system,
            &mut self.glyph_cache,
            &entry.buffer,
            Point::new(x, y),
            color,
            pixels,
            clip_mask,
            transformation,
        );
    }

    pub fn draw_raw(
        &mut self,
        buffer: &cosmic_text::Buffer,
        position: Point,
        color: Color,
        pixels: &mut tiny_skia::PixmapMut<'_>,
        clip_mask: Option<&tiny_skia::Mask>,
        transformation: Transformation,
    ) {
        let mut font_system = font_system().write().expect("Write font system");

        draw(
            font_system.raw(),
            &mut self.glyph_cache,
            buffer,
            position,
            color,
            pixels,
            clip_mask,
            transformation,
        );
    }

    pub fn trim_cache(&mut self) {
        self.cache.get_mut().trim();
        self.glyph_cache.trim();
    }
}

fn draw(
    font_system: &mut cosmic_text::FontSystem,
    glyph_cache: &mut GlyphCache,
    buffer: &cosmic_text::Buffer,
    position: Point,
    color: Color,
    pixels: &mut tiny_skia::PixmapMut<'_>,
    clip_mask: Option<&tiny_skia::Mask>,
    transformation: Transformation,
) {
    let position = position * transformation;

    let mut swash = cosmic_text::SwashCache::new();

    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let physical_glyph = glyph.physical(
                (position.x, position.y),
                transformation.scale_factor(),
            );

            if let Some((buffer, placement)) = glyph_cache.allocate(
                physical_glyph.cache_key,
                glyph.color_opt.map(from_color).unwrap_or(color),
                font_system,
                &mut swash,
            ) {
                let pixmap = tiny_skia::PixmapRef::from_bytes(
                    buffer,
                    placement.width,
                    placement.height,
                )
                .expect("Create glyph pixel map");

                let opacity = color.a
                    * glyph
                        .color_opt
                        .map(|c| c.a() as f32 / 255.0)
                        .unwrap_or(1.0);

                pixels.draw_pixmap(
                    physical_glyph.x + placement.left,
                    physical_glyph.y - placement.top
                        + (run.line_y * transformation.scale_factor()).round()
                            as i32,
                    pixmap,
                    &tiny_skia::PixmapPaint {
                        opacity,
                        ..tiny_skia::PixmapPaint::default()
                    },
                    tiny_skia::Transform::identity(),
                    clip_mask,
                );
            }
        }
    }
}

fn from_color(color: cosmic_text::Color) -> Color {
    let [r, g, b, a] = color.as_rgba();

    Color::from_rgba8(r, g, b, a as f32 / 255.0)
}

/// Placement used for glyphs that have no ink (whitespace).
const EMPTY_PLACEMENT: cosmic_text::Placement = cosmic_text::Placement {
    left: 0,
    top: 0,
    width: 0,
    height: 0,
};

#[derive(Debug, Clone, Default)]
struct GlyphCache {
    entries: FxHashMap<
        (cosmic_text::CacheKey, [u8; 3]),
        (Vec<u32>, cosmic_text::Placement),
    >,
    recently_used: FxHashSet<(cosmic_text::CacheKey, [u8; 3])>,
    trim_count: usize,
}

impl GlyphCache {
    const TRIM_INTERVAL: usize = 300;
    const CAPACITY_LIMIT: usize = 16 * 1024;

    fn new() -> Self {
        GlyphCache::default()
    }

    fn allocate(
        &mut self,
        cache_key: cosmic_text::CacheKey,
        color: Color,
        font_system: &mut cosmic_text::FontSystem,
        swash: &mut cosmic_text::SwashCache,
    ) -> Option<(&[u8], cosmic_text::Placement)> {
        let [r, g, b, _a] = color.into_rgba8();
        let key = (cache_key, [r, g, b]);

        if let hash_map::Entry::Vacant(entry) = self.entries.entry(key) {
            // TODO: Outline support
            let _ = GLYPH_CACHE_MISSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(image) = swash.get_image_uncached(font_system, cache_key) else {
                // The glyph has no rasterizable outline (e.g. a control
                // character). Remember that, otherwise every frame pays the
                // shaping/rasterization probe again for the same glyph.
                let _ = entry.insert((Vec::new(), EMPTY_PLACEMENT));
                return None;
            };

            let glyph_size = image.placement.width as usize
                * image.placement.height as usize;

            if glyph_size == 0 {
                // Whitespace glyphs have no ink. Upstream returned early
                // *without* inserting the entry, so the cache never learned
                // about them: every frame re-ran the scaler for each space
                // on screen (a long Chinese paragraph has dozens of them),
                // which dominated the frame time while scrolling.
                // Cache the empty result instead.
                let _ = entry.insert((Vec::new(), EMPTY_PLACEMENT));
                return None;
            }

            let mut buffer = vec![0u32; glyph_size];

            match image.content {
                cosmic_text::SwashContent::Mask => {
                    let mut i = 0;

                    // Grayscale coverage is composited as-is by upstream.
                    // The coverage is adjusted (see `coverage_to_alpha`) so
                    // that partially covered pixels contribute less ink, which
                    // keeps dense CJK glyphs from clogging at small sizes.
                    let alpha = coverage_table(cache_key.font_weight.0);

                    for _y in 0..image.placement.height {
                        for _x in 0..image.placement.width {
                            buffer[i] = bytemuck::cast(
                                tiny_skia::ColorU8::from_rgba(
                                    b,
                                    g,
                                    r,
                                    alpha[image.data[i] as usize],
                                )
                                .premultiply(),
                            );

                            i += 1;
                        }
                    }
                }
                cosmic_text::SwashContent::Color => {
                    let mut i = 0;

                    for _y in 0..image.placement.height {
                        for _x in 0..image.placement.width {
                            // TODO: Blend alpha
                            buffer[i >> 2] = bytemuck::cast(
                                tiny_skia::ColorU8::from_rgba(
                                    image.data[i + 2],
                                    image.data[i + 1],
                                    image.data[i],
                                    image.data[i + 3],
                                )
                                .premultiply(),
                            );

                            i += 4;
                        }
                    }
                }
                cosmic_text::SwashContent::SubpixelMask => {
                    // TODO
                }
            }

            let _ = entry.insert((buffer, image.placement));
        }

        let _ = self.recently_used.insert(key);

        self.entries.get(&key).and_then(|(buffer, placement)| {
            // An empty buffer marks a glyph without ink (whitespace).
            if buffer.is_empty() {
                return None;
            }
            Some((bytemuck::cast_slice(buffer.as_slice()), *placement))
        })
    }

    pub fn trim(&mut self) {
        if self.trim_count > Self::TRIM_INTERVAL
            || self.recently_used.len() >= Self::CAPACITY_LIMIT
        {
            self.entries
                .retain(|key, _| self.recently_used.contains(key));

            self.recently_used.clear();

            self.entries.shrink_to(Self::CAPACITY_LIMIT);
            self.recently_used.shrink_to(Self::CAPACITY_LIMIT);

            self.trim_count = 0;
        } else {
            self.trim_count += 1;
        }
    }
}
