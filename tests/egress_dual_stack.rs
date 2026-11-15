//! IPv6 出口门禁回归(真实生产路径,无公网、无真实费用):
//! 使用真 `GeoIpGuard`(双族并行检测)+ 真 `OpenRouter` 生成客户端 +
//! 真 `ChatService` admission/dispatch,验证:
//! - 原"v4 放行、v6 泄漏"被重现并拦截(生成 POST=0、零落库);
//! - 许可只授权已核验地址族,发送连接器对未核验族物理不可达;
//! - 本地 IPv4 HTTP 代理兼容:检测/生成都走代理,不误探测裸直连;
//! - 无 IPv6 / 单族网络不被误拦;检测请求无凭据。
//!
//! 说明:族过滤作用于域名解析(hyper-util 对 IP 字面量主机跳过解析器),
//! 故 mock 一律经 `localhost` 域名访问,服务器按族绑定 127.0.0.1 或
//! [::1](macOS 本地 loopback 双族,不依赖公网 IPv6)。

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aicouncil::domain::plan::{ContinuationSelection, ModelTarget, Planner};
use aicouncil::domain::{Conversation, ConversationId, MessageTree, ProviderId};
use aicouncil::providers::cancel::CancellationToken;
use aicouncil::providers::openrouter::OpenRouter;
use aicouncil::services::chat::{AdmissionError, SubmitError};
use aicouncil::services::egress::{
    EgressGuard, EgressPermit, EgressVerdict, GeoIpGuard, SourceSpec,
};
use aicouncil::services::network::{NetworkConfig, NetworkLimits, RouteFamilies};
use aicouncil::services::ChatService;
use aicouncil::storage::{open_in_memory, Db};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;

// ---------- 单族 mock 基础设施 ----------

enum Resp {
    Text(String),
    Status(u16),
    /// 接受连接但不响应(超时/取消模拟)。
    Hang,
}

impl Clone for Resp {
    fn clone(&self) -> Self {
        match self {
            Self::Text(s) => Self::Text(s.clone()),
            Self::Status(c) => Self::Status(*c),
            Self::Hang => Self::Hang,
        }
    }
}

struct TraceMock {
    port: u16,
    hits: Arc<AtomicU64>,
    /// 每次请求一条记录:METHOD /path;header...
    requests: Arc<tokio::sync::Mutex<Vec<String>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl TraceMock {
    fn hit_count(&self) -> u64 {
        self.hits.load(Ordering::SeqCst)
    }

    async fn all_requests(&self) -> Vec<String> {
        self.requests.lock().await.clone()
    }

    fn abort(&self) {
        self.handle.abort();
    }
}

/// 按单族地址(`127.0.0.1` 或 `[::1]`)绑定并伺服检测源响应。
async fn spawn_trace_mock(bind: &str, script: Vec<(&'static str, Resp)>) -> TraceMock {
    let listener = tokio::net::TcpListener::bind(format!("{bind}:0"))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicU64::new(0));
    let requests: Arc<tokio::sync::Mutex<Vec<String>>> =
        Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let script: Vec<(String, Resp)> = script
        .into_iter()
        .map(|(p, r)| (p.to_string(), r))
        .collect();
    let hits2 = hits.clone();
    let req2 = requests.clone();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let hits = hits2.clone();
            let requests = req2.clone();
            let script = script.clone();
            tokio::spawn(async move {
                let head = match read_head(&mut sock).await {
                    Some(h) => h,
                    None => return,
                };
                let first = head.lines().next().unwrap_or_default().to_string();
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts
                    .next()
                    .unwrap_or("/")
                    .trim_start_matches('/')
                    .split('?')
                    .next()
                    .unwrap_or("")
                    .to_string();
                hits.fetch_add(1, Ordering::SeqCst);
                requests.lock().await.push(format!(
                    "{method} /{path};{}",
                    head.lines().skip(1).collect::<Vec<_>>().join(";")
                ));
                let resp = script
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, r)| r)
                    .unwrap_or(&Resp::Status(404));
                match resp {
                    Resp::Text(body) => {
                        let out = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = sock.write_all(out.as_bytes()).await;
                    }
                    Resp::Status(code) => {
                        let out = format!(
                            "HTTP/1.1 {code} Err\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        );
                        let _ = sock.write_all(out.as_bytes()).await;
                    }
                    Resp::Hang => {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                }
            });
        }
    });
    TraceMock {
        port,
        hits,
        requests,
        handle,
    }
}

