//! Anthropic 全模型出口门禁(IPv4/IPv6 双族核验)。
//!
//! - 主源 `https://openrouter.ai/cdn-cgi/trace`(实际生成业务域名,
//!   无需 API key;解析文本 `ip=` 与 `loc=`,非 `colo=`);
//!   备源依次 `https://ipapi.co/json/`、`https://api.ip.sb/geoip`。
//!   仅主源不可用/数据无效时逐级降级;族内主源有效结论(含 CN/HK 拦截)
//!   权威,不可被备源覆盖。
//! - IPv4 与 IPv6 各用一个限定地址族的客户端并行检测(共享同一总预算
//!   截止期,不翻倍):任一族命中 CN/HK 即拦截且不可被另一族/备源覆盖;
//!   先完成的族结论保留(截止期只把未完成分支记 Unknown);未核验的族
//!   不进许可,发送连接器按族过滤(见 network.rs),未核验路径不可能被
//!   生成使用;两族都无结论才 Unknown 拦截。
//! - 本地/显式代理模式下,"族"约束作用于到代理的本地连接(检测与
//!   生成同一过滤),远端出口由代理决定;不把探测绑到本地 IPv6 地址。
//! - 仅屏蔽 CN/HK;明确有效的其他 ISO 国家/地区放行;超时/断网/
//!   429/无国家码/无效 IP/保留地址/解析失败/字段缺失/重复/哨兵码
//!   → 该族 Unknown(该族被排除,不当作不存在无约束放行)。
//! - 单源超时 4s,总预算 8s 强制;取消立即结束两族。检测请求不带
//!   Authorization/prompt/附件;响应体限量读取,禁跟随重定向。
//! - 检测结果仅短期内存,不持久化。
//!
//! 出口保证边界:GeoIP 结论是各数据库对同一出口的归属判定;证据表明
//! 不同服务的数据库结论可能相互冲突(主源取业务提供方自身域名的
//! 判定,与下游判定基准最接近)。按域名分流、同族多出口、代理动态
//! 换出口仍可能与上游不一致;不宣称证实真实物理地理,也不宣称保证
//! Anthropic 上游所见 IP。许可保证的是"发送连接只能使用已核验的
//! 连接路径(族)"。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::providers::cancel::CancellationToken;

use super::network::NetworkConfig;

/// 被拦截地区(CN 大陆 / HK 香港;不扩大到 MO/TW 等)。
const BLOCKED_COUNTRIES: &[&str] = &["CN", "HK"];

/// 检测结果。
#[derive(Debug, Clone, PartialEq)]
pub enum EgressVerdict {
    /// 明确有效且非 CN/HK:放行本次动作(obs.families = 已核验可用的
    /// 地址族;未核验族必须在发送连接层排除,不可让发送任意选路)。
    Allowed(EgressObservation),
    /// 明确 CN/HK:拦截(obs 指明是哪条路径/地址族)。
    Blocked(EgressObservation),
    /// 无法确认:拦截并提示(不把缺字段当境外)。
    Unknown(String),
}

/// 一次出口观测(仅短期内存;弹窗可显示,日志默认遮罩 IP)。
#[derive(Debug, Clone, PartialEq)]
pub struct EgressObservation {
    pub ip: Option<String>,
    pub country_code: Option<String>,
    pub source: &'static str,
    pub checked_at: chrono::DateTime<chrono::Utc>,
    /// 本观测核验可用的地址族(单源观测 = 单族;聚合观测 = 并集)。
    pub families: crate::services::network::RouteFamilies,
    /// 补充说明(另一族的状态/排除原因;不含密钥或用户内容)。
    pub note: Option<String>,
}

impl EgressObservation {
    /// 证据串(来源/IP/国家/族/检测时刻;不含任何密钥/对话内容)。
    pub fn evidence(&self) -> String {
        let mut s = format!(
            "source={} ip={} country={} families={} checked_at={}",
            self.source,
            self.ip.as_deref().unwrap_or("?"),
            self.country_code.as_deref().unwrap_or("?"),
            self.families.label(),
            self.checked_at.to_rfc3339()
        );
        if let Some(note) = &self.note {
            s.push_str("; note=");
            s.push_str(note);
        }
        s
    }
}

/// 短期许可:仅同 intent、同 network_revision 的批内使用;
/// 不跨动作缓存 Allowed(4.1)。`families` 是本次检测核验的地址族,
/// 发送连接器必须把请求限制在该集合内。
#[derive(Debug, Clone, Copy)]
pub struct EgressPermit {
    /// 单调时钟检测时刻(处理系统时间跳变)。
    pub checked_at_mono: tokio::time::Instant,
    pub revision: u64,
    pub families: crate::services::network::RouteFamilies,
}

pub const PERMIT_TTL: Duration = Duration::from_secs(3);

impl EgressPermit {
    pub fn fresh(revision: u64, families: crate::services::network::RouteFamilies) -> Self {
        Self {
            checked_at_mono: tokio::time::Instant::now(),
            revision,
            families,
        }
    }

    pub fn still_valid(&self, revision: u64) -> bool {
        self.revision == revision && self.checked_at_mono.elapsed() < PERMIT_TTL
    }
}

/// 门禁接口(测试可注入 mock;生产不提供"关闭地域检查"的开关)。
pub trait EgressGuard: Send + Sync {
    /// 执行一次检测(每次动作新检测;不读持久缓存)。
    fn check(&self, cancel: CancellationToken) -> BoxFuture<'static, EgressVerdict>;

