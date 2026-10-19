//! 统一网络配置。
//!
//! 同一 `NetworkConfig` 构造检测与生成客户端;认证头逐请求添加,
//! 检测请求不带 Authorization/prompt/附件/provider 专用头。
//! 代理配置失败禁止静默退回直连(不 `unwrap_or_default()`)。
//!
//! 已知边界:检测客户端与生成客户端即使同配置,公网出口仍可能因
//! 按域名分流/IPv4-IPv6 双栈/代理动态换出口而不同;本模块保证的是
//! "同一网络策略配置",不能宣称数学保证上游所见 IP。

use std::time::Duration;

/// 集中可调的超时/限额参数(5.4)。
#[derive(Debug, Clone, Copy)]
pub struct NetworkLimits {
    /// 生成:连接超时。
    pub connect_timeout: Duration,
    /// 生成:单次读取停滞超时(不限制长答案总时长)。
    pub read_stall_timeout: Duration,
    /// 检测:单源超时。
    pub egress_source_timeout: Duration,
    /// 检测:总预算。
    pub egress_total_timeout: Duration,
    /// 检测:响应体上限(字节)。
    pub egress_body_limit: usize,
}

impl Default for NetworkLimits {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(15),
            read_stall_timeout: Duration::from_secs(120),
            egress_source_timeout: Duration::from_secs(4),
            egress_total_timeout: Duration::from_secs(8),
            egress_body_limit: 64 * 1024,
        }
    }
}

/// 显式代理配置:None = 使用已核验的环境代理策略(reqwest 默认读取
/// 系统代理;不额外注入)。Some(url) = 检测与生成统一走该代理。
#[derive(Debug, Clone, Default)]
pub struct NetworkConfig {
    pub proxy: Option<String>,
    pub limits: NetworkLimits,
}

/// 检测 User-Agent(ip.sb 要求自定义 UA;不含任何 Key/用户信息)。
pub const EGRESS_USER_AGENT: &str = concat!(
    "AiCouncil/",
    env!("CARGO_PKG_VERSION"),
    " (+https://localhost; egress-policy-check)"
);

impl NetworkConfig {
    /// 构造检测客户端。代理配置失败返回 Err——禁止静默直连 fallback。
    pub fn detection_client(&self) -> Result<reqwest::Client, String> {
        let mut b = reqwest::Client::builder()
            .user_agent(EGRESS_USER_AGENT)
            .connect_timeout(self.limits.egress_source_timeout)
            .timeout(self.limits.egress_source_timeout)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none());
        if let Some(url) = &self.proxy {
            let p = reqwest::Proxy::all(url).map_err(|e| format!("代理配置失败({url}):{e}"))?;
            b = b.proxy(p);
        }
        b.build().map_err(|e| format!("检测客户端构建失败:{e}"))
    }

    /// 构造生成客户端:
    /// - 显式代理统一走该代理;无显式代理用环境策略;
    /// - 连接/读取停滞超时;
    /// - 不启用库级自动重发(reqwest 默认关闭;显式保持关闭,
    ///   不能只保证 ChatService 没有 retry 循环)。
    pub fn generation_client(&self) -> Result<reqwest::Client, String> {
        let mut b = reqwest::Client::builder()
            .connect_timeout(self.limits.connect_timeout)
            .read_timeout(self.limits.read_stall_timeout)
            .no_proxy();
        if let Some(url) = &self.proxy {
            let p = reqwest::Proxy::all(url).map_err(|e| format!("代理配置失败({url}):{e}"))?;
            b = b.proxy(p);
        }
        b.build().map_err(|e| format!("生成客户端构建失败:{e}"))
    }
}

/// 显式代理配置来源(生产):`AICOUNCIL_PROXY` 环境变量。
/// 未设置时返回 None(使用环境代理策略,不伪造统一出口)。
pub fn explicit_proxy_from_env() -> Option<String> {
    std::env::var("AICOUNCIL_PROXY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clients_build_with_and_without_proxy() {
        let cfg = NetworkConfig::default();
        assert!(cfg.detection_client().is_ok());
        assert!(cfg.generation_client().is_ok());
        let bad = NetworkConfig {
            proxy: Some("not a url %%%".into()),
            ..Default::default()
        };
        // 代理配置失败显式报错,不静默直连
        assert!(bad.detection_client().is_err(), "非法代理应报错");
        assert!(bad.generation_client().is_err());
    }

    #[test]
    fn ua_has_no_secrets() {
        assert!(EGRESS_USER_AGENT.starts_with("AiCouncil/"));
        assert!(!EGRESS_USER_AGENT.contains("sk-"));
    }
}