/// 最小本地 HTTP 代理:接受 absolute-form 请求(GET/POST),
/// 改写为 origin-form 加 `X-Forwarded-For` 转发到目标,回传响应。
/// 返回代理端口;命中计数由调用方传入。
async fn spawn_http_proxy(hits: Arc<AtomicU64>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut client, _)) = listener.accept().await else {
                break;
            };
            let hits = hits.clone();
            tokio::spawn(async move {
                // 读取完整请求(头 + 按 Content-Length 的体)
                let (head, body) = match read_request(&mut client).await {
                    Some(r) => r,
                    None => return,
                };
                let mut lines = head.split("\r\n");
                let request_line = lines.next().unwrap_or_default().to_string();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or("GET").to_string();
                let uri = parts.next().unwrap_or("").to_string();
                let rest = match uri.strip_prefix("http://") {
                    Some(r) => r.to_string(),
                    None => return, // 非 absolute-form:不是经代理的请求
                };
                hits.fetch_add(1, Ordering::SeqCst);
                let (authority, path) = match rest.find('/') {
                    Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
                    None => (rest.clone(), "/".to_string()),
                };
                // 解析目标(authority 可能是域名,如 localhost)
                let addrs = match tokio::net::lookup_host(authority.as_str()).await {
                    Ok(a) => a.collect::<Vec<_>>(),
                    Err(_) => return,
                };
                let mut upstream = None;
                for addr in addrs {
                    if let Ok(u) = tokio::net::TcpStream::connect(addr).await {
                        upstream = Some(u);
                        break;
                    }
                }
                let mut upstream = match upstream {
                    Some(u) => u,
                    None => return,
                };
                // 转发头:保留原头(除 Host/Connection),强制关闭复用
                let mut fwd = format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\nX-Forwarded-For: aicouncil-test-proxy\r\nConnection: close\r\n");
                for line in lines {
                    let low = line.to_ascii_lowercase();
                    if line.is_empty()
                        || low.starts_with("host:")
                        || low.starts_with("connection:")
                        || low.starts_with("proxy-connection:")
                        || low.starts_with("content-length:")
                    {
                        continue;
                    }
                    fwd.push_str(line);
                    fwd.push_str("\r\n");
                }
                fwd.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
                if upstream.write_all(fwd.as_bytes()).await.is_err() {
                    return;
                }
                if !body.is_empty() && upstream.write_all(&body).await.is_err() {
                    return;
                }
                // 回传目标响应直至 EOF
                let mut buf = [0u8; 4096];
                loop {
                    match upstream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if client.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
    });
    port
}

async fn read_head(sock: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Some(String::from_utf8_lossy(&buf).to_string());
        }
        if buf.len() > 1024 * 1024 {
            return None;
        }
    }
}

/// 读取一个完整 HTTP 请求(头 + Content-Length 体)。
async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > 1024 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let cl: usize = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("Content-Length:")
                .or_else(|| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < cl {
        let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(cl);
    Some((head, body))
}

fn leak_url(u: String) -> &'static str {
    Box::leak(u.into_boxed_str())
}

fn trace_source(url: &str) -> SourceSpec {
    SourceSpec {
        name: leak_url(format!("mock-trace:{url}")),
        url: leak_url(url.to_string()),
        parse: aicouncil::services::egress::parse_openrouter_trace,
    }
}

fn limits(source_ms: u64, total_ms: u64) -> NetworkLimits {
    NetworkLimits {
        egress_source_timeout: Duration::from_millis(source_ms),
        egress_total_timeout: Duration::from_millis(total_ms),
        ..Default::default()
    }
}

// ---------- 生产路径组装 ----------

fn anthropic_target(model: &str) -> ModelTarget {
    ModelTarget {
        provider_id: ProviderId::new(),
        model_id: model.to_string(),
        params: None,
        price: None,
    }
}