    /// 网络配置版本(代理/网络策略变化时 bump;许可失效)。
    fn network_revision(&self) -> u64 {
        0
    }
}

/// 静态判定 mock(测试用)。
pub struct StaticGuard {
    pub verdict: EgressVerdict,
    pub revision: AtomicU64,
    /// 检测调用计数(A07 批内合并断言)。
    pub checks: AtomicU64,
}

impl StaticGuard {
    pub fn new(verdict: EgressVerdict) -> Self {
        Self {
            verdict,
            revision: AtomicU64::new(0),
            checks: AtomicU64::new(0),
        }
    }

    pub fn bump_revision(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
    }

    pub fn check_count(&self) -> u64 {
        self.checks.load(Ordering::SeqCst)
    }
}

impl EgressGuard for StaticGuard {
    fn check(&self, _cancel: CancellationToken) -> BoxFuture<'static, EgressVerdict> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        let v = self.verdict.clone();
        Box::pin(async move { v })
    }

    fn network_revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }
}

/// 生产 GeoIP 门禁:IPv4/IPv6 双族并行核验(每族:主 ip.sb 序列中的
/// OpenRouter 业务域名主源 + 备源降级),聚合判定见 [`aggregate`]:
/// 任一族明确 CN/HK 即拦截(不可被另一族或备源 US 覆盖);未核验族
/// 从许可中排除(发送连接器按族过滤),只有全部族核验失败才 Unknown。
///
/// reload:`rebuild_clients` 以与新生成客户端相同的网络策略快照原子
/// 替换检测客户端并 bump revision(既有许可全部失效),保证检测与
/// 生成策略不分裂;构建失败时保持旧客户端不变并显式报错。
pub struct GeoIpGuard {
    clients: std::sync::RwLock<GuardClients>,
    sources_v4: Vec<SourceSpec>,
    sources_v6: Vec<SourceSpec>,
    limits: super::network::NetworkLimits,
    revision: AtomicU64,
}

#[derive(Clone)]
struct GuardClients {
    v4: reqwest::Client,
    v6: reqwest::Client,
}

impl GeoIpGuard {
    pub fn new(config: &NetworkConfig) -> Result<Self, String> {
        Self::with_sources(
            config,
            production_sources(),
            production_sources(),
            config.limits,
        )
    }

    /// 测试注入:两族各自的检测源(URL 指向本地 mock)与超时预算。
    /// 生产路径不使用;不复刻"关闭地域检查"开关语义(仅换观测目标)。
    #[doc(hidden)]
    pub fn with_sources(
        config: &NetworkConfig,
        sources_v4: Vec<SourceSpec>,
        sources_v6: Vec<SourceSpec>,
        limits: super::network::NetworkLimits,
    ) -> Result<Self, String> {
        Ok(Self {
            clients: std::sync::RwLock::new(GuardClients {
                v4: config
                    .detection_client_for(crate::services::network::RouteFamilies::V4_ONLY)?,
                v6: config
                    .detection_client_for(crate::services::network::RouteFamilies::V6_ONLY)?,
            }),
            sources_v4,
            sources_v6,
            limits,
            revision: AtomicU64::new(0),
        })
    }

    /// reload:以新网络策略快照重建检测客户端(与 `generation_client*`
    /// 同一构造路径);构建成功前不触碰旧客户端,失败保持现状并显式报错。
    /// 成功后 bump revision:既有许可(revision 绑定)全部失效。
    pub fn rebuild_clients(&self, config: &NetworkConfig) -> Result<(), String> {
        let v4 = config.detection_client_for(crate::services::network::RouteFamilies::V4_ONLY)?;
        let v6 = config.detection_client_for(crate::services::network::RouteFamilies::V6_ONLY)?;
        *self.clients.write().unwrap_or_else(|p| p.into_inner()) = GuardClients { v4, v6 };
        self.bump_revision();
        Ok(())
    }

    pub fn bump_revision(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
    }
}

impl Default for GeoIpGuard {
    fn default() -> Self {
        Self::new(&NetworkConfig::default()).expect("默认检测客户端可用")
    }
}

impl EgressGuard for GeoIpGuard {
    fn check(&self, cancel: CancellationToken) -> BoxFuture<'static, EgressVerdict> {
        let clients = self
            .clients
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let sources_v4 = self.sources_v4.clone();
        let sources_v6 = self.sources_v6.clone();
        let limits = self.limits;
        Box::pin(async move {
            run_detection(
                &clients.v4,
                &sources_v4,
                &clients.v6,
                &sources_v6,
                limits,
                cancel,
            )
            .await
        })
    }

    fn network_revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }
}

/// 单源原始观测/失败(解析函数返回值;随 SourceSpec 暴露给测试注入)。
#[derive(Debug)]
pub enum SourceOutcome {
    Ok(EgressObservation),
    Invalid(String),
    Failed(String),
}

/// 一个检测源:URL + 响应解析函数(测试可指向本地 mock)。
#[derive(Clone, Copy)]
pub struct SourceSpec {
    pub name: &'static str,
    pub url: &'static str,
    pub parse: fn(&str) -> SourceOutcome,
}

