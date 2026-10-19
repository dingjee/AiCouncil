//! 入口:启动、日志、平台数据目录(实际初始化在 `app::main`)。

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

fn main() -> iced::Result {
    aicouncil::app::main()
}