fn make_plan(conv: ConversationId, targets: Vec<ModelTarget>) -> aicouncil::domain::SendPlan {
    Planner::prepare(
        conv,
        &ContinuationSelection::SameQuestionMulti {
            leaf: None,
            targets,
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

fn sse_ok_response() -> Vec<u8> {
    common::sse_ok(&format!(
        "{}{}data: [DONE]\n\n",
        common::delta("ok"),
        common::finish_stop("gen-ok")
    ))
}

struct Env {
    db: Arc<Db>,
    svc: ChatService<OpenRouter>,
    mock: common::CountingMock,
    mocks: Vec<TraceMock>,
}

impl Env {
    async fn abort_all(&self) {
        for m in &self.mocks {
            m.abort();
        }
        self.mock.handle.abort();
    }
}

/// 组装:双族检测源 + 生成目标(CountingMock,经 localhost 域名访问)。
async fn build_env(guard: GeoIpGuard, mocks: Vec<TraceMock>) -> Env {
    let db = Arc::new(open_in_memory().unwrap());
    let mock = common::spawn_counting_mock(move |_b, _h| sse_ok_response()).await;
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        format!("http://localhost:{}/v1", mock.port),
        "test-key".into(),
    ));
    let (tx, _rx) = broadcast::channel(256);
    let svc = ChatService::new(provider, db.clone(), tx, 4).with_egress_guard(Arc::new(guard));
    Env {
        db,
        svc,
        mock,
        mocks,
    }
}

async fn new_conv(db: &Db) -> ConversationId {
    let conv = ConversationId::new();
    db.upsert_conversation(&Conversation::new(conv, chrono::Utc::now()))
        .unwrap();
    conv
}

fn counts(db: &Db, conv: ConversationId) -> (usize, usize, usize) {
    let gens = db.generations_of(conv).unwrap().len();
    let msgs = db.messages_of(conv).unwrap().len();
    let batches: i64 = db
        .lock()
        .query_row("SELECT COUNT(*) FROM send_batches", [], |r| r.get(0))
        .unwrap();
    (batches as usize, gens, msgs)
}

async fn wait_generation_failed(db: &Db, conv: ConversationId) -> bool {
    common::wait_for(
        || {
            db.generations_of(conv)
                .unwrap()
                .first()
                .map(|g| g.status.is_terminal())
                .unwrap_or(false)
        },
        8000,
    )
    .await
}

// ---------- 关键回归 ----------

/// 关键回归(计划验收核心):v4 US + v6 CN —— 原"v4 放行、v6 泄漏"
/// 必须在生产路径被拦截;生成 POST=0;零落库;两族都被实际探测;
/// 阻断证据指明 v6 路径、国家与出口 IP。
#[tokio::test]
async fn v4_us_v6_cn_blocks_with_zero_generation_post() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let v6 = spawn_trace_mock(
        "[::1]",
        vec![("trace", Resp::Text("ip=2408:8207:db0::a\nloc=CN\n".into()))],
    )
    .await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!("http://localhost:{}/trace", v6.port))],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    let err = env.svc.submit(plan).await.unwrap_err();
    match &err {
        SubmitError::Admission(AdmissionError::EgressBlocked { country, detail }) => {
            assert_eq!(country.as_deref(), Some("CN"));
            assert!(
                detail.contains("families=v6"),
                "证据须指明 v6 路径:{detail}"
            );
            assert!(
                detail.contains("2408:8207:db0::a"),
                "证据含 v6 出口 IP:{detail}"
            );
        }
        other => panic!("v4 US + v6 CN 必须拦截:{other:?}"),
    }
    assert_eq!(env.mock.count(), 0, "生成 POST=0(修复 v6 泄漏)");
    assert_eq!(counts(&env.db, conv), (0, 0, 0), "零落库");
    for m in &env.mocks {
        assert!(m.hit_count() >= 1, "两族都应被实际探测");
    }
    env.abort_all().await;
}

/// v4 CN + v6 US:同样拦截,证据是 v4 路径(不被另一族覆盖)。
#[tokio::test]
async fn v4_cn_v6_us_blocked_not_overridden() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=HK\n".into()))],
    )
    .await;
    let v6 = spawn_trace_mock(
        "[::1]",
        vec![("trace", Resp::Text("ip=2606:4700::1\nloc=US\n".into()))],
    )
    .await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!("http://localhost:{}/trace", v6.port))],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    match env.svc.submit(plan).await.unwrap_err() {
        SubmitError::Admission(AdmissionError::EgressBlocked { country, detail }) => {
            assert_eq!(country.as_deref(), Some("HK"));
            assert!(detail.contains("families=v4"), "{detail}");
            assert!(detail.contains("IPv6 核验通过"), "另一族证据保留:{detail}");
        }
        other => panic!("v4 CN/HK 必须拦截:{other:?}"),
    }
    assert_eq!(env.mock.count(), 0);
    assert_eq!(counts(&env.db, conv), (0, 0, 0));
    env.abort_all().await;
}

