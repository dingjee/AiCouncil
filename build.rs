//! 构建脚本:Windows 下把 assets/icons/aicouncil.ico 嵌入 exe 资源
//! (资源管理器/任务栏图标)。ICO 由 `examples/gen_icon.rs` 生成。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon = "assets/icons/aicouncil.ico";
        if std::path::Path::new(icon).exists() {
            winresource::WindowsResource::new()
                .set_icon(icon)
                .compile()
                .unwrap_or_else(|e| panic!("嵌入 Windows 图标失败: {e}"));
        } else {
            panic!("缺少 Windows 图标: {icon}");
        }
    }
    println!("cargo:rerun-if-changed=assets/icons/logo.svg");
    println!("cargo:rerun-if-changed=assets/icons/logo.png");
    println!("cargo:rerun-if-changed=assets/icons/aicouncil.ico");
}
