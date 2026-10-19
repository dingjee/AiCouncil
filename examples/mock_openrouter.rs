//! 本地 OpenRouter mock:/models 目录 + /chat/completions SSE 流。
//! 用法:`cargo run --example mock_openrouter -- [port]`
//! 配合:AICOUNCIL_BASE_URL=http://127.0.0.1:[port]/v1
//!
//! 目录含多家族模型(Claude/GPT/Gemini/独立),验证家族分组展示;
//! chat 流式回显模型名;model 含 "fail" 的请求返回 SSE 错误帧
//! (验证失败恢复);其余正常 stop+usage+DONE。
//!
//! 环境开关(E2E 辅助):
//!   MOCK_NO_ANTHROPIC=1  目录去掉 Anthropic 条目(首模型 openai/gpt-4o)
//!   MOCK_FAIL_FIRST=1    目录首模型为 mock/fail-demo(autopilot 首发即失败)

use std::time::Duration;

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(8901);
    let no_anthropic = std::env::var_os("MOCK_NO_ANTHROPIC").is_some();
    let fail_first = std::env::var_os("MOCK_FAIL_FIRST").is_some();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind");
    println!(
        "mock openrouter on 127.0.0.1:{port} (no_anthropic={no_anthropic}, fail_first={fail_first})"
    );
    loop {
        let (mut sock, _) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => return,
        };
        tokio::spawn(async move {
            let mut buf = [0u8; 8192];
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let first_line = String::from_utf8_lossy(&buf[..n.min(120)])
                .lines()
                .next()
                .unwrap_or("")
                .to_string();
            println!("[req] {first_line}");
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();

            let body = if req.contains("/models") {
                let mut entries = vec![
                    serde_json::json!({
                        "id": "anthropic/claude-opus-5.5",
                        "name": "Anthropic: Claude Opus 5.5",
                        "context_length": 32768,
                        "top_provider": {"max_completion_tokens": 4096},
                        "pricing": {"prompt": "0.000003", "completion": "0.000015"},
                        "supported_parameters": ["temperature", "max_tokens"]
                    }),
                    serde_json::json!({
                        "id": "anthropic/claude-fable-5.1",
                        "name": "Anthropic: Claude Fable 5.1",
                        "context_length": 32768,
                        "top_provider": {"max_completion_tokens": 4096},
                        "pricing": {"prompt": "0.000002", "completion": "0.000010"},
                        "supported_parameters": ["temperature"]
                    }),
                    serde_json::json!({
                        "id": "openai/gpt-4o",
                        "name": "OpenAI: GPT-4o",
                        "context_length": 16384,
                        "top_provider": {"max_completion_tokens": 2048},
                        "pricing": {"prompt": "0.000001", "completion": "0.000002"},
                        "supported_parameters": ["temperature", "max_tokens"]
                    }),
                    serde_json::json!({
                        "id": "google/gemini-2.0-flash",
                        "name": "Google: Gemini 2.0 Flash",
                        "context_length": 32768,
                        "top_provider": {"max_completion_tokens": 4096},
                        "pricing": {"prompt": "0", "completion": "0"},
                        "supported_parameters": []
                    }),
                    serde_json::json!({
                        "id": "moonshotai/kimi-k2",
                        "name": "Moonshot AI: Kimi K2",
                        "context_length": 32768,
                        "top_provider": {"max_completion_tokens": 4096},
                        "pricing": {"prompt": "0", "completion": "0"},
                        "supported_parameters": []
                    }),
                    serde_json::json!({
                        "id": "mock/alpha",
                        "name": "Mock Alpha",
                        "context_length": 32768,
                        "top_provider": {"max_completion_tokens": 4096},
                        "pricing": {"prompt": "0.000001", "completion": "0.000002"},
                        "supported_parameters": ["temperature", "max_tokens"]
                    }),
                    serde_json::json!({
                        "id": "mock/beta",
                        "name": "Mock Beta",
                        "context_length": 16384,
                        "top_provider": {"max_completion_tokens": 2048},
                        "pricing": {"prompt": "0", "completion": "0"},
                        "supported_parameters": []
                    }),
                ];
                if fail_first {
                    // autopilot 首发即失败(失败卡/重试 UI 验证)
                    entries.insert(
                        0,
                        serde_json::json!({
                            "id": "mock/fail-demo",
                            "name": "Mock Fail Demo",
                            "context_length": 8192,
                            "top_provider": {"max_completion_tokens": 2048},
                            "pricing": {"prompt": "0", "completion": "0"},
                            "supported_parameters": []
                        }),
                    );
                }
                if no_anthropic {
                    entries.retain(|e| {
                        e["id"]
                            .as_str()
                            .map(|s| !s.starts_with("anthropic/"))
                            .unwrap_or(true)
                    });
                }
                serde_json::json!({ "data": entries }).to_string()
            } else if req.contains("/chat/completions") {
                // 解析 model 字段,不同模型回不同内容
                let model = req
                    .split("\"model\":\"")
                    .nth(1)
                    .and_then(|s| s.split('"').next())
                    .unwrap_or("mock/alpha")
                    .to_string();
                if model.contains("fail") {
                    // SSE 错误帧(验证失败恢复;HTTP 200 内错误)
                    let sse = "data: {\"error\":{\"message\":\"mock upstream failure\",\"code\":500}}\n\ndata: [DONE]\n\n";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse}"
                    )
                } else {
                    let mut sse = String::new();
                    for chunk in ["你好", ",", "我是 ", &model, " 模型。\n\n公式：$E=mc^2$"]
                    {
                        let esc = chunk.replace('"', "\\\"");
                        sse.push_str(&format!(
                            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{esc}\"}}}}]}}\n\n"
                        ));
                    }
                    sse.push_str(
                        "data: {\"id\":\"gen-mock-123\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    );
                    sse.push_str(
                        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":88,\"completion_tokens\":21},\"cost\":\"0.00042\"}\n\n",
                    );
                    sse.push_str("data: [DONE]\n\n");
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse}"
                    )
                }
            } else if req.contains("/key") {
                "{\"data\":{\"label\":\"mock\",\"usage\":0.5,\"limit\":10.0,\"limit_remaining\":9.5}}".to_string()
            } else {
                "{\"ok\":true}".to_string()
            };

            let ctype = if body.starts_with("HTTP/") {
                body.clone()
            } else {
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            // SSE 按块写,模拟流式
            if ctype.contains("event-stream") {
                for chunk in ctype.as_bytes().chunks(24) {
                    let _ = sock.write_all(chunk).await;
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            } else {
                let _ = sock.write_all(ctype.as_bytes()).await;
            }
        });
    }
}
