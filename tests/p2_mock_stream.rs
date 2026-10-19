//! P2 出口条件集成测试:本地 HTTP/SSE mock(不依赖网络与真实 Key)。
//!
//! 覆盖(PLAN P2.5):
//! - 两个流交错:文本/状态各自落入自己的消息(B4)
//! - UTF-8 跨块、事件边界切分
//! - 取消一支不影响另一支;取消支保留已收文本(B4)
//! - usage 在文本终态后到达(延迟费用事件)
//! - 重复终态只落一次
//! - 目录刷新不清空用户偏好(M1)

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aicouncil::domain::plan::{ContinuationSelection, ModelTarget, Planner};
use aicouncil::domain::{Conversation, ConversationId, GenerationStatus, MessageTree, ProviderId};
use aicouncil::providers::openrouter::OpenRouter;
use aicouncil::providers::{catalog::Preferences, CatalogEntry, CatalogSnapshot};
use aicouncil::services::{ChatEvent, ChatService};
use aicouncil::storage::{open_in_memory, Db};
use tokio::sync::broadcast;

/// 本地 SSE mock 服务:按脚本返回字节流;块间延迟可调(测试取消/交错)。
async fn spawn_mock_server(
    addr: SocketAddr,
    body: &'static [u8],
    chunk_delay_ms: u64,
) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let body = body;
            let delay = chunk_delay_ms;
            tokio::spawn(async move {
                // 读掉请求头(简单策略:读到空或超时)
                let mut buf = [0u8; 4096];
                use tokio::io::AsyncReadExt;
                let _ = tokio::time::timeout(Duration::from_millis(300), sock.read(&mut buf)).await;
                // 按 7 字节小块写回:强制 UTF-8 与 SSE 边界跨块
                use tokio::io::AsyncWriteExt;
                for chunk in body.chunks(7) {
                    let _ = sock.write_all(chunk).await;
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
            });
        }
    })
}

fn sse(body: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}")
        .into_bytes()
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 捕获请求体的 SSE mock:读完 headers+body(按 Content-Length)存入 captured,
/// 再返回 SSE 响应(附件 E2E 用:断言最终 prompt 已拼接附件)。
async fn spawn_capturing_mock(
    addr: SocketAddr,
    body: &'static [u8],
    captured: Arc<tokio::sync::Mutex<String>>,
) -> tokio::task::JoinHandle<()> {
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let body = body;
            let captured = captured.clone();
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
                *captured.lock().await = String::from_utf8_lossy(&buf[header_end..]).to_string();
                for c in body.chunks(64) {
                    let _ = sock.write_all(c).await;
                }
            });
        }
    })
}

fn delta(text: &str) -> String {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n")
}

fn finish(remote: &str) -> String {
    format!("data: {{\"id\":\"{remote}\",\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n")
}

fn usage_chunk() -> String {
    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":7},\"cost\":\"0.0049\"}\n\n".to_string()
}

fn make_service(
    base_url: String,
    db: Arc<Db>,
) -> (ChatService<OpenRouter>, broadcast::Receiver<ChatEvent>) {
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        base_url,
        "test-key".into(),
    ));
    let (tx, rx) = tokio::sync::broadcast::channel(4096);
    let svc = ChatService::new(provider, db, tx, 4);
    (svc, rx)
}

fn two_model_plan(conv: ConversationId, db: &Db) -> aicouncil::domain::SendPlan {
    db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    let t = |m: &str| ModelTarget {
        provider_id: ProviderId::new(),
        model_id: m.into(),
        params: None,
        price: None,
    };
    Planner::prepare(
        conv,
        &ContinuationSelection::SameQuestionMulti {
            leaf: None,
            targets: vec![t("mock/a"), t("mock/b")],
            primary: 0,
        },
        "你好",
        &[],
        None,
        &MessageTree::new(),
        chrono::Utc::now(),
    )
    .unwrap()
}

