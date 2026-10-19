//! 生成应用图标产物:`assets/icons/logo.png`(256×256 RGBA)与
//! `assets/icons/aicouncil.ico`(多尺寸,PNG 压缩条目)。
//! 源:`assets/icons/logo.svg`。产物提交入库,运行时不依赖本工具。

use std::fs::write;
use std::path::Path;

fn main() {
    let svg = std::fs::read("assets/icons/logo.svg").expect("读取 logo.svg");
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(&svg, &opt).expect("解析 SVG");

    // 256×256 PNG(窗口图标运行时解码用)
    render_to_png(&tree, 256, "assets/icons/logo.png");

    // ICO:覆盖常见任务栏、列表和高 DPI 尺寸。
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut entries: Vec<(u8, u8, Vec<u8>)> = Vec::new();
    for &s in &sizes {
        entries.push((s as u8, s as u8, render_png_bytes(&tree, s)));
    }
    write_ico(&entries, "assets/icons/aicouncil.ico");
    println!("生成完成: assets/icons/logo.png, assets/icons/aicouncil.ico");
}

fn render_to_png(tree: &resvg::usvg::Tree, size: u32, path: &str) {
    let bytes = render_png_bytes(tree, size);
    write(Path::new(path), bytes).expect("写 PNG");
    println!("  {path} ({size}x{size})");
}

fn render_png_bytes(tree: &resvg::usvg::Tree, size: u32) -> Vec<u8> {
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
    // SVG viewBox 已裁到主体边界，等比铺满目标方形画布。
    let svg_size = tree.size();
    let scale = size as f32 / svg_size.width().max(svg_size.height());
    let tx = (size as f32 - svg_size.width() * scale) / 2.0;
    let ty = (size as f32 - svg_size.height() * scale) / 2.0;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale).post_translate(tx, ty);
    resvg::render(tree, transform, &mut pixmap.as_mut());
    pixmap.encode_png().expect("encode png")
}

fn write_ico(entries: &[(u8, u8, Vec<u8>)], path: &str) {
    // ICO 容器:ICONDIR(6 字节) + N×ICONDIRENTRY(16 字节) + 数据
    let mut out = vec![0u8, 0u8, 1u8, 0u8, entries.len() as u8, 0u8];
    let mut offset = 6 + 16 * entries.len();
    for (w, h, data) in entries {
        out.push(*w); // 0 表示 256
        out.push(*h);
        out.push(0); // 调色板
        out.push(0); // reserved
        out.push(1); // planes
        out.push(0);
        out.push(32); // bpp
        out.push(0);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += data.len();
    }
    for (_, _, data) in entries {
        out.extend_from_slice(data);
    }
    write(Path::new(path), out).expect("写 ICO");
    println!("  {path} ({} 尺寸)", entries.len());
}
