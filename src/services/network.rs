//! 统一网络配置。
//!
//! 同一 `NetworkConfig` 构造检测与生成客户端;认证头逐请求添加,
//! 检测请求不带 Authorization/prompt/附件/provider 专用头。
//! 代理配置失败禁止静默退回直连(不 `unwrap_or_default()`)。
//!
//! 已知边界:检测客户端与生成客户端即使同配置,公网出口仍可能因
//! 按域名分流/IPv4-IPv6 双栈/代理动态换出口而不同;本模块保证的是
//! "同一网络策略配置",不能宣称数学保证上游所见 IP。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

/// 已核验可用的出网连接地址族集合(IPv4/IPv6)。
///
/// 这是检测与生成**共用**的连接约束:检测客户端按该集合过滤解析结果,
/// 生成客户端用同一过滤构造,因此"许可授权的族"与"实际发送可用族"
/// 一致(按构造一致,不靠事后断言)。
///
/// 注意:`DUAL` 表示两族连接都被核验过或都不可单独归因(如本地代理
/// 模式下"族"指到代理的本地连接族,远端出口由代理决定)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteFamilies {
    v4: bool,
    v6: bool,
}

impl RouteFamilies {
    pub const V4_ONLY: Self = Self {
        v4: true,
        v6: false,
    };
    pub const V6_ONLY: Self = Self {
        v4: false,
        v6: true,
    };
    pub const DUAL: Self = Self { v4: true, v6: true };

    pub fn allows_v4(&self) -> bool {
        self.v4
    }

    pub fn allows_v6(&self) -> bool {
        self.v6
    }

    /// 单族判定(是否只允许一族)。
    pub fn is_single(&self) -> bool {
        (self.v4 || self.v6) && !(self.v4 && self.v6)
    }

    pub fn union(&self, other: Self) -> Self {
        Self {
            v4: self.v4 || other.v4,
            v6: self.v6 || other.v6,
        }
    }

    /// 证据串用短标签。
    pub fn label(&self) -> &'static str {
        match (self.v4, self.v6) {
            (true, true) => "v4+v6",
            (true, false) => "v4",
            (false, true) => "v6",
            (false, false) => "none",
        }
    }
}

impl Default for RouteFamilies {
    fn default() -> Self {
        Self::DUAL
    }
}

/// 地址族过滤解析器:先走系统解析(getaddrinfo,与默认 GaiResolver
/// 同源,遵循 /etc/hosts 与系统 DNS),再按允许族过滤;过滤后为空则
/// 显式报错(该地址族在本机解析层面不可用,而不是静默换族)。
///
/// 该过滤器同样作用于代理模式下"到代理的连接"(reqwest 经解析器
/// 解析的正是它要连接的 host:直连时为目标域,代理时为代理 host),
/// 因此与生成客户端保持按构造一致。
///
/// 已知边界:hyper-util 对 IP 字面量主机(如 `127.0.0.1`)跳过解析器
/// 直连,不过滤;生产检测源与业务域名均为域名,不受影响。
#[derive(Debug, Clone, Copy)]
struct FamilyFilterResolver {
    allow_v4: bool,
    allow_v6: bool,
}

impl reqwest::dns::Resolve for FamilyFilterResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        let allow_v4 = self.allow_v4;
        let allow_v6 = self.allow_v6;
        Box::pin(async move {
            let host_for_blocking = host.clone();
            let resolved: Vec<SocketAddr> =
                tokio::task::spawn_blocking(move || -> Result<Vec<SocketAddr>, std::io::Error> {
                    use std::net::ToSocketAddrs;
                    Ok((host_for_blocking.as_str(), 0).to_socket_addrs()?.collect())
                })
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))??;
            let filtered: Vec<SocketAddr> = resolved
                .into_iter()
                .filter(|a| (a.is_ipv4() && allow_v4) || (a.is_ipv6() && allow_v6))
                .collect();
            if filtered.is_empty() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!(
                        "AiCouncil 地址族策略:主机 {host} 在允许族(v4={allow_v4},v6={allow_v6})内无解析结果"
                    ),
                )
                .into());
            }
            let addrs: reqwest::dns::Addrs = Box::new(filtered.into_iter());
            Ok(addrs)
        })
    }
}

/// 按地址族约束包一层解析器;DUAL 保持默认解析器(行为不变)。
fn apply_family_filter(
    b: reqwest::ClientBuilder,
    families: RouteFamilies,
) -> reqwest::ClientBuilder {
    if families == RouteFamilies::DUAL {
        return b;
    }
    b.dns_resolver(Arc::new(FamilyFilterResolver {
        allow_v4: families.allows_v4(),
        allow_v6: families.allows_v6(),
    }))
}

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