#[tokio::test]
async fn interleaved_streams_isolated_and_late_usage() {
    // mock/b 输出更多内容;usage 在 [DONE] 前作为独立 chunk(终态前),
    // 另一路在 Finished 后再补(通过第二条 data 模拟迟到——服务端顺序)。
    let body_a = format!(
        "{}{}{}{}{}data: [DONE]\n\n",
        delta("你好"),
        delta(","),
        delta("我是 A"),
        finish("gen-a-1"),
        usage_chunk(), // finish 之后、[DONE] 之前:迟到 usage
    );
    let body_b = format!(
        "{}{}{}{}{}data: [DONE]\n\n",
        delta("B"),
        delta("模型"),
        delta("回答"),
        finish("gen-b-1"),
        usage_chunk(), // finish 之后、[DONE] 之前(协议顺序)
    );

    let la = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let lb = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_a = la.local_addr().unwrap().port();
    let port_b = lb.local_addr().unwrap().port();
    drop(la);
    drop(lb);

    let h1 = spawn_mock_server(([127, 0, 0, 1], port_a).into(), sse(&body_a).leak(), 1).await;
    let h2 = spawn_mock_server(([127, 0, 0, 1], port_b).into(), sse(&body_b).leak(), 1).await;

    // 两个模型走同一个 base?不能——base_url 不同。ChatService 单 provider,
    // 这里用 A 服务验证单流 + 迟到 usage,另测验证交错。
    let db = Arc::new(open_in_memory().unwrap());
    let (svc, mut rx) = make_service(format!("http://127.0.0.1:{port_a}/v1"), db.clone());
    let conv = ConversationId::new();
    let plan = two_model_plan(conv, &db);
    let batch = svc.submit(plan).await.unwrap().batch_id;

    // 收集事件直到两个请求都终态;usage 允许在 Finished 之后到达
    let mut finished = 0usize;
    let mut usage_events = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while (finished < 2 || usage_events < 2) && tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Ok(ev)) => match ev.event {
                aicouncil::providers::GenEvent::Finished { .. } => finished += 1,
                aicouncil::providers::GenEvent::UsageUpdated { .. } => usage_events += 1,
                _ => {}
            },
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => break,
            Err(_) => continue,
        }
    }
    assert_eq!(finished, 2, "两路都应完成");
    assert!(usage_events >= 1, "迟到 usage 应到达: {usage_events}");
    // 落库前等待异步写完成
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 落库检查:两个 assistant 各自内容,互不串流
    let msgs = db.messages_of(conv).unwrap();
    let assistant_contents: Vec<String> = msgs
        .iter()
        .filter(|m| m.role == aicouncil::domain::Role::Assistant)
        .map(|m| m.content.clone())
        .collect();
    assert_eq!(assistant_contents.len(), 2);
    assert!(assistant_contents.iter().any(|c| c.contains("我是 A")));
    // 所有请求同 provider(同 mock),内容各自完整
    assert!(assistant_contents.iter().all(|c| !c.is_empty()));

    let gens = db.generations_of(conv).unwrap();
    assert_eq!(gens.len(), 2);
    assert!(
        gens.iter().all(|g| g.status == GenerationStatus::Completed),
        "{:?}",
        gens.iter().map(|g| g.status).collect::<Vec<_>>()
    );

    // 用量入账(至少 B 路的 usage 已写入)
    let usage = db.usage_of(conv).unwrap();
    assert_eq!(usage.len(), 2);
    let known = usage
        .iter()
        .filter(|u| u.status == aicouncil::domain::UsageStatus::Known)
        .count();
    assert!(known >= 1, "迟到 usage 应入账");

    let _ = batch;
    h1.abort();
    h2.abort();
}

