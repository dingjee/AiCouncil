//! Anthropic 全模型出口门禁。
//!
//! - 主服务 `https://api.ip.sb/geoip`,备服务 `https://ipapi.co/json/`;
//!   仅主服务不可用/数据无效时备用;有效 CN/HK 不可被备用覆盖。
//! - 仅屏蔽 CN/HK;明确有效的其他 ISO 国家/地区放行;超时/断网/
//!   429/无国家码/无效 IP/保留地址/解析失败/冲突 → Unknown 拦截。
//! - 检测结果仅短期内存,不持久化;检测请求不带 Authorization/
//!   prompt/附件。
//!
//! 出口保证边界:GeoIP 查到的是访问检测服务的公网出口。按域名分流、
//! IPv4/IPv6 不同出口、代理动态换出口仍可能与上游不一致;本模块落地
//! 的是"发请求前检查当前网络策略下的检测出口",不宣称保证上游所见 IP。

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
    /// 明确有效且非 CN/HK:放行本次动作。
    Allowed(EgressObservation),
    /// 明确 CN/HK:拦截。
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
}

/// 短期许可:仅同 intent、同 network_revision 的批内使用;
/// 不跨动作缓存 Allowed(4.1)。
#[derive(Debug, Clone, Copy)]
pub struct EgressPermit {
    /// 单调时钟检测时刻(处理系统时间跳变)。
    pub checked_at_mono: tokio::time::Instant,
    pub revision: u64,
}

pub const PERMIT_TTL: Duration = Duration::from_secs(3);

impl EgressPermit {
    pub fn fresh(revision: u64) -> Self {
        Self {
            checked_at_mono: tokio::time::Instant::now(),
            revision,
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

/// 生产 GeoIP 门禁:主 ip.sb,备 ipapi.co。
pub struct GeoIpGuard {
    http: reqwest::Client,
    revision: AtomicU64,
}

impl GeoIpGuard {
    pub fn new(config: &NetworkConfig) -> Result<Self, String> {
        Ok(Self {
            http: config.detection_client()?,
            revision: AtomicU64::new(0),
        })
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
        let http = self.http.clone();
        Box::pin(async move { run_detection(&http, cancel).await })
    }

    fn network_revision(&self) -> u64 {
        self.revision.load(Ordering::SeqCst)
    }
}

/// 单源原始观测/失败。
enum SourceOutcome {
    Ok(EgressObservation),
    Invalid(String),
    Failed(String),
}

/// 一个 GeoIP 数据源的 schema 适配器。
trait GeoSource {
    const NAME: &'static str;
    const URL: &'static str;
    /// 从 JSON 提取 ip + country_code;字段可能缺失。
    fn parse(json: &serde_json::Value) -> SourceOutcome;
}

struct IpSb;
impl GeoSource for IpSb {
    const NAME: &'static str = "ip.sb";
    const URL: &'static str = "https://api.ip.sb/geoip";
    fn parse(json: &serde_json::Value) -> SourceOutcome {
        // ip.sb 200 也可能带 error 字段
        if json.get("error").is_some() {
            return SourceOutcome::Invalid("ip.sb 返回 error".into());
        }
        common_parse(json, Self::NAME)
    }
}

struct IpApiCo;
impl GeoSource for IpApiCo {
    const NAME: &'static str = "ipapi.co";
    const URL: &'static str = "https://ipapi.co/json/";
    fn parse(json: &serde_json::Value) -> SourceOutcome {
        // ipapi.co 限流/错误时 HTTP 200 内含 {"error": true, "reason": ...}
        if json.get("error").and_then(|e| e.as_bool()).unwrap_or(false) {
            let reason = json
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");
            return SourceOutcome::Invalid(format!("ipapi.co error: {reason}"));
        }
        common_parse(json, Self::NAME)
    }
}

fn common_parse(json: &serde_json::Value, source: &'static str) -> SourceOutcome {
    let ip = json.get("ip").and_then(|v| v.as_str()).map(String::from);
    let cc = json
        .get("country_code")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_uppercase());
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
    SourceOutcome::Ok(EgressObservation {
        ip,
        country_code: cc,
        source,
        checked_at: chrono::Utc::now(),
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
                !(v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xfe00) == 0xfc00)
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

async fn fetch_source<S: GeoSource>(
    http: &reqwest::Client,
    cancel: &CancellationToken,
    timeout: Duration,
) -> SourceOutcome {
    let _ = cancel;
    let fut = async {
        let resp = http
            .get(S::URL)
            .send()
            .await
            .map_err(|e| format!("网络错误:{e}"))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            // 429/5xx/HTML 错误页等:该源本次不可用
            return Err(format!("HTTP {status}"));
        }
        let has_ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("json"))
            .unwrap_or(false);
        let body = read_limited(
            resp,
            super::network::NetworkLimits::default().egress_body_limit,
        )
        .await?;
        if !has_ct && !body.trim_start().starts_with('{') {
            return Err("非 JSON 响应(HTML 错误页等)".to_string());
        }
        let json: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| format!("JSON 解析失败:{e}"))?;
        Ok(S::parse(&json))
    };
    // 内层 Result<String> = 该源失败/无效原因
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(e)) => SourceOutcome::Failed(e),
        Err(_) => SourceOutcome::Failed(format!("{} 超时", S::NAME)),
    }
}

