//! 入口:日志初始化,交予 Tauri 桌面装配。

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aicouncil=info".into()),
        )
        .init();

    aicouncil::desktop::run();
}
