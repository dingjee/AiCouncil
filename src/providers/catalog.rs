//! 目录缓存与偏好合成(P2.3 / PLAN 4.3)。
//!
//! - TTL 24h;成功取得完整有效目录后原子替换缓存,失败保留旧数据及时间。
//! - CatalogModel 与 ModelPreference 分离:目录更新不改勾选/默认/排序/覆盖。
//! - 新增模型默认不启用;消失模型标记“目录中未找到”,不静默换 ID。
//! - 参数 Auto:服务端有默认值则用之;没有则省略可选字段,不伪造。

use std::collections::HashMap;

use crate::domain::{Pricing, ProviderId};

use super::{CatalogEntry, CatalogSnapshot};

pub const CATALOG_TTL: chrono::Duration = chrono::Duration::hours(24);

/// 用户偏好(与目录分表;目录刷新不触碰)。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelPreference {
    pub enabled: bool,
    pub sort_order: i64,
    pub is_default: bool,
    /// 高级覆盖(仅能力允许时发送)。
    pub overrides: Option<serde_json::Value>,
}

/// 目录缓存(内存镜像 + 原子替换;跨任务共享故用 RwLock)。
pub struct CatalogCache {
    pub provider_id: ProviderId,
    snapshot: std::sync::RwLock<Option<CatalogSnapshot>>,
    /// 上次成功更新时间(失败保留)。
    last_success: std::sync::RwLock<Option<chrono::DateTime<chrono::Utc>>>,
    /// 并发刷新合并:同一时刻只有一个刷新实际执行。
    lock: tokio::sync::Mutex<()>,
}

