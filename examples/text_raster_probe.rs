//! 离线字体栅格探针:输出“修补前 / 修补后”的对比图与度量。
//!
//! 运行:`cargo run --release --example text_raster_probe`
//! 输出:`screenshots/text_raster/*.png`(已 gitignore,验收后删除)
//!
//! 覆盖:15/18/22px × 1.0/1.5 DPI × 正常/加粗 × 中文(情 馈 懂 赢)/拉丁。
//! 左半边是上游行为(原始线性覆盖率),右半边是修补后的覆盖率调整。

#[path = "raster/common.rs"]
mod common;

use std::path::PathBuf;

use common::{Coverage, CJK_SAMPLE, LATIN_SAMPLE};

const SIZES: [f32; 3] = [15.0, 18.0, 22.0];
const SCALES: [f32; 2] = [1.0, 1.5];
const WEIGHTS: [u16; 2] = [400, 700];

/// 输出图放大倍数(最近邻),便于肉眼检查栅格。
const ZOOM: usize = 6;

fn main() {
    let output = PathBuf::from("screenshots/text_raster");
    std::fs::create_dir_all(&output).expect("create screenshot directory");

    println!(
        "coverage gamma = {}, bold threshold = {}",
        iced_tiny_skia::TEXT_COVERAGE_GAMMA,
        iced_tiny_skia::TEXT_COVERAGE_BOLD_THRESHOLD
    );
    println!("size dpi weight | ink(raw) ink(fix) delta | full(raw) full(fix) | faces");

    for sample in [CJK_SAMPLE, LATIN_SAMPLE] {
        let label = if sample == CJK_SAMPLE { "cjk" } else { "latin" };

        for size in SIZES {
            for scale in SCALES {
                for weight in WEIGHTS {
                    let raw = common::rasterize(sample, size, scale, weight, Coverage::Raw);
                    let fixed = common::rasterize(sample, size, scale, weight, Coverage::Corrected);

                    let raw_metrics = raw.metrics();
                    let fixed_metrics = fixed.metrics();

                    let delta = if raw_metrics.ink == 0 {
                        0.0
                    } else {
                        (fixed_metrics.ink as f64 - raw_metrics.ink as f64) / raw_metrics.ink as f64
                            * 100.0
                    };

                    println!(
                        "{size:>6.0} {scale:>5.1} {weight:>5} | {:>9} {:>9} {:>5.1}% | {:>6} {:>6} | {}",
                        raw_metrics.ink, fixed_metrics.ink, delta,
                        raw_metrics.solid, fixed_metrics.solid, raw.faces.join(", ")
                    );

                    let (width, height, rgba) = common::side_by_side(&raw, &fixed);
                    let (width, height, rgba) = common::zoom(&rgba, width, height, ZOOM);

                    common::write_png(
                        &output.join(format!("{label}-{size:.0}px-{scale:.1}x-{weight}.png")),
                        width,
                        height,
                        &rgba,
                    );
                }
            }
        }
    }

    println!("\nimages: {}", output.display());
}