/// 两族均非受限 → 放行并发送(生产判定 + 发送路径)。
#[tokio::test]
async fn both_families_verified_allowed_and_send_succeeds() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let v6 = spawn_trace_mock(
        "[::1]",
        vec![("trace", Resp::Text("ip=2606:4700::1\nloc=JP\n".into()))],
    )
    .await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!("http://localhost:{}/trace", v6.port))],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    env.svc.submit(plan).await.expect("两族合法应放行");
    assert!(
        common::wait_for(|| env.mock.count() == 1, 4000).await,
        "POST=1"
    );
    let bodies = env.mock.all_bodies().await;
    assert!(bodies[0].contains("anthropic/claude-opus-5.5"), "原模型 ID");
    env.abort_all().await;
}

/// v4 US + v6 可达但检测挂起:v6 不能当作"不存在"并入许可;
/// 判定 Allowed 但许可只含 v4 —— 生成连接器被限制到 v4 族。
/// 生成目标只有 v6 可达 → 发送必然失败且 POST=0,证明族约束生效。
#[tokio::test]
async fn v6_unverified_excluded_from_send_connector() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let v6hang = spawn_trace_mock("[::1]", vec![("trace", Resp::Hang)]).await;
    // 生成目标:v6-only(仅 ::1 监听)
    let genv6 = spawn_trace_mock("[::1]", vec![("sse", Resp::Status(404))]).await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!(
            "http://localhost:{}/trace",
            v6hang.port
        ))],
        limits(400, 3000),
    )
    .unwrap();
    let db = Arc::new(open_in_memory().unwrap());
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        format!("http://localhost:{}/v1", genv6.port),
        "test-key".into(),
    ));
    let (tx, _rx) = broadcast::channel(256);
    let svc = ChatService::new(provider, db.clone(), tx, 4).with_egress_guard(Arc::new(guard));
    let conv = new_conv(&db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    // admission 放行(v4 已核验);发送连接器排除 v6 → 到不了 v6-only 目标
    svc.submit(plan)
        .await
        .expect("admission 应放行(排除未核验族而非拦截)");
    assert!(
        wait_generation_failed(&db, conv).await,
        "生成应失败(不可达)"
    );
    let g = &db.generations_of(conv).unwrap()[0];
    assert_eq!(
        g.dispatch_phase,
        aicouncil::domain::DispatchPhase::DispatchAttempted,
        "已尝试发送但未到达(族约束生效)"
    );
    assert_eq!(genv6.hit_count(), 0, "v6 生成目标零连接(未核验族被排除)");
    for m in [&v4, &v6hang, &genv6] {
        m.abort();
    }
}

/// 对称:仅 v6 核验合法(US)→ 发送连接器限制到 v6 族;
/// 生成目标只有 v4 可达 → 必然失败,POST=0。
#[tokio::test]
async fn v6_only_verified_cannot_use_unverified_v4() {
    // v4 检测源指向仅 ::1 监听的端口:v4 族解析后不可达(可靠失败)
    let v6srv = spawn_trace_mock(
        "[::1]",
        vec![("trace", Resp::Text("ip=2606:4700::1\nloc=JP\n".into()))],
    )
    .await;
    let genv4 = spawn_trace_mock("127.0.0.1", vec![("sse", Resp::Status(404))]).await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!(
            "http://localhost:{}/trace",
            v6srv.port
        ))],
        vec![trace_source(&format!(
            "http://localhost:{}/trace",
            v6srv.port
        ))],
        limits(1000, 4000),
    )
    .unwrap();
    let db = Arc::new(open_in_memory().unwrap());
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        format!("http://localhost:{}/v1", genv4.port),
        "test-key".into(),
    ));
    let (tx, _rx) = broadcast::channel(256);
    let svc = ChatService::new(provider, db.clone(), tx, 4).with_egress_guard(Arc::new(guard));
    let conv = new_conv(&db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    svc.submit(plan).await.expect("v6 核验合法应放行");
    assert!(
        wait_generation_failed(&db, conv).await,
        "生成应失败(v4 不可达)"
    );
    assert_eq!(genv4.hit_count(), 0, "未核验 v4 族对生成不可达");
    v6srv.abort();
    genv4.abort();
}