impl CatalogCache {
    pub fn new(provider_id: ProviderId) -> Self {
        Self {
            provider_id,
            snapshot: std::sync::RwLock::new(None),
            last_success: std::sync::RwLock::new(None),
            lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> Option<CatalogSnapshot> {
        self.snapshot
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn entries(&self) -> Vec<CatalogEntry> {
        self.snapshot().map(|s| s.entries).unwrap_or_default()
    }

    pub fn last_success(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        *self.last_success.read().unwrap_or_else(|p| p.into_inner())
    }

    /// 是否到期需要刷新(启动/恢复联网时按 TTL;持续运行由调度器周期触发)。
    pub fn needs_refresh(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        match self.last_success() {
            None => true,
            Some(t) => now.signed_duration_since(t) >= CATALOG_TTL,
        }
    }

    /// 并发刷新合并:同一时刻只有一个刷新在跑,后来者等结果。
    pub async fn refresh<F, Fut>(&self, fetch: F, force: bool, now: chrono::DateTime<chrono::Utc>)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<CatalogSnapshot, super::ProviderError>>,
    {
        if !force && !self.needs_refresh(now) {
            return;
        }
        let _guard = self.lock.lock().await;
        // 拿到锁后再检查(前一个刷新可能已完成)
        if !force && !self.needs_refresh(now) {
            return;
        }
        if let Ok(snap) = fetch().await {
            self.replace(snap, now);
        }
        // 失败:保留旧数据与时间,不把模型标为下架
    }

    /// 原子替换(成功路径)。
    pub fn replace(&self, snap: CatalogSnapshot, now: chrono::DateTime<chrono::Utc>) {
        *self.snapshot.write().unwrap_or_else(|p| p.into_inner()) = Some(snap);
        *self.last_success.write().unwrap_or_else(|p| p.into_inner()) = Some(now);
    }
}

/// 偏好表(provider+model 键)。
#[derive(Default)]
pub struct Preferences {
    map: HashMap<(ProviderId, String), ModelPreference>,
}

impl Preferences {
    pub fn get(&self, provider: ProviderId, model: &str) -> ModelPreference {
        self.map
            .get(&(provider, model.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    pub fn set(&mut self, provider: ProviderId, model: &str, pref: ModelPreference) {
        self.map.insert((provider, model.to_string()), pref);
    }

    /// 启用的模型(聊天选择器只显示这些),按 sort_order。
    pub fn enabled(&self, provider: ProviderId) -> Vec<String> {
        let mut out: Vec<(i64, String)> = self
            .map
            .iter()
            .filter(|((p, _), pref)| *p == provider && pref.enabled)
            .map(|((_, m), pref)| (pref.sort_order, m.clone()))
            .collect();
        out.sort();
        out.into_iter().map(|(_, m)| m).collect()
    }

    /// 默认模型(唯一;无则 None,UI 引导选择)。
    pub fn default_model(&self, provider: ProviderId) -> Option<String> {
        self.map
            .iter()
            .filter(|((p, _), pref)| *p == provider && pref.is_default)
            .map(|((_, m), _)| m.clone())
            .next()
    }

    /// 目录替换后重算“目录中未找到”标记,不改变任何偏好字段。
    pub fn catalog_status(
        &self,
        provider: ProviderId,
        snapshot: &CatalogSnapshot,
    ) -> HashMap<String, bool> {
        let mut out = HashMap::new();
        for ((p, m), pref) in &self.map {
            if *p == provider {
                let found = snapshot.entries.iter().any(|e| &e.model_id == m);
                out.insert(m.clone(), found || !pref.enabled);
            }
        }
        out
    }
}

/// 参数合成:Auto(服务端默认)与用户覆盖的合并结果。
/// - Auto:服务端 defaults 里的值直接采用;没有则省略(不伪造)。
/// - Override:仅在能力允许列表内放行,否则忽略并报告。
pub fn compose_params(
    entry: &CatalogEntry,
    pref: &ModelPreference,
) -> (Option<serde_json::Value>, Vec<String /* 被拒绝的覆盖键 */>) {
    let mut rejected = Vec::new();
    let server_defaults = entry.server_defaults.clone().unwrap_or_default();
    let server_obj = server_defaults.as_object().cloned().unwrap_or_default();

    if let Some(overrides) = pref.overrides.as_ref().and_then(|v| v.as_object()) {
        let mut merged = serde_json::Map::new();
        // 服务端默认(仅目录声明的可选参数键)
        for (k, v) in &server_obj {
            merged.insert(k.clone(), v.clone());
        }
        // 用户覆盖:能力允许才放行
        for (k, v) in overrides {
            let allowed = entry.capabilities.iter().any(|c| c == k)
                // 通用键(OpenRouter 通行)
                || matches!(k.as_str(), "temperature" | "max_tokens" | "top_p");
            if allowed {
                merged.insert(k.clone(), v.clone());
            } else {
                rejected.push(k.clone());
            }
        }
        if merged.is_empty() {
            (None, rejected)
        } else {
            (Some(serde_json::Value::Object(merged)), rejected)
        }
    } else if server_obj.is_empty() {
        (None, rejected)
    } else {
        (Some(serde_json::Value::Object(server_obj)), rejected)
    }
}

/// 有效输出上限:min(模型 max_output, 上下文余量)。
pub fn effective_max_output(entry: &CatalogEntry, context_used_tokens: Option<u64>) -> Option<u64> {
    let model_cap = entry.max_output?;
    let Some(used) = context_used_tokens else {
        return Some(model_cap);
    };
    let window = entry.context_window?;
    Some(model_cap.min(window.saturating_sub(used)))
}

/// 从 OpenRouter /models JSON 解析目录条目。
pub fn parse_openrouter_models(json: &serde_json::Value) -> Vec<CatalogEntry> {
    let Some(arr) = json.get("data").and_then(|d| d.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|m| {
            let model_id = m.get("id")?.as_str()?.to_string();
            let name = m
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or(&model_id)
                .to_string();
            let context_window = m.get("context_length").and_then(|c| c.as_u64());
            let max_output = m
                .get("top_provider")
                .and_then(|t| t.get("max_completion_tokens"))
                .and_then(|c| c.as_u64());
            let pricing = parse_openrouter_pricing(m.get("pricing"));
            let capabilities = m
                .get("supported_parameters")
                .and_then(|s| s.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Some(CatalogEntry {
                model_id,
                name,
                context_window,
                max_output,
                pricing,
                capabilities,
                server_defaults: None,
                available: true,
            })
        })
        .collect()
}

fn parse_openrouter_pricing(v: Option<&serde_json::Value>) -> Pricing {
    use std::str::FromStr;
    let per_mtok = rust_decimal::Decimal::from(1_000_000u64);
    let get = |key: &str| {
        v.and_then(|p| p.get(key))
            .and_then(|x| x.as_str())
            .and_then(|s| rust_decimal::Decimal::from_str(s).ok())
            .map(|p| p * per_mtok)
    };
    Pricing {
        prompt_per_mtok: get("prompt"),
        completion_per_mtok: get("completion"),
        request_fee: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(model: &str, caps: Vec<&str>, defaults: Option<serde_json::Value>) -> CatalogEntry {
        CatalogEntry {
            model_id: model.into(),
            name: model.into(),
            context_window: Some(8192),
            max_output: Some(1024),
            pricing: Pricing::default(),
            capabilities: caps.into_iter().map(String::from).collect(),
            server_defaults: defaults,
            available: true,
        }
    }

    #[test]
    fn auto_uses_server_defaults_only() {
        let e = entry(
            "m",
            vec!["temperature"],
            Some(serde_json::json!({"temperature": 0.7})),
        );
        let pref = ModelPreference::default();
        let (params, rejected) = compose_params(&e, &pref);
        assert!(rejected.is_empty());
        assert_eq!(params, Some(serde_json::json!({"temperature": 0.7})));
    }

    #[test]
    fn auto_without_defaults_omits_optionals() {
        let e = entry("m", vec![], None);
        let (params, _) = compose_params(&e, &ModelPreference::default());
        assert_eq!(params, None, "不伪造 temperature");
    }

    #[test]
    fn override_respects_capabilities() {
        let e = entry("m", vec!["reasoning"], None);
        let pref = ModelPreference {
            overrides: Some(
                serde_json::json!({"reasoning": {"effort": "high"}, "temperature": 0.9}),
            ),
            ..Default::default()
        };
        let (params, rejected) = compose_params(&e, &pref);
        let obj = params.unwrap();
        assert!(obj.get("reasoning").is_some());
        assert!(obj.get("temperature").is_some(), "temperature 属通用键");
        assert!(rejected.is_empty());

        let e2 = entry("m", vec![], None);
        let (_, rejected2) = compose_params(&e2, &pref);
        assert!(rejected2.contains(&"reasoning".to_string()));
    }

    #[test]
    fn effective_output_capped_by_context_room() {
        let mut e = entry("m", vec![], None);
        e.context_window = Some(1000);
        e.max_output = Some(900);
        assert_eq!(effective_max_output(&e, Some(800)), Some(200));
        assert_eq!(effective_max_output(&e, Some(1200)), Some(0));
        assert_eq!(effective_max_output(&e, None), Some(900));
    }

    #[test]
    fn preferences_enable_and_default() {
        let mut p = Preferences::default();
        let pid = ProviderId::new();
        p.set(
            pid,
            "a",
            ModelPreference {
                enabled: true,
                sort_order: 2,
                ..Default::default()
            },
        );
        p.set(
            pid,
            "b",
            ModelPreference {
                enabled: true,
                sort_order: 1,
                is_default: true,
                ..Default::default()
            },
        );
        assert_eq!(p.enabled(pid), vec!["b".to_string(), "a".to_string()]);
        assert_eq!(p.default_model(pid).as_deref(), Some("b"));
    }

    #[test]
    fn openrouter_models_parse() {
        let json = serde_json::json!({
            "data": [
                {
                    "id": "test/mini",
                    "name": "Test Mini",
                    "context_length": 32768,
                    "top_provider": {"max_completion_tokens": 4096},
                    "pricing": {"prompt": "0.00005", "completion": "0.0002"},
                    "supported_parameters": ["temperature", "max_tokens"]
                }
            ]
        });
        let entries = parse_openrouter_models(&json);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.model_id, "test/mini");
        assert_eq!(e.context_window, Some(32768));
        assert_eq!(e.max_output, Some(4096));
        assert_eq!(
            e.pricing.prompt_per_mtok,
            Some(rust_decimal::Decimal::from(50))
        );
        assert_eq!(e.capabilities, vec!["temperature", "max_tokens"]);
    }

    #[tokio::test]
    async fn catalog_refresh_failure_keeps_old() {
        let cache = CatalogCache::new(ProviderId::new());
        let now = chrono::Utc::now();
        cache
            .refresh(
                || async {
                    Ok(CatalogSnapshot {
                        provider_id: ProviderId::new(),
                        entries: vec![entry("a", vec![], None)],
                        fetched_at: now,
                    })
                },
                true,
                now,
            )
            .await;
        assert_eq!(cache.snapshot().unwrap().entries.len(), 1);
        // 失败刷新:保留旧数据
        cache
            .refresh(
                || async { Err(super::super::ProviderError::Network("x".into())) },
                true,
                now,
            )
            .await;
        assert_eq!(cache.snapshot().unwrap().entries.len(), 1, "失败不清空目录");
        assert!(!cache.needs_refresh(now));
    }
}