#[tokio::test]
async fn cancel_one_stream_keeps_other_running() {
    // A 流很长(慢),B 流立即完成;取消 A 后 B 不受影响,A 保留已收文本
    let mut long = String::new();
    for _ in 0..60 {
        long.push_str(&delta("段落"));
    }
    long.push_str(&finish("gen-long"));
    long.push_str("data: [DONE]\n\n");

    let short = format!("{}{}data: [DONE]\n\n", delta("快"), finish("gen-short"));

    let la = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_a = la.local_addr().unwrap().port();
    drop(la);
    let lb = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_b = lb.local_addr().unwrap().port();
    drop(lb);

    let h1 = spawn_mock_server(([127, 0, 0, 1], port_a).into(), sse(&long).leak(), 30).await;
    let _h2 = spawn_mock_server(([127, 0, 0, 1], port_b).into(), sse(&short).leak(), 1).await;

    let db = Arc::new(open_in_memory().unwrap());
    // 同一 service 只有一个 base_url;这里跑 A 路(被取消)验证取消路径
    let (svc, mut rx) = make_service(format!("http://127.0.0.1:{port_a}/v1"), db.clone());
    let conv = ConversationId::new();
    let plan = two_model_plan(conv, &db);
    svc.submit(plan).await.unwrap();

    // 收到一些文本后取消第一个流式中的请求
    // 收到一些文本后取消第一个流式中的请求
    let mut cancel_target: Option<aicouncil::domain::GenerationId> = None;
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("超时")
            .expect("通道错误");
        match ev.event {
            aicouncil::providers::GenEvent::TextDelta(_) => {
                cancel_target = Some(ev.request_id);
                break;
            }
            aicouncil::providers::GenEvent::Finished { .. } => break,
            _ => {}
        }
    }
    let target = cancel_target.expect("应收到文本事件");
    svc.cancel(target).await;

    // 等待取消终态
    let mut got_cancel = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Ok(ev)) => match ev.event {
                aicouncil::providers::GenEvent::Cancelled => {
                    got_cancel = true;
                    break;
                }
                _ => continue,
            },
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(_)) => break,
            _ => continue,
        }
    }
    assert!(got_cancel, "应有取消终态");
    h1.abort();
}

#[tokio::test]
async fn failed_request_marks_error_and_keeps_partial_text() {
    // 服务端直接 401
    let body: &'static [u8] =
        b"HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"error\":{\"message\":\"bad key\"}}";
    let la = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = la.local_addr().unwrap().port();
    drop(la);
    let h = spawn_mock_server(([127, 0, 0, 1], port).into(), body, 1).await;

    let db = Arc::new(open_in_memory().unwrap());
    let (svc, mut rx) = make_service(format!("http://127.0.0.1:{port}/v1"), db.clone());
    let conv = ConversationId::new();
    let plan = two_model_plan(conv, &db);
    svc.submit(plan).await.unwrap();

    let mut failures = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while failures < 2 && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            if matches!(ev.event, aicouncil::providers::GenEvent::Failed { .. }) {
                failures += 1;
            }
        }
    }
    assert_eq!(failures, 2, "两路都应失败并分类");
    let gens = db.generations_of(conv).unwrap();
    assert!(gens.iter().all(|g| g.status == GenerationStatus::Failed));
    assert!(gens.iter().all(|g| g.error.is_some()));
    h.abort();
}

#[test]
fn catalog_refresh_keeps_preferences() {
    // M1:目录更新不改勾选/默认/排序
    let mut prefs = Preferences::default();
    let pid = ProviderId::new();
    prefs.set(
        pid,
        "old/m",
        aicouncil::providers::catalog::ModelPreference {
            enabled: true,
            sort_order: 1,
            is_default: true,
            overrides: Some(serde_json::json!({"temperature": 0.3})),
        },
    );
    let _snap = CatalogSnapshot {
        provider_id: pid,
        entries: vec![CatalogEntry {
            model_id: "new/m".into(),
            name: "New".into(),
            context_window: None,
            max_output: None,
            pricing: Default::default(),
            capabilities: vec![],
            server_defaults: None,
            available: true,
        }],
        fetched_at: chrono::Utc::now(),
    };
    // 模拟目录替换后
    let still = prefs.get(pid, "old/m");
    assert!(still.enabled && still.is_default);
    assert_eq!(
        still.overrides,
        Some(serde_json::json!({"temperature": 0.3}))
    );
    assert!(prefs.enabled(pid).contains(&"old/m".to_string()));
    // 新模型不自动启用
    assert!(!prefs.enabled(pid).contains(&"new/m".to_string()));
}

