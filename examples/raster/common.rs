//! 共享:字形栅格化与覆盖率度量(离线探针与回归测试共用)。
//!
//! 这里复刻生产路径(iced_tiny_skia → cosmic-text 排版 → swash 栅格化),
//! 唯一区别是“灰度覆盖率如何变成 alpha”:
//!
//! - `Coverage::Raw`:修补前,直接使用标量器输出的线性覆盖率(上游行为)。
//! - `Coverage::Corrected`:修补后,使用生产代码
//!   `iced_tiny_skia::coverage_to_alpha` 的覆盖率调整。
//!
//! 合成模型与生产一致:字形颜色按覆盖率做预乘,再在 sRGB 空间做 source-over,
//! 即 `out = ink * a + bg * (1 - a)`。

#![allow(dead_code)]

use std::path::Path;

use iced_tiny_skia::graphics::text::{cosmic_text, font_system};

/// 探针样例:笔画密集、字腔细小的常用汉字。
pub const CJK_SAMPLE: &str = "情 馈 懂 赢";

/// 拉丁样例:用于对比覆盖率修正对拉丁字形的影响。
pub const LATIN_SAMPLE: &str = "Ag Wy";

/// 背景色(与界面浅色主题一致)。
pub const BACKGROUND: [u8; 3] = [250, 250, 248];

/// 文字颜色。
pub const INK: [u8; 3] = [38, 38, 38];

/// 覆盖率处理方式。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Coverage {
    /// 修补前:原始线性覆盖率。
    Raw,
    /// 修补后:生产路径使用的覆盖率调整。
    Corrected,
}

/// 一次栅格化的结果。
pub struct Raster {
    pub width: usize,
    pub height: usize,
    /// 灰度字形的累计覆盖率(0..=255)。
    pub coverage: Vec<u8>,
    /// 合成到背景后的 RGBA 像素。
    pub rgba: Vec<u8>,
    /// 彩色字形(emoji)数量:彩色字形不走覆盖率路径。
    pub color_glyphs: usize,
    /// 参与栅格化的字体名(去重)。
    pub faces: Vec<String>,
}

/// 覆盖率度量。
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    /// Sum of grayscale coverage (255 is one fully covered pixel).
    pub ink: u64,
    /// Number of fully covered pixels, whose ink must remain unchanged.
    pub solid: usize,
}

impl Raster {
    pub fn metrics(&self) -> Metrics {
        metrics(&self.coverage)
    }

    /// 墨迹包围盒(不含背景),返回 (x0, y0, x1, y1)。
    pub fn ink_bounds(&self) -> Option<(usize, usize, usize, usize)> {
        let mut bounds: Option<(usize, usize, usize, usize)> = None;

        for y in 0..self.height {
            for x in 0..self.width {
                if self.coverage[y * self.width + x] >= 32 {
                    bounds = Some(match bounds {
                        None => (x, y, x + 1, y + 1),
                        Some((x0, y0, x1, y1)) => {
                            (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1))
                        }
                    });
                }
            }
        }

        bounds
    }
}

