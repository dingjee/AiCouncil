//! 原生冒烟 glue(K4C 最小实现;harness 本体为 GLM 的 `src/native_smoke.rs`)。
//!
//! 契约(GLMF_STATUS 协作项 1):
//! - 页面加载 **Finished** 时注入 `native_smoke::page_load_script()`
//!   (真实 IPC + DOM 断言);`Started` 时注入过早(文档/JS 上下文未就绪),
//!   会让 harness 在第一行 `window.__TAURI_INTERNALS__.invoke` 处抛错 → 无报告;
//! - 注册 debug-only 命令 `smoke_report(payload: serde_json::Value)`
//!   → `native_smoke::write_report_json(&payload.to_string())`。
//!
//! 健壮性(实测“无报告”事故后的加固):
//! - 注入脚本先轮询等待 Tauri IPC 就绪(`__TAURI_INTERNALS__.invoke`),
//!   再执行 harness;harness 同步抛错时兜底上报一份失败报告;
//! - Rust 侧兜底:页面加载回调未触发时,按秒重试注入(直到出现报告文件)。
//!
//! 安全:仅 debug 构建 + 显式 `AICOUNCIL_NATIVE_SMOKE=1` 生效;
//! release 不含脚本与报告落盘逻辑(命令体为空操作)。

use tauri::webview::{PageLoadEvent, PageLoadPayload, Webview};
use tauri::Manager;

/// debug-only 模块(glm 所有)。
#[cfg(debug_assertions)]
use crate::native_smoke;

/// 冒烟是否启用(debug + 显式 opt-in;与 harness 同一判定)。
pub fn enabled() -> bool {
    #[cfg(debug_assertions)]
    {
        native_smoke::enabled()
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// 注入包装脚本:等待 IPC 就绪 → 执行 harness → 异常兜底上报。
///
/// harness 文本直接内联进函数体(不使用 `eval`),避免页面 CSP
/// (`script-src 'self'`)阻断间接 eval。
#[cfg(debug_assertions)]
fn injection_script() -> String {
    let harness = native_smoke::page_load_script();
    let mut s = String::with_capacity(harness.len() + 2048);
    s.push_str("(function(){\n");
    s.push_str(
        "  function __aic_ready(){ return !!(window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke); }\n",
    );
    s.push_str(
        "  function __aic_report(payload){ try { if (__aic_ready()) { window.__TAURI_INTERNALS__.invoke('smoke_report', { payload: payload }); } } catch (e) { console.error('[native-smoke] report failed', e); } }\n",
    );
    s.push_str("  function __aic_run(){\n");
    // 同一页面只允许一个 harness 实例(兜底重注入不得并发重复跑)。
    s.push_str("    if (window.__AICOUNCIL_SMOKE_STARTED__) { return; }\n");
    s.push_str("    window.__AICOUNCIL_SMOKE_STARTED__ = true;\n");
    s.push_str("    try {\n");
    s.push_str(harness);
    s.push_str(
        "\n    } catch (e) {\n      window.__AICOUNCIL_SMOKE_STARTED__ = false;\n      console.error('[native-smoke] harness threw', e);\n      __aic_report({ ok: false, enabled: true, steps: [], error: String((e && e.message) || e) });\n    }\n  }\n",
    );
    s.push_str("  if (__aic_ready()) { __aic_run(); }\n");
    s.push_str(
        "  else { var n = 0; var t = setInterval(function(){ n++; if (__aic_ready() || n > 150) { clearInterval(t); __aic_run(); } }, 100); }\n",
    );
    s.push_str("})();\n");
    s
}

/// 页面加载回调:仅在 `Finished` 注入(Started 过早)。
pub fn on_page_load(webview: &Webview, payload: &PageLoadPayload) {
    if !enabled() {
        return;
    }
    if !matches!(payload.event(), PageLoadEvent::Finished) {
        return;
    }
    inject(webview);
}

/// 执行注入(带 stderr 诊断,便于 driver 日志定位)。
pub fn inject(webview: &Webview) {
    #[cfg(debug_assertions)]
    {
        match webview.eval(injection_script()) {
            Ok(()) => eprintln!("[native-smoke] harness injected"),
            Err(e) => eprintln!("[native-smoke] eval failed: {e}"),
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = webview;
    }
}

/// 报告文件是否已出现(用于兜底重注入的终止条件)。
fn report_exists() -> bool {
    #[cfg(debug_assertions)]
    {
        native_smoke::report_path()
            .map(|p| p.exists())
            .unwrap_or(false)
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

/// 兜底重注入:页面加载回调未触发(或注入早于 JS 就绪)时按固定间隔重试,
/// 直到报告文件出现或超时(debug-only)。
///
/// 间隔 10 s:页面级 `__AICOUNCIL_SMOKE_STARTED__` 守卫保证同一页面只跑
/// 一个 harness 实例,重注入不会并发重复执行。
pub fn spawn_fallback_injection(app: tauri::AppHandle) {
    if !enabled() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        for _ in 0..12 {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            if report_exists() {
                return;
            }
            if let Some(window) = app.get_webview_window("main") {
                // WebviewWindow 实现 AsRef<Webview>。
                let webview: &Webview = window.as_ref();
                inject(webview);
            }
        }
    });
}

/// debug-only `smoke_report` 命令的落盘实现;release 为空操作。
pub fn write_report(payload: &serde_json::Value) -> Option<std::path::PathBuf> {
    #[cfg(debug_assertions)]
    {
        native_smoke::write_report_json(&payload.to_string())
            .ok()
            .flatten()
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = payload;
        None
    }
}