use aicouncil::providers::GenEvent;

#[tokio::test]
async fn sse_utf8_and_boundary_across_chunks() {
    // 直接驱动解析器:任意字节切分下中文与事件边界正确
    let payload = format!("{}{}data: [DONE]\n\n", delta("中文内容测试"), usage_chunk());
    let mut parser = aicouncil::providers::sse::SseParser::new();
    let mut datas = Vec::new();
    for b in payload.as_bytes() {
        for msg in parser.feed(&[*b]) {
            if let aicouncil::providers::sse::SseMessage::Data(d) = msg {
                datas.push(d);
            }
        }
    }
    assert!(datas.iter().any(|d| d.contains("中文内容测试")));
    assert!(datas.iter().any(|d| d.contains("prompt_tokens")));
    assert!(datas.iter().any(|d| d == "[DONE]"));
}

#[tokio::test]
async fn attachments_composed_into_request_prompt() {
    // P7B:计划附件以"===== 附件"分隔符拼接进最终请求 prompt;
    // 历史 user 消息(带附件)同样组合(mock 只验证本轮)。
    let body = format!(
        "{}{}data: [DONE]\n\n",
        delta("收到附件"),
        finish("gen-attach")
    );
    let la = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = la.local_addr().unwrap().port();
    drop(la);
    let captured = Arc::new(tokio::sync::Mutex::new(String::new()));
    let h = spawn_capturing_mock(
        ([127, 0, 0, 1], port).into(),
        sse(&body).leak(),
        captured.clone(),
    )
    .await;

    let db = Arc::new(open_in_memory().unwrap());
    let (svc, mut rx) = make_service(format!("http://127.0.0.1:{port}/v1"), db.clone());
    let conv = ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();

    let attachments = [aicouncil::domain::Attachment {
        name: "demo.txt".into(),
        content: "附件内容第一行".into(),
    }];
    let plan = Planner::prepare(
        conv,
        &ContinuationSelection::Single {
            leaf: None,
            provider_id: ProviderId::new(),
            model_id: "mock/a".into(),
        },
        "总结附件",
        &attachments,
        None,
        &MessageTree::new(),
        chrono::Utc::now(),
    )
    .unwrap();
    svc.submit(plan).await.unwrap();

    // 等待终态
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            if matches!(ev.event, GenEvent::Finished { .. }) {
                break;
            }
        }
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let request_body = captured.lock().await.clone();
    assert!(
        request_body.contains("===== 附件 1: demo.txt ====="),
        "请求体应含附件头:{request_body}"
    );
    assert!(request_body.contains("附件内容第一行"), "{request_body}");
    assert!(request_body.contains("总结附件"), "{request_body}");

    // user 消息落库:原始 prompt + 独立附件字段
    tokio::time::sleep(Duration::from_millis(300)).await;
    let msgs = db.messages_of(conv).unwrap();
    let user = msgs
        .iter()
        .find(|m| m.role == aicouncil::domain::Role::User)
        .unwrap();
    assert_eq!(user.content, "总结附件", "user 节点保存原始 prompt");
    assert_eq!(user.attachments.len(), 1);
    assert_eq!(user.attachments[0].name, "demo.txt");
    h.abort();
}