/// 生产检测序列:主源为 OpenRouter 自身域名(与生成请求同域观测),
/// 备源仅当主源不可用/数据无效时逐级降级。
fn production_sources() -> Vec<SourceSpec> {
    vec![
        SourceSpec {
            name: "openrouter.ai/cdn-cgi/trace",
            url: "https://openrouter.ai/cdn-cgi/trace",
            parse: parse_openrouter_trace,
        },
        SourceSpec {
            name: "ipapi.co",
            url: "https://ipapi.co/json/",
            parse: parse_ipapi_co,
        },
        SourceSpec {
            name: "ip.sb",
            url: "https://api.ip.sb/geoip",
            parse: parse_ip_sb,
        },
    ]
}

/// 解析 Cloudflare trace 文本(实际业务域名出口观测)。
/// 只取 `ip=` 与 `loc=`(非 `colo=`);严格拒绝缺失/重复/非法字段。
#[doc(hidden)]
pub fn parse_openrouter_trace(raw: &str) -> SourceOutcome {
    let mut ip: Option<&str> = None;
    let mut loc: Option<&str> = None;
    for line in raw.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        match k.trim() {
            "ip" if ip.replace(v).is_some() => {
                return SourceOutcome::Invalid("重复 ip 字段".into())
            }
            "loc" if loc.replace(v).is_some() => {
                return SourceOutcome::Invalid("重复 loc 字段".into())
            }
            _ => {}
        }
    }
    validate_pair(ip, loc, "openrouter.ai/cdn-cgi/trace")
}

fn parse_ip_sb(raw: &str) -> SourceOutcome {
    json_source_parse(raw, "ip.sb", |json| {
        // ip.sb 200 也可能带 error 字段
        if json.get("error").is_some() {
            Some(SourceOutcome::Invalid("ip.sb 返回 error".into()))
        } else {
            None
        }
    })
}

fn parse_ipapi_co(raw: &str) -> SourceOutcome {
    json_source_parse(raw, "ipapi.co", |json| {
        // ipapi.co 限流/错误时 HTTP 200 内含 {"error": true, "reason": ...}
        if json.get("error").and_then(|e| e.as_bool()).unwrap_or(false) {
            let reason = json
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");
            Some(SourceOutcome::Invalid(format!("ipapi.co error: {reason}")))
        } else {
            None
        }
    })
}

/// JSON 源公共入口:防 HTML 错误页(体必须以 `{` 开头)→ 解析 →
/// 源特定 error 预检 → 公共 ip/country_code 校验。
fn json_source_parse(
    raw: &str,
    source: &'static str,
    pre: fn(&serde_json::Value) -> Option<SourceOutcome>,
) -> SourceOutcome {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with('{') {
        return SourceOutcome::Failed("非 JSON 响应(HTML 错误页等)".into());
    }
    let json: serde_json::Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => return SourceOutcome::Failed(format!("JSON 解析失败:{e}")),
    };
    if let Some(o) = pre(&json) {
        return o;
    }
    common_parse(&json, source)
}

fn common_parse(json: &serde_json::Value, source: &'static str) -> SourceOutcome {
    let ip = json.get("ip").and_then(|v| v.as_str());
    let cc = json.get("country_code").and_then(|v| v.as_str());
    validate_pair(ip, cc, source)
}

/// 共享校验:公网 IP 形态 + 2 位大写 ISO 码;ZZ/XX/空哨兵拒绝。
fn validate_pair(ip: Option<&str>, cc: Option<&str>, source: &'static str) -> SourceOutcome {
    let ip = ip
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let cc = cc
        .map(|s| s.trim().to_uppercase())
        .filter(|s| !s.is_empty());
    let ip_valid = ip.as_deref().is_some_and(looks_like_public_ip);
    let cc_valid = cc
        .as_deref()
        .is_some_and(|c| c.len() == 2 && c.chars().all(|ch| ch.is_ascii_uppercase()));
    if !ip_valid || !cc_valid {
        let mut why = Vec::new();
        if !ip_valid {
            why.push("无效/缺失 IP");
        }
        if !cc_valid {
            why.push("缺失国家码");
        }
        return SourceOutcome::Invalid(why.join(";"));
    }
    // ZZ/XX/空值哨兵 → Unknown
    if matches!(cc.as_deref(), Some("ZZ") | Some("XX")) {
        return SourceOutcome::Invalid("哨兵国家码".into());
    }
    // families/note 由分族检测层(detect_sequence/aggregate)填写
    SourceOutcome::Ok(EgressObservation {
        ip,
        country_code: cc,
        source,
        checked_at: chrono::Utc::now(),
        families: crate::services::network::RouteFamilies::DUAL,
        note: None,
    })
}

/// 公网 IP 形态检查(IPv4/IPv6 文本;拒绝保留/回环/链路本地段)。
fn looks_like_public_ip(s: &str) -> bool {
    s.parse::<std::net::IpAddr>()
        .map(|ip| match ip {
            std::net::IpAddr::V4(v4) => {
                !(v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_broadcast()
                    || v4.is_unspecified())
            }
            std::net::IpAddr::V6(v6) => {
                // v4-mapped 地址按其 IPv4 语义校验(映射私网/回环不算公网)
                if let Some(v4) = v6.to_ipv4_mapped() {
                    return !(v4.is_loopback()
                        || v4.is_private()
                        || v4.is_link_local()
                        || v4.is_broadcast()
                        || v4.is_unspecified());
                }
                !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_unicast_link_local()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00)
            }
        })
        .unwrap_or(false)
}

