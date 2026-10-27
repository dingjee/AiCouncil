//! API Key 凭据来源(macOS,0.2.0 起):稳定数据目录下的 `.env` 是**唯一**来源。
//!
//! - 具名变量:`OPENROUTER_API_KEY`、`DEEPSEEK_API_KEY`(白名单;文件由
//!   用户/部署方放置,应用只读不写,绝不打印/记录其值)。
//! - 定位与 desktop env_keys 一致(`repo::env_file_candidates`):
//!   `AICOUNCIL_ENV_PATH` > 数据目录 `.env` > exe 同级;`AICOUNCIL_DATA_DIR`
//!   设置时严格隔离、绝不回退。**第一个存在的文件**即唯一权威文件;
//!   键为空/缺失即为“未配置”,不再尝试其他文件。
//! - 每次查询即时重读文件(外部更新无需重启)。
//! - 无 Keychain/keyring/DPAPI;无写入/删除 API(0.1.x 的保存/删除入口
//!   已随 iced UI 移除,`.env` 为外部只读来源)。

use std::path::PathBuf;

/// `.env` 具名变量白名单:provider_ref → 环境变量名。仅这两个具名来源。
const ENV_PROVIDERS: [(&str, &str); 2] = [
    ("openrouter", "OPENROUTER_API_KEY"),
    ("deepseek", "DEEPSEEK_API_KEY"),
];

/// provider_ref 对应的 `.env` 具名变量(白名单外返回 None)。
pub fn env_var_for_provider(provider_ref: &str) -> Option<&'static str> {
    ENV_PROVIDERS
        .iter()
        .find(|(ref_name, _)| *ref_name == provider_ref)
        .map(|(_, var)| *var)
}

/// 凭据当前是否由 `.env` 提供(不返回值本身;供 UI 显示“已配置”状态)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStatus {
    /// 权威 `.env` 中该具名变量存在且非空。
    EnvFile,
    /// 无任何来源。
    NotConfigured,
}

/// 唯一权威 `.env` 文件:候选中**第一个存在**者;都不存在则 None。
/// 之后的一切读取都只针对该文件(不因键缺失而回退到其他候选)。
pub fn authoritative_env_file() -> Option<PathBuf> {
    crate::storage::repo::env_file_candidates()
        .ok()?
        .into_iter()
        .find(|p| p.is_file())
}

/// 读取权威 `.env` 中该 provider 的具名变量值;只读,值不落日志。
/// 权威文件缺失、键缺失或为空 → None(不回退到其他候选文件)。
pub fn named_env_value(provider_ref: &str) -> Option<String> {
    let var = env_var_for_provider(provider_ref)?;
    let path = authoritative_env_file()?;
    let iter = dotenvy::from_path_iter(&path).ok()?;
    for (key, value) in iter.flatten() {
        if key == var && !value.trim().is_empty() {
            return Some(value);
        }
    }
    None
}

/// 当前生效状态(不返回值)。优先级语义与 `named_env_value` 一致。
pub fn status(provider_ref: &str) -> CredentialStatus {
    if named_env_value(provider_ref).is_some() {
        CredentialStatus::EnvFile
    } else {
        CredentialStatus::NotConfigured
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::repo::test_sync::ENV_LOCK;
    use std::sync::MutexGuard;

    fn env_guard() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 在隔离临时目录写入**假** .env fixture(绝不触碰真实凭据文件)。
    fn fixture_env(content: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), content).unwrap();
        dir
    }

    #[test]
    fn env_is_the_sole_source_for_named_providers() {
        let _g = env_guard();
        let dir = fixture_env("OPENROUTER_API_KEY=sk-fixture-or\nDEEPSEEK_API_KEY=sk-fixture-ds\n");
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        assert_eq!(
            named_env_value("openrouter").as_deref(),
            Some("sk-fixture-or")
        );
        assert_eq!(
            named_env_value("deepseek").as_deref(),
            Some("sk-fixture-ds")
        );
        assert_eq!(status("openrouter"), CredentialStatus::EnvFile);
        // 白名单外 provider 不从 .env 读取
        assert_eq!(named_env_value("custom-provider"), None);
        assert_eq!(status("custom-provider"), CredentialStatus::NotConfigured);
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }

    #[test]
    fn reload_rereads_env_without_restart() {
        let _g = env_guard();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        assert_eq!(named_env_value("openrouter"), None);
        // 外部写入 .env 后,无需重启即可读到(重新加载语义)
        std::fs::write(
            dir.path().join(".env"),
            "OPENROUTER_API_KEY=sk-fixture-new\n",
        )
        .unwrap();
        assert_eq!(
            named_env_value("openrouter").as_deref(),
            Some("sk-fixture-new")
        );
        // 外部清空后同样即时反映
        std::fs::write(dir.path().join(".env"), "# rotated out\n").unwrap();
        assert_eq!(named_env_value("openrouter"), None);
        assert_eq!(status("openrouter"), CredentialStatus::NotConfigured);
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }

    #[test]
    fn empty_key_in_authoritative_file_does_not_fall_through() {
        let _g = env_guard();
        // 权威文件(数据目录)存在但键为空:不得回退到 exe 同级等其他候选
        let dir = fixture_env("OPENROUTER_API_KEY=\n");
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        std::env::set_var("AICOUNCIL_ENV_PATH", "/nonexistent/.env");
        assert_eq!(
            authoritative_env_file().as_deref(),
            Some(dir.path().join(".env").as_path())
        );
        assert_eq!(named_env_value("openrouter"), None);
        assert_eq!(status("openrouter"), CredentialStatus::NotConfigured);
        std::env::remove_var("AICOUNCIL_ENV_PATH");
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }

    #[test]
    fn data_dir_override_is_strictly_isolated() {
        let _g = env_guard();
        // AICOUNCIL_DATA_DIR 设置时:候选只有 [AICOUNCIL_ENV_PATH?, 数据目录/.env],
        // 绝不包含 exe 同级/CWD —— 测试永不触达真实私有文件。
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        let candidates = crate::storage::repo::env_file_candidates().unwrap();
        assert_eq!(candidates, vec![dir.path().join(".env")]);
        // 显式 AICOUNCIL_ENV_PATH 排在最前
        let explicit = dir.path().join("explicit.env");
        std::env::set_var("AICOUNCIL_ENV_PATH", &explicit);
        let candidates = crate::storage::repo::env_file_candidates().unwrap();
        assert_eq!(candidates, vec![explicit.clone(), dir.path().join(".env")]);
        std::env::remove_var("AICOUNCIL_ENV_PATH");
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }

    #[test]
    fn env_var_mapping_is_allowlisted() {
        assert_eq!(
            env_var_for_provider("openrouter"),
            Some("OPENROUTER_API_KEY")
        );
        assert_eq!(env_var_for_provider("deepseek"), Some("DEEPSEEK_API_KEY"));
        assert_eq!(env_var_for_provider("other"), None);
    }
}