fn classify(obs: EgressObservation) -> EgressVerdict {
    if BLOCKED_COUNTRIES.contains(&obs.country_code.as_deref().unwrap_or("")) {
        EgressVerdict::Blocked(obs)
    } else {
        EgressVerdict::Allowed(obs)
    }
}

/// 检测序列:主源 → (不可用/无效时)备源。
/// 每源最多一次,不递归重试;总预算 8 秒由调用方传入的超时控制。
async fn run_detection(http: &reqwest::Client, cancel: CancellationToken) -> EgressVerdict {
    let limits = super::network::NetworkLimits::default();
    let per_source = limits.egress_source_timeout;
    let miss = |o: &SourceOutcome| -> String {
        match o {
            SourceOutcome::Failed(e) | SourceOutcome::Invalid(e) => e.clone(),
            SourceOutcome::Ok(_) => String::new(),
        }
    };
    let run = async {
        let primary = fetch_source::<IpSb>(http, &cancel, per_source).await;
        match primary {
            SourceOutcome::Ok(obs) => classify(obs),
            primary_miss => {
                // 主服务不可用/数据无效 → 备用一次
                let secondary = fetch_source::<IpApiCo>(http, &cancel, per_source).await;
                match secondary {
                    SourceOutcome::Ok(obs) => classify(obs),
                    sec_miss => EgressVerdict::Unknown(format!(
                        "主源失败({});备源失败({})",
                        miss(&primary_miss),
                        miss(&sec_miss)
                    )),
                }
            }
        }
    };
    // 总预算(8s);取消立即返回
    tokio::select! {
        v = run => v,
        _ = cancel.cancelled() => EgressVerdict::Unknown("检测已取消".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(cc: &str) -> EgressObservation {
        EgressObservation {
            ip: Some("203.0.113.10".into()),
            country_code: Some(cc.into()),
            source: "test",
            checked_at: chrono::Utc::now(),
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
        assert!(matches!(IpSb::parse(&j), SourceOutcome::Invalid(_)));
        let j = serde_json::json!({"ip": "1.1.1.1", "country_code": "ZZ"});
        assert!(matches!(IpSb::parse(&j), SourceOutcome::Invalid(_)));
        let j = serde_json::json!({"ip": "127.0.0.1", "country_code": "US"});
        assert!(matches!(IpSb::parse(&j), SourceOutcome::Invalid(_)));
        let j = serde_json::json!({"ip": "10.0.0.5", "country_code": "US"});
        assert!(matches!(IpSb::parse(&j), SourceOutcome::Invalid(_)));
        let j = serde_json::json!({"error": true, "reason": "rate limited"});
        assert!(matches!(IpApiCo::parse(&j), SourceOutcome::Invalid(_)));
        // 小写国家码规范化
        let j = serde_json::json!({"ip": "203.0.113.9", "country_code": "jp"});
        assert!(matches!(IpSb::parse(&j), SourceOutcome::Ok(_)));
    }

    #[test]
    fn permit_ttl_and_revision() {
        let mut p = EgressPermit::fresh(0);
        assert!(p.still_valid(0));
        assert!(!p.still_valid(1), "revision 变化即失效");
        p.checked_at_mono = tokio::time::Instant::now() - PERMIT_TTL - Duration::from_millis(1);
        assert!(!p.still_valid(0), "过期失效");
    }
}