/// 限量读取响应体(超过上限判失败,防超大响应)。
async fn read_limited(resp: reqwest::Response, limit: usize) -> Result<String, String> {
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(2048);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("读取失败:{e}"))?;
        buf.extend_from_slice(&chunk);
        if buf.len() > limit {
            return Err("响应体超过 64 KiB 上限".into());
        }
    }
    String::from_utf8(buf).map_err(|_| "响应非 UTF-8".to_string())
}

async fn fetch_source(
    http: &reqwest::Client,
    src: &SourceSpec,
    timeout: Duration,
) -> SourceOutcome {
    let fut = async {
        let resp = http
            .get(src.url)
            .send()
            .await
            .map_err(|e| format!("网络错误:{e}"))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            // 429/5xx/HTML 错误页等:该源本次不可用
            return Err(format!("HTTP {status}"));
        }
        let body = read_limited(
            resp,
            super::network::NetworkLimits::default().egress_body_limit,
        )
        .await?;
        Ok((src.parse)(&body))
    };
    // 内层 Result<String> = 该源失败/无效原因
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => SourceOutcome::Failed(e),
        Err(_) => SourceOutcome::Failed(format!("{} 超时", src.name)),
    }
}

fn classify(obs: EgressObservation) -> EgressVerdict {
    if BLOCKED_COUNTRIES.contains(&obs.country_code.as_deref().unwrap_or("")) {
        EgressVerdict::Blocked(obs)
    } else {
        EgressVerdict::Allowed(obs)
    }
}

fn miss(o: &SourceOutcome) -> String {
    match o {
        SourceOutcome::Failed(e) | SourceOutcome::Invalid(e) => e.clone(),
        SourceOutcome::Ok(_) => String::new(),
    }
}

/// 单族检测序列:按序尝试,首个有效观测权威(主源有效 CN/HK 即拦截,
/// 不可被备源覆盖);仅不可用/无效才降级到下一源。每源最多一次,
/// 单源超时由 fetch_source 强制。观测的 families 由本函数盖章为该族。
async fn detect_sequence(
    http: &reqwest::Client,
    sources: &[SourceSpec],
    per_source: Duration,
    family: crate::services::network::RouteFamilies,
) -> EgressVerdict {
    let mut misses: Vec<String> = Vec::new();
    for src in sources {
        let outcome = fetch_source(http, src, per_source).await;
        match outcome {
            SourceOutcome::Ok(mut obs) => {
                obs.families = family;
                return classify(obs);
            }
            o => misses.push(format!("{}:{}", src.name, miss(&o))),
        }
    }
    EgressVerdict::Unknown(format!("检测源全部失败({})", misses.join("; ")))
}

/// 聚合两族判定:
/// - 任一族明确 CN/HK → Blocked(该族证据;不可被另一族或备源覆盖);
/// - 否则任一族 Allowed → Allowed(families = 已核验族并集;未核验族
///   的失败原因写入 note,发送连接器据此排除该族);
/// - 两族都无有效结论 → Unknown(两族原因并列,不猜)。
fn aggregate(v4: EgressVerdict, v6: EgressVerdict) -> EgressVerdict {
    use crate::services::network::RouteFamilies;
    let branch_text = |v: &EgressVerdict, tag: &str| match v {
        EgressVerdict::Allowed(o) => {
            format!(
                "{tag}:{} 国家={}",
                o.source,
                o.country_code.as_deref().unwrap_or("?")
            )
        }
        EgressVerdict::Blocked(o) => {
            format!(
                "{tag}:{} 国家={}",
                o.source,
                o.country_code.as_deref().unwrap_or("?")
            )
        }
        EgressVerdict::Unknown(r) => format!("{tag}:未核验({r})"),
    };
    let note_for = |other: &EgressVerdict, other_tag: &str| match other {
        EgressVerdict::Allowed(o) => Some(format!(
            "{other_tag} 核验通过:ip={} country={} source={}",
            o.ip.as_deref().unwrap_or("?"),
            o.country_code.as_deref().unwrap_or("?"),
            o.source
        )),
        EgressVerdict::Blocked(o) => Some(format!(
            "{other_tag} 受限:ip={} country={} source={}",
            o.ip.as_deref().unwrap_or("?"),
            o.country_code.as_deref().unwrap_or("?"),
            o.source
        )),
        EgressVerdict::Unknown(r) => {
            // Unknown 分支:明确记录"未核验且已排除",不当作不存在
            Some(format!("{other_tag} 未核验,已从发送连接排除({r})"))
        }
    };

    match (&v4, &v6) {
        // Blocked 优先;两族同时受限时优先呈现 v6(新增受核验路径)
        (_, EgressVerdict::Blocked(o6)) => {
            let mut o = o6.clone();
            o.note = note_for(&v4, "IPv4");
            EgressVerdict::Blocked(o)
        }
        (EgressVerdict::Blocked(o4), _) => {
            let mut o = o4.clone();
            o.note = note_for(&v6, "IPv6");
            EgressVerdict::Blocked(o)
        }
        (EgressVerdict::Allowed(o4), EgressVerdict::Allowed(o6)) => {
            let mut o = o4.clone();
            o.families = o4.families.union(o6.families);
            o.note = note_for(&v6, "IPv6");
            EgressVerdict::Allowed(o)
        }
        (EgressVerdict::Allowed(o4), EgressVerdict::Unknown(r6)) => {
            let mut o = o4.clone();
            o.families = RouteFamilies::V4_ONLY;
            o.note = Some(format!("IPv6 未核验,已从发送连接排除({r6})"));
            EgressVerdict::Allowed(o)
        }
        (EgressVerdict::Unknown(r4), EgressVerdict::Allowed(o6)) => {
            let mut o = o6.clone();
            o.families = RouteFamilies::V6_ONLY;
            o.note = Some(format!("IPv4 未核验,已从发送连接排除({r4})"));
            EgressVerdict::Allowed(o)
        }
        _ => EgressVerdict::Unknown(format!(
            "无任何已核验出网地址族({}; {})",
            branch_text(&v4, "IPv4"),
            branch_text(&v6, "IPv6")
        )),
    }
}

