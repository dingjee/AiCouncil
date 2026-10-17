//! 配置文件(非敏感连接配置与通用设置)+ 平台数据目录。
//!
//! 数据源边界(PLAN 3.1):配置文件保存提供方非敏感连接配置与通用设置;
//! 数据库保存会话/偏好/目录缓存;系统凭据保存 Key。同一字段不双写。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::ProviderId;

/// 通用应用配置(data_dir/config.json)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    /// 自定义提供方(内置 OpenRouter 之外)。
    pub providers: Vec<CustomProvider>,
    pub general: GeneralSettings,
    /// UIState(当前会话、侧栏宽度等)序列化;与会话相关的树状态在 DB。
    pub window: WindowState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GeneralSettings {
    /// 并发上限(默认 4)。
    pub max_concurrent_requests: u32,
    /// 假定输出 token(预估用,默认 1000)。
    pub assumed_output_tokens: u64,
    /// 连接超时秒。
    pub connect_timeout_secs: u64,
    /// 读取空闲超时秒。
    pub read_idle_timeout_secs: u64,
}

impl GeneralSettings {
    pub fn with_defaults() -> Self {
        Self {
            max_concurrent_requests: 4,
            assumed_output_tokens: 1000,
            connect_timeout_secs: 15,
            read_idle_timeout_secs: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WindowState {
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: ProviderId,
    pub kind: String, // "openai_compatible"
    pub base_url: String,
    pub display_name: String,
}

/// 平台数据目录(%APPDATA%/AiCouncil、~/Library/Application Support/AiCouncil、
/// ~/.local/share/AiCouncil)。
pub fn data_dir() -> std::io::Result<PathBuf> {
    let dir = directories::ProjectDirs::from("", "", "AiCouncil")
        .ok_or_else(|| std::io::Error::other("无法确定平台数据目录"))?
        .data_dir()
        .to_path_buf();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 数据库文件路径。
pub fn db_path() -> std::io::Result<PathBuf> {
    Ok(data_dir()?.join("aicouncil.db"))
}

impl AppConfig {
    pub fn load_or_default() -> std::io::Result<Self> {
        let path = data_dir()?.join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                general: GeneralSettings::with_defaults(),
                ..Default::default()
            }),
            Err(e) => Err(e),
        }
    }

    /// 原子写:先写临时文件再替换,失败不破坏已有文件。
    pub fn save(&self) -> std::io::Result<()> {
        let path = data_dir()?.join("config.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_via_temp_dir() {
        // 不动真实数据目录:序列化/反序列化往返即可覆盖格式稳定性
        let mut cfg = AppConfig {
            general: GeneralSettings::with_defaults(),
            ..Default::default()
        };
        cfg.general.max_concurrent_requests = 6;
        let s = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.general.max_concurrent_requests, 6);
    }

    #[test]
    fn unknown_fields_survive_default() {
        let s = r#"{"general": {"max_concurrent_requests": 9}, "future_field": 1}"#;
        let cfg: AppConfig = serde_json::from_str(s).unwrap();
        assert_eq!(cfg.general.max_concurrent_requests, 9);
    }
}