/// 仅 IPv4 可用(v6 探测 429):不误拦合法网络,发送成功(POST=1)。
#[tokio::test]
async fn v4_only_usable_network_not_misblocked_on_v6_429() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let v6 = spawn_trace_mock("[::1]", vec![("trace", Resp::Status(429))]).await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!("http://localhost:{}/trace", v6.port))],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    env.svc.submit(plan).await.expect("仅 IPv4 可用不被误拦");
    assert!(
        common::wait_for(|| env.mock.count() == 1, 4000).await,
        "POST=1"
    );
    env.abort_all().await;
}

/// 总预算回归 ①:v4 已核验 Allowed,v6 分支持续挂起**超过总预算**:
/// 已完成的 v4 结论必须保留(整体不得 Unknown),判定 Allowed 仅含
/// v4 族,发送成功(POST=1)。这是普通 IPv4-only / IPv6 黑洞网络
/// 不被误拦的关键。
#[tokio::test]
async fn v4_allowed_survives_v6_exceeding_total_budget() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    // v6:接受但不响应;单源超时(5s)**长于**总预算(700ms):
    // 只有截止期能结束该分支 —— 正是旧行为丢弃已完成族的场景
    let v6hang = spawn_trace_mock("[::1]", vec![("trace", Resp::Hang)]).await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!(
            "http://localhost:{}/trace",
            v6hang.port
        ))],
        limits(5000, 700),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6hang]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    let started = std::time::Instant::now();
    env.svc
        .submit(plan)
        .await
        .expect("已核验族结论应保留,不整体 Unknown");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(4000),
        "应在总预算截止后快速放行:{elapsed:?}"
    );
    assert!(
        common::wait_for(|| env.mock.count() == 1, 4000).await,
        "v4 族发送成功(POST=1)"
    );
    assert!(env.mocks[0].hit_count() >= 1, "v4 分支实际探测");
    env.abort_all().await;
}