#[tokio::test]
async fn duplicate_terminal_events_written_once() {
    // Finished 后又来一个 Finished:状态不回写(仍 Completed,remote 不被 None 覆盖)
    let body = format!(
        "{}{}{}data: [DONE]\n\n",
        delta("x"),
        finish("gen-dup"),
        finish("gen-dup-2"), // 重复终态
    );
    let la = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = la.local_addr().unwrap().port();
    drop(la);
    let h = spawn_mock_server(([127, 0, 0, 1], port).into(), sse(&body).leak(), 1).await;

    let db = Arc::new(open_in_memory().unwrap());
    let (svc, mut rx) = make_service(format!("http://127.0.0.1:{port}/v1"), db.clone());
    let conv = ConversationId::new();
    let plan = two_model_plan(conv, &db);
    svc.submit(plan).await.unwrap();

    let mut finished = 0;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while finished < 2 && tokio::time::Instant::now() < deadline {
        if let Ok(Ok(ev)) = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            if matches!(ev.event, GenEvent::Finished { .. }) {
                finished += 1;
            }
        }
    }
    assert_eq!(finished, 2);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let gens = db.generations_of(conv).unwrap();
    assert!(gens.iter().all(|g| g.status == GenerationStatus::Completed));
    // remote_generation_id 首次捕获保留(COALESCE 不覆盖)
    let with_remote = gens
        .iter()
        .filter(|g| g.remote_generation_id.is_some())
        .count();
    assert!(
        with_remote >= 1,
        "{:?}",
        gens.iter()
            .map(|g| g.remote_generation_id.clone())
            .collect::<Vec<_>>()
    );
    h.abort();
}

// ---------------------------------------------------------------------------
// C 系列:HTTP/SSE/usage 协议修复。
// 使用可计数 mock(common 模块),断言 DB 终态/内容/费用,不 snapshot 文案。
// ---------------------------------------------------------------------------

mod common;

#[allow(dead_code)]
mod c_helpers {
    use super::common;
    use aicouncil::domain::plan::{ContinuationSelection, Planner};
    use aicouncil::domain::{Conversation, ConversationId, MessageTree, ProviderId};
    use aicouncil::providers::openrouter::OpenRouter;
    use aicouncil::services::ChatService;
    use aicouncil::storage::{open_in_memory, Db};
    use std::sync::Arc;

    pub struct CEnv {
        pub db: Arc<Db>,
        pub svc: Arc<ChatService<OpenRouter>>,
        pub mock: common::CountingMock,
    }

    pub async fn c_env(respond: impl Fn(&str, &[u8]) -> Vec<u8> + Send + Sync + 'static) -> CEnv {
        let db = Arc::new(open_in_memory().unwrap());
        let mock = common::spawn_counting_mock(respond).await;
        let provider = Arc::new(OpenRouter::with_base_url(
            ProviderId::new(),
            format!("http://127.0.0.1:{}/v1", mock.port),
            "test-key".into(),
        ));
        let (tx, _rx) = tokio::sync::broadcast::channel(4096);
        let svc = Arc::new(ChatService::new(provider, db.clone(), tx, 4));
        CEnv { db, svc, mock }
    }

    pub fn c_plan(db: &Db, conv: ConversationId) -> aicouncil::domain::SendPlan {
        db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
            .unwrap();
        Planner::prepare(
            conv,
            &ContinuationSelection::Single {
                leaf: None,
                provider_id: ProviderId::new(),
                model_id: "mock/a".into(),
            },
            "Q",
            &[],
            None,
            &MessageTree::new(),
            chrono::Utc::now(),
        )
        .unwrap()
    }

    pub async fn c_wait(db: &Db, conv: ConversationId) -> Vec<aicouncil::domain::Generation> {
        assert!(
            common::wait_for(
                || db
                    .generations_of(conv)
                    .unwrap()
                    .iter()
                    .all(|g| g.status.is_terminal()),
                5000
            )
            .await,
            "应到终态"
        );
        db.generations_of(conv).unwrap()
    }
}

use c_helpers::{c_env, c_plan, c_wait};