/// 显式代理配置:None = 使用已核验的环境代理策略(reqwest 依赖
/// `system-proxy` 特性读取环境/系统代理;不额外注入)。
/// Some(url) = 检测与生成统一走该代理。
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
    /// 不强制 `no_proxy`:尊重环境/系统代理策略(reqwest `system-proxy`)。
    pub fn detection_client(&self) -> Result<reqwest::Client, String> {
        self.detection_client_for(RouteFamilies::DUAL)
    }

    /// 构造限定地址族的检测客户端(`DUAL` 与 [`Self::detection_client`] 相同)。
    pub fn detection_client_for(&self, families: RouteFamilies) -> Result<reqwest::Client, String> {
        let mut b = reqwest::Client::builder()
            .user_agent(EGRESS_USER_AGENT)
            .connect_timeout(self.limits.egress_source_timeout)
            .timeout(self.limits.egress_source_timeout)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(url) = &self.proxy {
            let p = reqwest::Proxy::all(url).map_err(|e| format!("代理配置失败({url}):{e}"))?;
            b = b.proxy(p);
        }
        apply_family_filter(b, families)
            .build()
            .map_err(|e| format!("检测客户端构建失败:{e}"))
    }

    /// 构造生成客户端:
    /// - 显式代理统一走该代理;无显式代理用环境/系统代理策略
    ///   (与检测客户端同一解析路径,出口策略一致);
    /// - 连接/读取停滞超时;
    /// - 不启用库级自动重发(reqwest 默认关闭;显式保持关闭,
    ///   不能只保证 ChatService 没有 retry 循环)。
    pub fn generation_client(&self) -> Result<reqwest::Client, String> {
        self.generation_client_for(RouteFamilies::DUAL)
    }

    /// 构造限定地址族的生成客户端(`DUAL` 与 [`Self::generation_client`] 相同)。
    /// 与同族检测客户端使用同一过滤,保证"核验过的族 = 发送可用族"。
    pub fn generation_client_for(
        &self,
        families: RouteFamilies,
    ) -> Result<reqwest::Client, String> {
        let mut b = reqwest::Client::builder()
            .connect_timeout(self.limits.connect_timeout)
            .read_timeout(self.limits.read_stall_timeout);
        if let Some(url) = &self.proxy {
            let p = reqwest::Proxy::all(url).map_err(|e| format!("代理配置失败({url}):{e}"))?;
            b = b.proxy(p);
        }
        apply_family_filter(b, families)
            .build()
            .map_err(|e| format!("生成客户端构建失败:{e}"))
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
        assert!(bad.generation_client_for(RouteFamilies::V4_ONLY).is_err());
        assert!(bad.generation_client_for(RouteFamilies::V6_ONLY).is_err());
    }

    #[test]
    fn ua_has_no_secrets() {
        assert!(EGRESS_USER_AGENT.starts_with("AiCouncil/"));
        assert!(!EGRESS_USER_AGENT.contains("sk-"));
    }

    #[test]
    fn route_families_logic() {
        assert!(RouteFamilies::DUAL.allows_v4() && RouteFamilies::DUAL.allows_v6());
        assert!(RouteFamilies::V4_ONLY.is_single());
        assert!(RouteFamilies::V6_ONLY.is_single());
        assert!(!RouteFamilies::DUAL.is_single());
        assert_eq!(
            RouteFamilies::V4_ONLY.union(RouteFamilies::V6_ONLY),
            RouteFamilies::DUAL
        );
        assert_eq!(
            RouteFamilies::V4_ONLY.union(RouteFamilies::V4_ONLY).label(),
            "v4"
        );
        assert_eq!(RouteFamilies::default(), RouteFamilies::DUAL);
    }

    #[tokio::test]
    async fn family_filtered_clients_only_reach_their_family() {
        // 已知边界:hyper-util 对 IP 字面量主机(127.0.0.1/[::1])跳过
        // 解析器,族过滤只作用于域名连接。故用域名 `localhost`
        // (macOS 同时解析出 127.0.0.1 与 ::1)+ 单族绑定的服务器验证。
        let l4 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let v4_port = l4.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = l4.accept().await else { break };
                drop(s);
            }
        });
        let l6 = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
        let v6_port = l6.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((s, _)) = l6.accept().await else { break };
                drop(s);
            }
        });
        let cfg = NetworkConfig::default();
        let v4c = cfg.generation_client_for(RouteFamilies::V4_ONLY).unwrap();
        let v6c = cfg.generation_client_for(RouteFamilies::V6_ONLY).unwrap();
        let connect_ok = |result: Result<reqwest::Response, reqwest::Error>| match result {
            Ok(_) => true,
            Err(e) => !e.to_string().contains("地址族策略"),
        };
        // 同族可达
        let ok4 = connect_ok(v4c.get(format!("http://localhost:{v4_port}/")).send().await);
        let ok6 = connect_ok(v6c.get(format!("http://localhost:{v6_port}/")).send().await);
        assert!(ok4, "v4-only 客户端应可经 localhost 命中 v4 服务器");
        assert!(ok6, "v6-only 客户端应可经 localhost 命中 v6 服务器");
        // 跨族不可达(服务器只绑了另一族端口,过滤后只剩被拒地址)
        let cross1 = v4c.get(format!("http://localhost:{v6_port}/")).send().await;
        assert!(cross1.is_err(), "v4-only 客户端不应命中 v6-only 服务器");
        let cross2 = v6c.get(format!("http://localhost:{v4_port}/")).send().await;
        assert!(cross2.is_err(), "v6-only 客户端不应命中 v4-only 服务器");
    }
}
