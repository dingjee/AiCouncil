//! 集成测试共享 mock:可计数/可捕获请求体的本地 HTTP/SSE 服务。
//! 端口与进程句柄归测试所有;不依赖网络与真实 Key。

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 计数 + 捕获:每个连接读完请求(按 Content-Length)后计数、
/// 存请求体,再按脚本返回响应。
pub struct CountingMock {
    pub post_count: Arc<AtomicU64>,
    pub bodies: Arc<tokio::sync::Mutex<Vec<String>>>,
    pub handle: tokio::task::JoinHandle<()>,
    pub port: u16,
}

impl CountingMock {
    pub fn count(&self) -> u64 {
        self.post_count.load(Ordering::SeqCst)
    }

    pub async fn all_bodies(&self) -> Vec<String> {
        self.bodies.lock().await.clone()
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 启动可计数 mock:`respond(request_body, headers) -> 响应字节`。
/// 响应按 64B 块写回(流式;SSE 跨块)。
pub async fn spawn_counting_mock<F>(respond: F) -> CountingMock
where
    F: Fn(&str, &[u8]) -> Vec<u8> + Send + Sync + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let post_count = Arc::new(AtomicU64::new(0));
    let bodies: Arc<tokio::sync::Mutex<Vec<String>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let respond = Arc::new(respond);
    let post_count_outer = post_count.clone();
    let bodies_outer = bodies.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let post_count = post_count_outer.clone();
            let bodies = bodies_outer.clone();
            let respond = respond.clone();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 4096];
                let header_end = loop {
                    let n =
                        match tokio::time::timeout(Duration::from_secs(3), sock.read(&mut chunk))
                            .await
                        {
                            Ok(Ok(n)) => n,
                            _ => 0,
                        };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                let content_length: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                while buf.len() < header_end + content_length {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
                let first_line = String::from_utf8_lossy(&buf[..header_end.min(120)]).to_string();
                if first_line.contains("POST") {
                    post_count.fetch_add(1, Ordering::SeqCst);
                    bodies.lock().await.push(body.clone());
                }
                let resp = respond(&body, &buf[..header_end]);
                for c in resp.chunks(64) {
                    let _ = sock.write_all(c).await;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            });
        }
    });
    CountingMock {
        post_count,
        bodies,
        handle,
        port,
    }
}

/// SSE 200 响应包装。
pub fn sse_ok(body: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}")
        .into_bytes()
}

/// JSON 错误响应(HTTP 非 2xx)。
pub fn http_err(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} Err\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}"
    )
    .into_bytes()
}

pub fn delta(text: &str) -> String {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n")
}

pub fn finish_stop(remote: &str) -> String {
    format!("data: {{\"id\":\"{remote}\",\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n")
}

pub fn finish_reason(reason: &str) -> String {
    format!("data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"{reason}\"}}]}}\n\n")
}

pub fn usage_chunk() -> String {
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":7},\"cost\":\"0.0049\"}\n\n".to_string()
}

/// 等待条件成立或超时(毫秒)。
pub async fn wait_for<F: Fn() -> bool>(f: F, timeout_ms: u64) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    while tokio::time::Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    f()
}

/// 预留端口(避免 fixed port 冲突)。
pub async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

pub fn addr(port: u16) -> SocketAddr {
    ([127, 0, 0, 1], port).into()
}