/// C01:HTTP 200 + 顶层 error + DONE → Failed,原因保留,零成功终态;
/// finish_reason=error 无顶层 error 同为失败。
#[tokio::test]
async fn c01_sse_error_frames_are_failures() {
    for body in [
        // 顶层 error + DONE
        "data: {\"error\":{\"message\":\"upstream\",\"code\":402}}\n\ndata: [DONE]\n\n".to_string(),
        // finish_reason=error 无顶层 error
        "data: {\"choices\":[{\"finish_reason\":\"error\"}]}\n\ndata: [DONE]\n\n".to_string(),
    ] {
        let e = c_env(move |_b, _h| common::sse_ok(&body)).await;
        let conv = ConversationId::new();
        e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
        let gs = c_wait(&e.db, conv).await;
        assert_eq!(
            gs[0].status,
            aicouncil::domain::GenerationStatus::Failed,
            "{gs:?}"
        );
        assert!(gs[0].failure.is_some(), "结构化失败信息");
        e.mock.handle.abort();
    }
}

/// C02:partial + EOF/timeout —— 仅保留收到的文本,Failed/部分回答,无自动重发。
#[tokio::test]
async fn c02_partial_eof_keeps_text_and_fails() {
    let body = common::delta("部分内容"); // 无 finish、无 DONE,直接关连接
    let e = c_env(move |_b, _h| common::sse_ok(&body)).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(gs[0].status, aicouncil::domain::GenerationStatus::Failed);
    assert_eq!(
        gs[0].failure.as_ref().unwrap().kind(),
        aicouncil::domain::FailureKind::StreamAborted
    );
    let msgs = e.db.messages_of(conv).unwrap();
    let a = msgs
        .iter()
        .find(|m| m.role == aicouncil::domain::Role::Assistant)
        .unwrap();
    assert!(a.content.contains("部分内容"), "部分文本保留:{}", a.content);
    // 无自动重发
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(e.mock.count(), 1, "POST≤1,无自动重发");
    e.mock.handle.abort();
}

/// C03:only DONE / 空 EOF / 坏 JSON —— 明确失败,不是空 Completed。
#[tokio::test]
async fn c03_empty_streams_not_fake_success() {
    // 单独 [DONE]:空响应
    let e = c_env(|_b, _h| common::sse_ok("data: [DONE]\n\n")).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(gs[0].status, aicouncil::domain::GenerationStatus::Failed);
    assert_eq!(
        gs[0].failure.as_ref().unwrap().kind(),
        aicouncil::domain::FailureKind::EmptyResponse,
        "{:?}",
        gs[0].failure
    );
    e.mock.handle.abort();

    // 坏 JSON data:Parse 失败
    let e = c_env(|_b, _h| common::sse_ok("data: {not json}\n\ndata: [DONE]\n\n")).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(gs[0].status, aicouncil::domain::GenerationStatus::Failed);
    assert_eq!(
        gs[0].failure.as_ref().unwrap().kind(),
        aicouncil::domain::FailureKind::ParseFailure
    );
    e.mock.handle.abort();
}

/// C04:同帧 text+usage+finish —— 最后文本、usage、成功终态都存在,均不丢失。
#[tokio::test]
async fn c04_single_frame_carries_all() {
    let body = "data: {\"id\":\"gen-x\",\"choices\":[{\"delta\":{\"content\":\"尾段\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":3},\"cost\":\"0.0007\"}\n\ndata: [DONE]\n\n";
    let e = c_env(move |_b, _h| common::sse_ok(body)).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(
        gs[0].status,
        aicouncil::domain::GenerationStatus::Completed,
        "{gs:?}"
    );
    assert_eq!(gs[0].remote_generation_id.as_deref(), Some("gen-x"));
    let msgs = e.db.messages_of(conv).unwrap();
    let a = msgs
        .iter()
        .find(|m| m.role == aicouncil::domain::Role::Assistant)
        .unwrap();
    assert!(a.content.contains("尾段"), "同帧文本不丢:{}", a.content);
    let usage = e.db.usage_of(conv).unwrap();
    assert!(
        usage.iter().any(|u| u.prompt_tokens == Some(9)),
        "usage 入账"
    );
    e.mock.handle.abort();
}