/// 双族并行检测:每族独立走主源→备源序列;共享同一总预算截止期
/// (不翻倍):任一族先完成即保留其结论,截止期只把**未完成**的分支
/// 记为 Unknown(未核验族随后在聚合中被发送层排除),不因一族超时
/// 丢弃另一族已完成的结果。取消立即结束两族。
async fn run_detection(
    http_v4: &reqwest::Client,
    sources_v4: &[SourceSpec],
    http_v6: &reqwest::Client,
    sources_v6: &[SourceSpec],
    limits: super::network::NetworkLimits,
    cancel: CancellationToken,
) -> EgressVerdict {
    let deadline = tokio::time::Instant::now() + limits.egress_total_timeout;
    let v4 = detect_sequence(
        http_v4,
        sources_v4,
        limits.egress_source_timeout,
        crate::services::network::RouteFamilies::V4_ONLY,
    );
    let v6 = detect_sequence(
        http_v6,
        sources_v6,
        limits.egress_source_timeout,
        crate::services::network::RouteFamilies::V6_ONLY,
    );
    tokio::pin!(v4, v6);
    let deadline = tokio::time::Instant::now() + limits.egress_total_timeout;
    let guarded_v4 = async {
        tokio::select! {
            v = &mut v4 => v,
            _ = tokio::time::sleep_until(deadline) => {
                EgressVerdict::Unknown("检测总预算超时(IPv4 分支未完成)".into())
            }
        }
    };
    let guarded_v6 = async {
        tokio::select! {
            v = &mut v6 => v,
            _ = tokio::time::sleep_until(deadline) => {
                EgressVerdict::Unknown("检测总预算超时(IPv6 分支未完成)".into())
            }
        }
    };
    tokio::select! {
        _ = cancel.cancelled() => EgressVerdict::Unknown("检测已取消".into()),
        (v4, v6) = futures_util::future::join(guarded_v4, guarded_v6) => aggregate(v4, v6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::network::RouteFamilies;

    fn obs(cc: &str) -> EgressObservation {
        EgressObservation {
            ip: Some("203.0.113.10".into()),
            country_code: Some(cc.into()),
            source: "test",
            checked_at: chrono::Utc::now(),
            families: RouteFamilies::DUAL,
            note: None,
        }
    }

    #[test]
    fn classification_cn_hk_blocked_others_allowed() {
        assert!(matches!(classify(obs("CN")), EgressVerdict::Blocked(_)));
        assert!(matches!(classify(obs("HK")), EgressVerdict::Blocked(_)));
        // A02:MO/TW 等不扩大封禁
        assert!(matches!(classify(obs("MO")), EgressVerdict::Allowed(_)));
        assert!(matches!(classify(obs("TW")), EgressVerdict::Allowed(_)));
        assert!(matches!(classify(obs("US")), EgressVerdict::Allowed(_)));
    }

    #[test]
    fn payload_validation_rejects_bad_data() {
        // A03:缺码/ZZ/保留 IP/200 error 均为无效
        let j = serde_json::json!({"ip": "1.1.1.1"});
        assert!(matches!(
            parse_ip_sb(&j.to_string()),
            SourceOutcome::Invalid(_)
        ));
        let j = serde_json::json!({"ip": "1.1.1.1", "country_code": "ZZ"});
        assert!(matches!(
            parse_ip_sb(&j.to_string()),
            SourceOutcome::Invalid(_)
        ));
        let j = serde_json::json!({"ip": "127.0.0.1", "country_code": "US"});
        assert!(matches!(
            parse_ip_sb(&j.to_string()),
            SourceOutcome::Invalid(_)
        ));
        let j = serde_json::json!({"ip": "10.0.0.5", "country_code": "US"});
        assert!(matches!(
            parse_ip_sb(&j.to_string()),
            SourceOutcome::Invalid(_)
        ));
        let j = serde_json::json!({"error": true, "reason": "rate limited"});
        assert!(matches!(
            parse_ipapi_co(&j.to_string()),
            SourceOutcome::Invalid(_)
        ));
        // 非 JSON(HTML 错误页)→ 失败
        assert!(matches!(
            parse_ip_sb("<html>err</html>"),
            SourceOutcome::Failed(_)
        ));
        // 小写国家码规范化
        let j = serde_json::json!({"ip": "203.0.113.9", "country_code": "jp"});
        assert!(matches!(parse_ip_sb(&j.to_string()), SourceOutcome::Ok(_)));
    }

    #[test]
    fn trace_parse_valid_and_authoritative_fields() {
        // 真实 trace 形态:ip/loc 与 colo 并存;只取 ip/loc
        let body = "fl=1abc\nh=openrouter.ai\nip=14.137.237.253\nts=1730000000.1\nvisit_scheme=https\nuag=Mozilla/5.0\ncolo=SJC\nhttp=h2\nloc=JP\ntls=TLSv1.3\nsni=plaintext\nwarp=off\n";
        match parse_openrouter_trace(body) {
            SourceOutcome::Ok(obs) => {
                assert_eq!(obs.ip.as_deref(), Some("14.137.237.253"));
                assert_eq!(obs.country_code.as_deref(), Some("JP"));
                assert_eq!(obs.source, "openrouter.ai/cdn-cgi/trace");
            }
            other => panic!("应为有效观测:{other:?}"),
        }
        // loc=CN 同样有效观测(由 classify 判拦截)
        let body = "ip=203.0.113.7\nloc=cn\n";
        assert!(matches!(parse_openrouter_trace(body), SourceOutcome::Ok(_)));
    }

    #[test]
    fn trace_parse_strictly_rejects_bad_fields() {
        // 重复字段
        let dup_ip = "ip=203.0.113.7\nloc=JP\nip=198.51.100.9\n";
        assert!(matches!(
            parse_openrouter_trace(dup_ip),
            SourceOutcome::Invalid(ref m) if m.contains("重复 ip")
        ));
        let dup_loc = "ip=203.0.113.7\nloc=JP\nloc=US\n";
        assert!(matches!(
            parse_openrouter_trace(dup_loc),
            SourceOutcome::Invalid(ref m) if m.contains("重复 loc")
        ));
        // 缺失字段
        assert!(matches!(
            parse_openrouter_trace("loc=JP\n"),
            SourceOutcome::Invalid(ref m) if m.contains("IP")
        ));
        assert!(matches!(
            parse_openrouter_trace("ip=203.0.113.7\n"),
            SourceOutcome::Invalid(ref m) if m.contains("国家码")
        ));
        // 哨兵/非法国家码(Tor T1 含数字,非 ISO)
        for bad in ["ip=203.0.113.7\nloc=XX\n", "ip=203.0.113.7\nloc=T1\n"] {
            assert!(matches!(
                parse_openrouter_trace(bad),
                SourceOutcome::Invalid(_)
            ));
        }
        // 保留 IP
        assert!(matches!(
            parse_openrouter_trace("ip=127.0.0.1\nloc=US\n"),
            SourceOutcome::Invalid(_)
        ));
        // 空字段值
        assert!(matches!(
            parse_openrouter_trace("ip=\nloc=\n"),
            SourceOutcome::Invalid(_)
        ));
    }

    #[test]
    fn evidence_contains_source_ip_country_time_only() {
        let e = obs("CN");
        let ev = e.evidence();
        assert!(ev.contains("source=test"), "{ev}");
        assert!(ev.contains("ip=203.0.113.10"), "{ev}");
        assert!(ev.contains("country=CN"), "{ev}");
        assert!(ev.contains("checked_at="), "{ev}");
        assert!(!ev.contains("sk-"), "证据串不得含密钥形态内容");
    }

    #[tokio::test]
    async fn sequence_primary_blocked_cannot_be_overridden_by_fallback() {
        // 主源有效 CN:即便备源报告 US 也必须拦截(权威,不降级)
        let srv = mock_server(vec![
            ("trace", "ip=203.0.113.7\nloc=CN\n"),
            ("ipapi", r#"{"ip":"203.0.113.8","country_code":"US"}"#),
            ("ipsb", r#"{"ip":"203.0.113.9","country_code":"US"}"#),
        ])
        .await;
        let http = plain_client();
        let v = detect_sequence(
            &http,
            &srv.sources,
            Duration::from_secs(2),
            RouteFamilies::V4_ONLY,
        )
        .await;
        match v {
            EgressVerdict::Blocked(o) => {
                assert_eq!(o.country_code.as_deref(), Some("CN"));
                assert_eq!(o.source, "openrouter.ai/cdn-cgi/trace");
                assert_eq!(o.families, RouteFamilies::V4_ONLY, "单族观测盖章");
            }
            other => panic!("主源 CN 应权威拦截:{other:?}"),
        }
        assert_eq!(srv.hits(), [1, 0, 0], "主源有效时不得访问备源");
    }

    #[tokio::test]
    async fn sequence_fallback_order_and_unknown_on_total_failure() {
        // 主源无效(哨兵)→ ipapi.co 先于 ip.sb,首个有效备源权威
        let srv = mock_server(vec![
            ("trace", "ip=203.0.113.7\nloc=ZZ\n"),
            ("ipapi", r#"{"ip":"203.0.113.8","country_code":"US"}"#),
            ("ipsb", r#"{"ip":"203.0.113.9","country_code":"GB"}"#),
        ])
        .await;
        let http = plain_client();
        let v = detect_sequence(
            &http,
            &srv.sources,
            Duration::from_secs(2),
            RouteFamilies::V4_ONLY,
        )
        .await;
        match v {
            EgressVerdict::Allowed(o) => {
                assert_eq!(o.source, "ipapi.co", "备源顺序:ipapi.co 先于 ip.sb");
            }
            other => panic!("应采用第一个有效备源:{other:?}"),
        }
        assert_eq!(srv.hits(), [1, 1, 0]);

        // 全部失败 → Unknown,且逐源降级到底
        let srv2 = mock_server(vec![
            ("trace", "<html>err</html>"),
            ("ipapi", "{}"),
            ("ipsb", "boom"),
        ])
        .await;
        let v2 = detect_sequence(
            &http,
            &srv2.sources,
            Duration::from_secs(2),
            RouteFamilies::V6_ONLY,
        )
        .await;
        assert!(
            matches!(&v2, EgressVerdict::Unknown(m) if m.contains("trace-src") && m.contains("ipapi-src") && m.contains("ipsb-src")),
            "{v2:?}"
        );
        assert_eq!(srv2.hits(), [1, 1, 1]);
    }

    #[tokio::test]
    async fn sequence_per_source_timeout_bounded() {
        // 源挂起不响应:单源超时后降级,不无限等待
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
                    .await;
                // 不写 body,挂起直至对端超时/断开
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        let sources = vec![SourceSpec {
            name: "hang-src",
            url: Box::leak(format!("http://{addr}/trace").into_boxed_str()),
            parse: parse_openrouter_trace,
        }];
        let http = plain_client();
        let started = std::time::Instant::now();
        let v = detect_sequence(
            &http,
            &sources,
            Duration::from_millis(300),
            RouteFamilies::V4_ONLY,
        )
        .await;
        let elapsed = started.elapsed();
        server.abort();
        assert!(
            matches!(&v, EgressVerdict::Unknown(m) if m.contains("hang-src") && m.contains("超时")),
            "{v:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "应在单源超时后返回:{elapsed:?}"
        );
    }

    // ---- 本地 mock 检测源(无公网)----

    fn plain_client() -> reqwest::Client {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    }

    struct MockSources {
        sources: Vec<SourceSpec>,
        hit_names: &'static std::sync::Mutex<Vec<&'static str>>,
    }

    impl MockSources {
        fn hits(&self) -> [usize; 3] {
            let h = self.hit_names.lock().unwrap();
            [
                h.iter().filter(|n| **n == "trace-src").count(),
                h.iter().filter(|n| **n == "ipapi-src").count(),
                h.iter().filter(|n| **n == "ipsb-src").count(),
            ]
        }
    }

    /// 起一个本地 HTTP 服务:路径 /{route} 返回对应 body;
    /// 返回指向该服务的 SourceSpec 列表(trace→ipapi→ipsb 顺序)。
    async fn mock_server(bodies: Vec<(&'static str, &'static str)>) -> MockSources {
        use std::collections::HashMap;
        use std::sync::Mutex as StdMutex;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let map: HashMap<String, &'static str> = bodies
            .into_iter()
            .map(|(r, b)| (r.to_string(), b))
            .collect();
        let hits: &'static StdMutex<Vec<&'static str>> = Box::leak(Box::new(StdMutex::new(vec![])));
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let route = req
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .trim_start_matches('/')
                    .to_string();
                let hit_name = match route.as_str() {
                    "trace" => "trace-src",
                    "ipapi" => "ipapi-src",
                    _ => "ipsb-src",
                };
                hits.lock().unwrap().push(hit_name);
                let body = map.get(&route).copied().unwrap_or("");
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        // server 与测试同生命周期;测试结束时 abort 由泄漏容忍(测试进程短命)
        tauri::async_runtime::spawn(async move {
            let _ = server.await;
        });
        let base = format!("http://{addr}");
        let mk = |route: &'static str, name: &'static str, parse: fn(&str) -> SourceOutcome| {
            let url = match route {
                "trace" => format!("{base}/trace"),
                "ipapi" => format!("{base}/ipapi"),
                _ => format!("{base}/ipsb"),
            };
            SourceSpec {
                name,
                url: Box::leak(url.into_boxed_str()),
                parse,
            }
        };
        MockSources {
            sources: vec![
                mk("trace", "trace-src", parse_openrouter_trace),
                mk("ipapi", "ipapi-src", parse_ipapi_co),
                mk("ipsb", "ipsb-src", parse_ip_sb),
            ],
            hit_names: hits,
        }
    }

    #[test]
    fn permit_ttl_and_revision() {
        let mut p = EgressPermit::fresh(0, RouteFamilies::DUAL);
        assert!(p.still_valid(0));
        assert_eq!(p.families, RouteFamilies::DUAL);
        assert!(!p.still_valid(1), "revision 变化即失效");
        p.checked_at_mono = tokio::time::Instant::now() - PERMIT_TTL - Duration::from_millis(1);
        assert!(!p.still_valid(0), "过期失效");
    }

    // ---- 双族聚合 ----

    fn fam_obs(cc: &str, ip: &str, f: RouteFamilies, src: &'static str) -> EgressObservation {
        EgressObservation {
            ip: Some(ip.into()),
            country_code: Some(cc.into()),
            source: src,
            checked_at: chrono::Utc::now(),
            families: f,
            note: None,
        }
    }

    #[test]
    fn aggregate_v4_us_v6_cn_blocked_with_v6_evidence() {
        // 关键回归:v4 US 放行不能掩盖 v6 CN —— 必须整体拦截
        let v4 = EgressVerdict::Allowed(fam_obs(
            "US",
            "203.0.113.1",
            RouteFamilies::V4_ONLY,
            "openrouter.ai/cdn-cgi/trace",
        ));
        let v6 = EgressVerdict::Blocked(fam_obs(
            "CN",
            "2408:8207::1",
            RouteFamilies::V6_ONLY,
            "openrouter.ai/cdn-cgi/trace",
        ));
        match aggregate(v4, v6) {
            EgressVerdict::Blocked(o) => {
                assert_eq!(o.country_code.as_deref(), Some("CN"));
                assert_eq!(o.families, RouteFamilies::V6_ONLY);
                let ev = o.evidence();
                assert!(ev.contains("families=v6"), "{ev}");
                assert!(ev.contains("IPv4 核验通过"), "另一族证据保留:{ev}");
            }
            other => panic!("v6 CN 必须拦截:{other:?}"),
        }
    }

    #[test]
    fn aggregate_v4_cn_v6_us_blocked_not_overridden() {
        let v4 = EgressVerdict::Blocked(fam_obs(
            "HK",
            "203.0.113.1",
            RouteFamilies::V4_ONLY,
            "openrouter.ai/cdn-cgi/trace",
        ));
        let v6 = EgressVerdict::Allowed(fam_obs(
            "US",
            "2606:4700::1",
            RouteFamilies::V6_ONLY,
            "openrouter.ai/cdn-cgi/trace",
        ));
        match aggregate(v4, v6) {
            EgressVerdict::Blocked(o) => {
                assert_eq!(o.country_code.as_deref(), Some("HK"));
                assert_eq!(o.families, RouteFamilies::V4_ONLY);
            }
            other => panic!("v4 受限不可被另一族覆盖:{other:?}"),
        }
    }

    #[test]
    fn aggregate_both_allowed_union_families() {
        let v4 = EgressVerdict::Allowed(fam_obs("US", "203.0.113.1", RouteFamilies::V4_ONLY, "s"));
        let v6 = EgressVerdict::Allowed(fam_obs("JP", "2606:4700::1", RouteFamilies::V6_ONLY, "s"));
        match aggregate(v4, v6) {
            EgressVerdict::Allowed(o) => {
                assert_eq!(o.families, RouteFamilies::DUAL);
                assert!(o
                    .note
                    .as_deref()
                    .is_some_and(|n| n.contains("IPv6 核验通过")));
            }
            other => panic!("两族合法应放行:{other:?}"),
        }
    }

    #[test]
    fn aggregate_single_allowed_excludes_unverified_family() {
        // v4 合法 + v6 检测失败:放行但 families 只含 v4(发送层排除 v6)
        let v4 = EgressVerdict::Allowed(fam_obs("US", "203.0.113.1", RouteFamilies::V4_ONLY, "s"));
        let v6 = EgressVerdict::Unknown("检测源全部失败(v6 超时)".into());
        match aggregate(v4, v6) {
            EgressVerdict::Allowed(o) => {
                assert_eq!(o.families, RouteFamilies::V4_ONLY, "未核验族不得进许可");
                assert!(
                    o.note
                        .as_deref()
                        .is_some_and(|n| n.contains("已从发送连接排除") && n.contains("v6 超时")),
                    "{o:?}"
                );
            }
            other => panic!("已核验族合法应放行并排除未核验族:{other:?}"),
        }
        // 对称:v6 合法 + v4 失败
        let v6 = EgressVerdict::Allowed(fam_obs("JP", "2606:4700::1", RouteFamilies::V6_ONLY, "s"));
        let v4 = EgressVerdict::Unknown("检测源全部失败(v4 429)".into());
        match aggregate(v4, v6) {
            EgressVerdict::Allowed(o) => {
                assert_eq!(o.families, RouteFamilies::V6_ONLY);
                assert!(o.note.as_deref().is_some_and(|n| n.contains("IPv4")));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn aggregate_both_failed_unknown_with_both_evidences() {
        let v4 = EgressVerdict::Unknown("v4:断网".into());
        let v6 = EgressVerdict::Unknown("v6:断网".into());
        match aggregate(v4, v6) {
            EgressVerdict::Unknown(m) => {
                assert!(m.contains("IPv4") && m.contains("IPv6"), "{m}");
                assert!(m.contains("断网"), "{m}");
            }
            other => panic!("两族都失败必须 Unknown:{other:?}"),
        }
    }

    #[tokio::test]
    async fn run_detection_cancel_ends_both_families_immediately() {
        // 双栈并行 + 取消立即结束(不等待单源超时)
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let _ = sock
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
                    .await;
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
        let sources = vec![SourceSpec {
            name: "hang-src",
            url: Box::leak(format!("http://{addr}/trace").into_boxed_str()),
            parse: parse_openrouter_trace,
        }];
        let cfg = NetworkConfig::default();
        let http_v4 = cfg.detection_client_for(RouteFamilies::V4_ONLY).unwrap();
        let http_v6 = cfg.detection_client_for(RouteFamilies::V6_ONLY).unwrap();
        let limits = super::super::network::NetworkLimits {
            egress_source_timeout: Duration::from_secs(30),
            egress_total_timeout: Duration::from_secs(30),
            ..Default::default()
        };
        let cancel = CancellationToken::new();
        let cancel2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            cancel2.cancel();
        });
        let started = std::time::Instant::now();
        let v = run_detection(&http_v4, &sources, &http_v6, &sources, limits, cancel).await;
        let elapsed = started.elapsed();
        server.abort();
        assert!(
            matches!(&v, EgressVerdict::Unknown(m) if m.contains("取消")),
            "{v:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "取消应立即结束两族检测:{elapsed:?}"
        );
    }
}