/// 栅格化一行文本。
///
/// `size` 是逻辑字号,`scale` 是缩放因子(1.0 = 1x DPI,1.5 = 1.5x DPI),
/// `weight` 是 CSS 数值字重(400 正常,700 加粗)。
pub fn rasterize(text: &str, size: f32, scale: f32, weight: u16, variant: Coverage) -> Raster {
    let mut guard = font_system().write().expect("font system lock");
    let fs = guard.raw();

    let mut buffer = cosmic_text::Buffer::new(fs, cosmic_text::Metrics::new(size, size * 1.4));

    let attrs = cosmic_text::Attrs::new()
        .family(cosmic_text::Family::SansSerif)
        .weight(cosmic_text::Weight(weight));

    buffer.set_text(fs, text, &attrs, cosmic_text::Shaping::Advanced, None);

    let mut swash = cosmic_text::SwashCache::new();

    struct Placed {
        x: i32,
        y: i32,
        width: usize,
        height: usize,
        data: Vec<u8>,
        color: bool,
        weight: u16,
    }

    let mut placed: Vec<Placed> = Vec::new();
    let mut faces: Vec<String> = Vec::new();

    for run in buffer.layout_runs() {
        for glyph in run.glyphs {
            let physical = glyph.physical((0.0, 0.0), scale);

            let Some(image) = swash.get_image_uncached(fs, physical.cache_key) else {
                continue;
            };

            if let Some(face) = fs.db_mut().face(glyph.font_id) {
                let name = face
                    .families
                    .first()
                    .map(|(name, _)| name.clone())
                    .unwrap_or_else(|| face.post_script_name.clone());

                if !faces.contains(&name) {
                    faces.push(name);
                }
            }

            let (data, color) = match image.content {
                cosmic_text::SwashContent::Mask => (image.data.clone(), false),
                cosmic_text::SwashContent::Color => (image.data.clone(), true),
                cosmic_text::SwashContent::SubpixelMask => continue,
            };

            placed.push(Placed {
                x: physical.x + image.placement.left,
                y: physical.y - image.placement.top + (run.line_y * scale).round() as i32,
                width: image.placement.width as usize,
                height: image.placement.height as usize,
                data,
                color,
                weight: physical.cache_key.font_weight.0,
            });
        }
    }

    let padding = 2i32;

    let min_x = placed.iter().map(|p| p.x).min().unwrap_or(0) - padding;
    let min_y = placed.iter().map(|p| p.y).min().unwrap_or(0) - padding;
    let max_x = placed
        .iter()
        .map(|p| p.x + p.width as i32)
        .max()
        .unwrap_or(0)
        + padding;
    let max_y = placed
        .iter()
        .map(|p| p.y + p.height as i32)
        .max()
        .unwrap_or(0)
        + padding;

    let width = (max_x - min_x).max(1) as usize;
    let height = (max_y - min_y).max(1) as usize;

    let mut coverage = vec![0u8; width * height];
    let mut rgba = Vec::with_capacity(width * height * 4);

    // 背景
    for _ in 0..width * height {
        rgba.extend_from_slice(&[BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255]);
    }

    let mut color_glyphs = 0;

    for glyph in &placed {
        if glyph.color {
            color_glyphs += 1;

            for row in 0..glyph.height {
                for column in 0..glyph.width {
                    let x = glyph.x - min_x + column as i32;
                    let y = glyph.y - min_y + row as i32;

                    if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
                        continue;
                    }

                    let index = (row * glyph.width + column) * 4;
                    let alpha = glyph.data[index + 3] as u32;

                    if alpha == 0 {
                        continue;
                    }

                    let offset = (y as usize * width + x as usize) * 4;

                    for channel in 0..3 {
                        let source = glyph.data[index + channel] as u32;
                        let destination = rgba[offset + channel] as u32;

                        rgba[offset + channel] =
                            ((source * alpha + destination * (255 - alpha)) / 255) as u8;
                    }
                }
            }

            continue;
        }

        for row in 0..glyph.height {
            for column in 0..glyph.width {
                let x = glyph.x - min_x + column as i32;
                let y = glyph.y - min_y + row as i32;

                if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
                    continue;
                }

                let sample = glyph.data[row * glyph.width + column];

                let alpha = match variant {
                    Coverage::Raw => sample,
                    Coverage::Corrected => iced_tiny_skia::coverage_to_alpha(sample, glyph.weight),
                };

                let offset = y as usize * width + x as usize;
                let previous = coverage[offset] as u32;

                // source-over 累计覆盖率
                coverage[offset] =
                    (alpha as u32 + previous * (255 - alpha as u32) / 255).min(255) as u8;
            }
        }
    }

    // 覆盖率 → 颜色(与生产一致:sRGB 空间 source-over)
    for (index, alpha) in coverage.iter().enumerate() {
        let alpha = *alpha as u32;

        for channel in 0..3 {
            let ink = INK[channel] as u32;
            let background = BACKGROUND[channel] as u32;

            rgba[index * 4 + channel] = ((ink * alpha + background * (255 - alpha)) / 255) as u8;
        }
    }

    Raster {
        width,
        height,
        coverage,
        rgba,
        color_glyphs,
        faces,
    }
}

/// Coverage measurements over the same aligned glyph mask.
pub fn metrics(coverage: &[u8]) -> Metrics {
    Metrics {
        ink: coverage.iter().map(|value| *value as u64).sum(),
        solid: coverage.iter().filter(|value| **value == 255).count(),
    }
}

/// Nearest-neighbor enlargement of diagnostic pixels.
pub fn zoom(rgba: &[u8], width: usize, height: usize, factor: usize) -> (usize, usize, Vec<u8>) {
    let width = width * factor;
    let height = height * factor;
    let mut out = vec![0u8; width * height * 4];

    for y in 0..height {
        for x in 0..width {
            let source = ((y / factor) * (width / factor) + x / factor) * 4;
            let target = (y * width + x) * 4;
            out[target..target + 4].copy_from_slice(&rgba[source..source + 4]);
        }
    }

    (width, height, out)
}

/// 写 PNG。
pub fn write_png(path: &Path, width: usize, height: usize, rgba: &[u8]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create screenshot directory");
    }

    let file = std::fs::File::create(path).expect("create png file");
    let mut encoder = png::Encoder::new(file, width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(rgba)
        .expect("png data");
}

/// 横向并排两张图(左:修补前,右:修补后),中间留 4px 间隔。
pub fn side_by_side(left: &Raster, right: &Raster) -> (usize, usize, Vec<u8>) {
    let gap = 4;
    let width = left.width + gap + right.width;
    let height = left.height.max(right.height);
    let mut rgba = vec![255u8; width * height * 4];

    for y in 0..height {
        for x in 0..width {
            let offset = (y * width + x) * 4;

            let (buffer, buffer_width, source_x, source_y) = if x < left.width {
                (&left.rgba, left.width, x, y)
            } else if x >= left.width + gap {
                (&right.rgba, right.width, x - left.width - gap, y)
            } else {
                continue;
            };

            if source_y < buffer.len() / buffer_width / 4 {
                let index = (source_y * buffer_width + source_x) * 4;
                rgba[offset..offset + 4].copy_from_slice(&buffer[index..index + 4]);
            }
        }
    }

    (width, height, rgba)
}