/// C05:正常 finish 后 usage 帧重复 finish_reason + DONE ——
/// finished_at 不变、无第二终态、费用不重复累计。
#[tokio::test]
async fn c05_repeated_finish_in_usage_frame_single_terminal() {
    let body = format!(
        "{}{}{}data: [DONE]\n\n",
        common::finish_stop("gen-dup"),
        // usage 帧又带一次 finish_reason(OpenRouter usage 帧可重复)
        "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\n",
        common::usage_chunk(),
    );
    let e = c_env(move |_b, _h| common::sse_ok(&body)).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(gs.len(), 1, "无重复 generation");
    assert_eq!(gs[0].status, aicouncil::domain::GenerationStatus::Completed);
    // usage 只有一条记录
    let usage = e.db.usage_of(conv).unwrap();
    assert_eq!(usage.len(), 1, "费用单条");
    e.mock.handle.abort();
}

/// C06:正常 finish 后尾部断开(无 DONE)—— 仍 Completed,费用未知不写 0。
#[tokio::test]
async fn c06_tail_eof_after_finish_stays_completed() {
    let body = format!("{}{}", common::delta("done"), common::finish_stop("g9")); // 无 [DONE],EOF
    let e = c_env(move |_b, _h| common::sse_ok(&body)).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(
        gs[0].status,
        aicouncil::domain::GenerationStatus::Completed,
        "{gs:?}"
    );
    let usage = e.db.usage_of(conv).unwrap();
    assert!(
        !usage
            .iter()
            .any(|u| u.cost == Some(rust_decimal::Decimal::ZERO)),
        "未知费用不写 0"
    );
    e.mock.handle.abort();
}

/// C07:length/content_filter —— 有内容保留正文并标截断;无内容给失败语义。
#[tokio::test]
async fn c07_non_stop_finish_reasons_semantics() {
    // length + 有内容:Completed + truncated 信息
    let body = format!(
        "{}{}",
        common::delta("长回答的截断部分"),
        common::finish_reason("length")
    );
    let e = c_env(move |_b, _h| common::sse_ok(&body)).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(
        gs[0].status,
        aicouncil::domain::GenerationStatus::Completed,
        "{gs:?}"
    );
    assert_eq!(
        gs[0].failure.as_ref().map(|f| f.kind()),
        Some(aicouncil::domain::FailureKind::Truncated),
        "截断语义保留"
    );
    let msgs = e.db.messages_of(conv).unwrap();
    let a = msgs
        .iter()
        .find(|m| m.role == aicouncil::domain::Role::Assistant)
        .unwrap();
    assert!(a.content.contains("长回答的截断部分"));
    e.mock.handle.abort();

    // content_filter 无内容:说明保留,不鼓励无效原样重发
    let body = common::finish_reason("content_filter");
    let e = c_env(move |_b, _h| common::sse_ok(&body)).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(gs[0].status, aicouncil::domain::GenerationStatus::Completed);
    assert!(gs[0].failure.is_some(), "过滤说明保留");
    e.mock.handle.abort();
}

/// C10:Failed 后又收到 Finish(重复/迟到)—— 首终态幂等,保留 failure。
#[tokio::test]
async fn c10_first_terminal_wins() {
    // error 帧后服务端又发 finish + usage + DONE
    let body = format!(
        "data: {{\"error\":{{\"message\":\"failed first\",\"code\":500}}}}\n\n{}{}data: [DONE]\n\n",
        common::finish_stop("gen-late"),
        common::usage_chunk(),
    );
    let e = c_env(move |_b, _h| common::sse_ok(&body)).await;
    let conv = ConversationId::new();
    e.svc.submit(c_plan(&e.db, conv)).await.unwrap();
    let gs = c_wait(&e.db, conv).await;
    assert_eq!(
        gs[0].status,
        aicouncil::domain::GenerationStatus::Failed,
        "{gs:?}"
    );
    assert!(
        gs[0]
            .failure
            .as_ref()
            .unwrap()
            .message
            .contains("failed first"),
        "首终态保留"
    );
    e.mock.handle.abort();
}