/// 总预算回归 ②:v6 已核验 Blocked(CN),v4 分支超过总预算:
/// Blocked 结论必须保留(不可被另一族超时覆盖),拦截发生在生成前
/// (POST=0、零落库)。
#[tokio::test]
async fn v6_blocked_survives_v4_exceeding_total_budget() {
    let v6 = spawn_trace_mock(
        "[::1]",
        vec![("trace", Resp::Text("ip=2408:8207:db0::a\nloc=CN\n".into()))],
    )
    .await;
    let v4hang = spawn_trace_mock("127.0.0.1", vec![("trace", Resp::Hang)]).await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!(
            "http://localhost:{}/trace",
            v4hang.port
        ))],
        vec![trace_source(&format!("http://localhost:{}/trace", v6.port))],
        limits(5000, 700),
    )
    .unwrap();
    let env = build_env(guard, vec![v4hang, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    match env.svc.submit(plan).await.unwrap_err() {
        SubmitError::Admission(AdmissionError::EgressBlocked { country, detail }) => {
            assert_eq!(country.as_deref(), Some("CN"), "已完成 Blocked 族必须保留");
            assert!(detail.contains("families=v6"), "{detail}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(env.mock.count(), 0, "生成 POST=0");
    assert_eq!(counts(&env.db, conv), (0, 0, 0), "零落库");
    env.abort_all().await;
}

/// v6 主源 CN 权威:v6 备源 US 不生效,也不被 v4 US 覆盖;
/// 主源有效时备源零访问。
#[tokio::test]
async fn primary_v6_cn_authoritative_over_fallback_and_v4() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let v6 = spawn_trace_mock(
        "[::1]",
        vec![
            ("trace", Resp::Text("ip=2408:8207:db0::a\nloc=CN\n".into())),
            ("ipapi", Resp::Text("ip=2606:4700::99\nloc=US\n".into())),
        ],
    )
    .await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![
            trace_source(&format!("http://localhost:{}/trace", v6.port)),
            trace_source(&format!("http://localhost:{}/ipapi", v6.port)),
        ],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    match env.svc.submit(plan).await.unwrap_err() {
        SubmitError::Admission(AdmissionError::EgressBlocked { country, detail }) => {
            assert_eq!(country.as_deref(), Some("CN"), "v6 主源权威");
            assert!(detail.contains("families=v6"), "{detail}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(env.mock.count(), 0, "POST=0");
    let v6mock = &env.mocks[1];
    let reqs = v6mock.all_requests().await;
    assert!(
        reqs.iter().all(|r| r.contains("/trace")),
        "主源有效时备源零访问:{reqs:?}"
    );
    env.abort_all().await;
}

/// 本地 IPv4 HTTP 代理:检测与生成都走代理;不误探测裸直连;
/// 远端(经代理)返回 CN → 拦截,POST=0;本地代理是 v4 不导致失败。
#[tokio::test]
async fn local_ipv4_http_proxy_probes_via_proxy_not_direct() {
    // 代理远端"出口"(mock 目标)返回 CN
    let target = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=2408:8207:db0::a\nloc=CN\n".into()))],
    )
    .await;
    let proxy_hits = Arc::new(AtomicU64::new(0));
    let proxy_port = spawn_http_proxy(proxy_hits.clone()).await;
    let cfg = NetworkConfig {
        proxy: Some(format!("http://127.0.0.1:{proxy_port}")),
        limits: limits(2000, 5000),
    };
    let url = format!("http://localhost:{}/trace", target.port);
    let guard = GeoIpGuard::with_sources(
        &cfg,
        vec![trace_source(&url)],
        vec![trace_source(&url)],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![target]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    match env.svc.submit(plan).await.unwrap_err() {
        SubmitError::Admission(AdmissionError::EgressBlocked { country, detail }) => {
            assert_eq!(country.as_deref(), Some("CN"), "代理路径出口判定生效");
            assert!(detail.contains("2408:8207:db0::a"), "{detail}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(env.mock.count(), 0, "POST=0");
    assert!(proxy_hits.load(Ordering::SeqCst) >= 1, "检测应经代理");
    // 目标收到的每个请求都带代理注入的 XFF:没有裸直连探测
    let reqs = env.mocks[0].all_requests().await;
    assert!(!reqs.is_empty());
    assert!(
        reqs.iter().all(|r| r
            .to_ascii_lowercase()
            .contains("x-forwarded-for: aicouncil-test-proxy")),
        "全部检测请求均经代理(无裸直连):{reqs:?}"
    );
    env.abort_all().await;
}

/// 本地 IPv4 HTTP 代理 + 远端 US:检测与生成都经代理(POST 计入),
/// Allowed 并成功发送 —— 本地代理不是 IPv6 不构成失败。
#[tokio::test]
async fn local_ipv4_http_proxy_allowed_send_goes_through_proxy() {
    let target = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let proxy_hits = Arc::new(AtomicU64::new(0));
    let proxy_port = spawn_http_proxy(proxy_hits.clone()).await;
    let cfg = NetworkConfig {
        proxy: Some(format!("http://127.0.0.1:{proxy_port}")),
        limits: limits(2000, 5000),
    };
    let url = format!("http://localhost:{}/trace", target.port);
    let guard = GeoIpGuard::with_sources(
        &cfg,
        vec![trace_source(&url)],
        vec![trace_source(&url)],
        limits(2000, 5000),
    )
    .unwrap();
    // 生成客户端也走代理(with_client 注入,生产等价于 AICOUNCIL_PROXY)
    let gen_client = NetworkConfig {
        proxy: cfg.proxy.clone(),
        ..Default::default()
    }
    .generation_client()
    .unwrap();
    let db = Arc::new(open_in_memory().unwrap());
    let gen_mock = common::spawn_counting_mock(move |_b, _h| sse_ok_response()).await;
    let provider = Arc::new(OpenRouter::with_client(
        ProviderId::new(),
        gen_client,
        format!("http://localhost:{}/v1", gen_mock.port),
        "test-key".into(),
    ));
    let (tx, _rx) = broadcast::channel(256);
    let svc = ChatService::new(provider, db.clone(), tx, 4).with_egress_guard(Arc::new(guard));
    let conv = new_conv(&db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    svc.submit(plan).await.expect("代理路径 US 应放行");
    assert!(
        common::wait_for(|| gen_mock.count() == 1, 5000).await,
        "生成 POST=1(经代理)"
    );
    assert!(
        proxy_hits.load(Ordering::SeqCst) >= 3,
        "两次检测 + 一次生成都应经代理:{}",
        proxy_hits.load(Ordering::SeqCst)
    );
    let reqs = target.all_requests().await;
    assert!(
        reqs.iter()
            .all(|r| r.to_ascii_lowercase().contains("x-forwarded-for")),
        "检测无裸直连:{reqs:?}"
    );
    target.abort();
    gen_mock.handle.abort();
}

/// 非门禁模型(非 Anthropic)无地理检测依赖:两族受限 mock 下
/// 正常发送,检测源零访问(原语义不退化)。
#[tokio::test]
async fn non_anthropic_models_keep_original_semantics() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=CN\n".into()))],
    )
    .await;
    let v6 = spawn_trace_mock(
        "[::1]",
        vec![("trace", Resp::Text("ip=2408:8207:db0::a\nloc=CN\n".into()))],
    )
    .await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!("http://localhost:{}/trace", v6.port))],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(
        conv,
        vec![
            anthropic_target("openai/gpt-4o"),
            anthropic_target("google/gemini-2.5-pro"),
        ],
    );
    env.svc.submit(plan).await.expect("非门禁模型无检测依赖");
    assert!(common::wait_for(|| env.mock.count() == 2, 4000).await);
    for m in &env.mocks {
        assert_eq!(m.hit_count(), 0, "非门禁模型零检测调用");
    }
    env.abort_all().await;
}

/// 混合批次 + 真生产门禁(v6 CN):整批拒绝,零落库零 POST。
#[tokio::test]
async fn mixed_batch_blocked_all_or_nothing_with_real_guard() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let v6 = spawn_trace_mock(
        "[::1]",
        vec![("trace", Resp::Text("ip=2408:8207:db0::a\nloc=CN\n".into()))],
    )
    .await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!("http://localhost:{}/trace", v6.port))],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(
        conv,
        vec![
            anthropic_target("openai/gpt-4o"),
            anthropic_target("anthropic/claude-opus-5.5"),
        ],
    );
    assert!(env.svc.submit(plan).await.is_err(), "含门禁模型整批拒绝");
    assert_eq!(env.mock.count(), 0);
    assert_eq!(counts(&env.db, conv), (0, 0, 0));
    env.abort_all().await;
}

/// 最小"源站型"代理:对任何请求直接回一段固定 trace 文本
/// (模拟"换了一个策略完全不同的代理")。返回端口并计数。
async fn spawn_origin_proxy(body: String, hits: Arc<AtomicU64>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let hits = hits.clone();
            let body = body.clone();
            tokio::spawn(async move {
                if read_request(&mut sock).await.is_none() {
                    return;
                }
                hits.fetch_add(1, Ordering::SeqCst);
                let out = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(out.as_bytes()).await;
            });
        }
    });
    port
}

/// 敏感数据:检测请求为 GET、无 Authorization、无 Key 形态内容。
#[tokio::test]
async fn detection_requests_carry_no_credentials() {
    let v4 = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let v6 = spawn_trace_mock(
        "[::1]",
        vec![("trace", Resp::Text("ip=2606:4700::1\nloc=JP\n".into()))],
    )
    .await;
    let guard = GeoIpGuard::with_sources(
        &NetworkConfig::default(),
        vec![trace_source(&format!("http://localhost:{}/trace", v4.port))],
        vec![trace_source(&format!("http://localhost:{}/trace", v6.port))],
        limits(2000, 5000),
    )
    .unwrap();
    let env = build_env(guard, vec![v4, v6]).await;
    let conv = new_conv(&env.db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    env.svc.submit(plan).await.unwrap();
    assert!(common::wait_for(|| env.mock.count() == 1, 4000).await);
    for m in &env.mocks {
        let reqs = m.all_requests().await;
        assert!(!reqs.is_empty());
        for r in reqs {
            assert!(r.starts_with("GET /trace"), "检测必须 GET /trace:{r}");
            let low = r.to_ascii_lowercase();
            assert!(!low.contains("authorization"), "检测不得带凭据:{r}");
            assert!(!r.contains("sk-"), "检测不得带 Key:{r}");
            assert!(
                r.to_ascii_lowercase().contains("user-agent: aicouncil/"),
                "UA 应为应用 UA:{r}"
            );
        }
    }
    env.abort_all().await;
}

/// reload 回归(真实客户端策略切换,非 StaticGuard bump):
/// guard 与 ChatService 共享同一 Arc 实例。初始经转发代理 P1(远端 US)
/// 发送成功;`rebuild_clients` 以新代理 P2(策略完全不同,直接返回 CN
/// trace)原子替换检测客户端后:
/// - revision 必须 bump(reload_providers 同款语义);
/// - **同一实例**检测随即走 P2 得 CN Blocked(检测策略与新生成策略一致);
/// - reload 前签发的旧 permit 因 revision 变化失效,admission 重检拒绝,
///   不再产生新 POST。
#[tokio::test]
async fn reload_rebuilds_detection_clients_shared_with_service() {
    // P1:转发型代理 → 目标 mock(US)
    let target_a = spawn_trace_mock(
        "127.0.0.1",
        vec![("trace", Resp::Text("ip=203.0.113.7\nloc=US\n".into()))],
    )
    .await;
    let p1_hits = Arc::new(AtomicU64::new(0));
    let p1 = spawn_http_proxy(p1_hits.clone()).await;
    // P2:源站型代理(直接返回 CN trace,策略与 P1 完全不同)
    let p2_hits = Arc::new(AtomicU64::new(0));
    let p2 = spawn_origin_proxy("ip=2408:8207:db0::a\nloc=CN\n".into(), p2_hits.clone()).await;

    let src = format!("http://localhost:{}/trace", target_a.port);
    let cfg_a = NetworkConfig {
        proxy: Some(format!("http://127.0.0.1:{p1}")),
        limits: limits(2000, 5000),
    };
    let guard = GeoIpGuard::with_sources(
        &cfg_a,
        vec![trace_source(&src)],
        vec![trace_source(&src)],
        limits(2000, 5000),
    )
    .unwrap();

    let db = Arc::new(open_in_memory().unwrap());
    let gen_mock = common::spawn_counting_mock(move |_b, _h| sse_ok_response()).await;
    let provider = Arc::new(OpenRouter::with_base_url(
        ProviderId::new(),
        format!("http://localhost:{}/v1", gen_mock.port),
        "test-key".into(),
    ));
    let (tx, _rx) = broadcast::channel(256);
    // 与生产 Desktop.egress / ChatService.guard 同款:同一 Arc 实例共享
    let shared: Arc<GeoIpGuard> = Arc::new(guard);
    let svc = ChatService::new(provider, db.clone(), tx, 4).with_egress_guard(shared.clone());
    let rev_before = shared.network_revision();

    // reload 前:经 P1 → US → 发送成功
    let conv = new_conv(&db).await;
    let plan = make_plan(conv, vec![anthropic_target("anthropic/claude-opus-5.5")]);
    svc.submit(plan).await.expect("P1(US)应放行");
    assert!(
        common::wait_for(|| gen_mock.count() == 1, 5000).await,
        "POST=1"
    );
    assert!(p1_hits.load(Ordering::SeqCst) >= 1, "检测经 P1");
    // reload 前的旧许可(revision 绑定旧策略)
    let stale_permit = EgressPermit::fresh(rev_before, RouteFamilies::DUAL);

    // 模拟 reload:同一实例原子重建检测客户端到 P2
    let cfg_b = NetworkConfig {
        proxy: Some(format!("http://127.0.0.1:{p2}")),
        limits: limits(2000, 5000),
    };
    shared.rebuild_clients(&cfg_b).unwrap();
    assert_eq!(
        shared.network_revision(),
        rev_before + 1,
        "rebuild_clients 必须 bump revision(旧许可失效)"
    );

    // 同一实例(ChatService 持有克隆)检测策略已切换:经 P2 → CN Blocked
    let verdict = shared.check(CancellationToken::new()).await;
    match &verdict {
        EgressVerdict::Blocked(o) => {
            assert_eq!(o.country_code.as_deref(), Some("CN"), "{verdict:?}");
            assert!(o.evidence().contains("families=v6"), "{verdict:?}");
        }
        other => panic!("重建后检测应切换到 P2(CN):{other:?}"),
    }
    assert!(p2_hits.load(Ordering::SeqCst) >= 1, "新检测客户端走 P2");
    assert_eq!(
        p1_hits.load(Ordering::SeqCst),
        2,
        "重建后不得再走 P1(两族各一次)"
    );

    // 旧 permit 失效:admission 重检 → Blocked → 无新 POST
    let plan2 = make_plan(conv, vec![anthropic_target("anthropic/claude-fable-5.1")]);
    match svc
        .submit_with_permit(plan2, Some(stale_permit))
        .await
        .unwrap_err()
    {
        SubmitError::Admission(AdmissionError::EgressBlocked { country, .. }) => {
            assert_eq!(country.as_deref(), Some("CN"));
        }
        other => panic!("旧 permit 必须失效并重检拒绝:{other:?}"),
    }
    assert_eq!(gen_mock.count(), 1, "reload 后旧许可不得产生新 POST");

    target_a.abort();
    gen_mock.handle.abort();
}
