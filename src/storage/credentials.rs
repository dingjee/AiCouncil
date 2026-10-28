//! API Key 凭据来源(macOS,0.2.1 起):稳定数据目录下的 `.env` 是**唯一**来源。
//!
//! - 具名变量:`OPENROUTER_API_KEY`、`DEEPSEEK_API_KEY`(白名单;文件由
//!   用户/部署方放置,应用只读不写,绝不打印/记录其值)。
//! - 定位与 desktop env_keys 一致(`repo::env_file_candidates`):
//!   `AICOUNCIL_ENV_PATH` > 数据目录 `.env` > exe 同级;`AICOUNCIL_DATA_DIR`
//!   设置时严格隔离、绝不回退。**第一个存在的文件**即唯一权威文件;
//!   键为空/缺失即为“未配置”,不再尝试其他文件。
//!   包内捆绑运行时的首次交付导入(包同级 `data/.env` → 稳定目录,原子、
//!   非覆盖、0600)在 `repo::import_legacy_data`,与工作目录无关。
//! - 每次查询即时重读文件(外部更新无需重启);文件整体解析后才查键:
//!   任一行语法无效即视为文件不可信(诊断 InvalidSyntax),不做部分读取。
//!   前导 UTF-8 BOM 被剥离(PC → Mac 交付常见产物),不影响首个键名。
//! - 无 Keychain/keyring/DPAPI;无写入/删除 API(0.1.x 的保存/删除入口
//!   已随 iced UI 移除,`.env` 为外部只读来源)。
//! - `CredentialDiagnostic` 安全区分:文件缺失 / 语法无效 / 变量缺失 /
//!   已加载(reload 成功)——只含类别,绝不含键值或行内容。

use std::path::{Path, PathBuf};

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

/// 凭据来源诊断(安全:只含失败/成功**类别**,绝不含键值、行内容或路径值)。
/// 用于把交付路径问题与文件语法问题、变量缺失区分开。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialDiagnostic {
    /// 权威 `.env` 不存在(稳定目录与包同级 `data/` 均未放置)。
    MissingFile,
    /// 权威 `.env` 存在但存在无法解析的行(整体视为不可信;不泄露内容)。
    InvalidSyntax,
    /// 文件可解析,但该 provider 的具名变量缺失或为空。
    AbsentVariable,
    /// 已从权威 `.env` 读取到该 provider 的非空具名变量(reload 成功)。
    Present,
}

/// 剥离前导 UTF-8 BOM 后用 dotenvy 解析整个文件。
/// 任一行不可解析 → None(整体不可信);成功 → 全部变量(按文件顺序)。
fn parsed_env_vars(path: &Path) -> Option<Vec<(String, String)>> {
    let bytes = std::fs::read(path).ok()?;
    let bytes: &[u8] = bytes
        .strip_prefix([0xEF, 0xBB, 0xBF].as_slice())
        .unwrap_or(&bytes);
    // from_read_iter 逐项产出;语法错误以 Err 项出现 → 整体不可信。
    // 注意:迭代路径不剥离 BOM(load() 才会),上面的显式剥离是必需的。
    let mut vars = Vec::new();
    for item in dotenvy::from_read_iter(std::io::Cursor::new(bytes)) {
        vars.push(item.ok()?);
    }
    Some(vars)
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
/// 权威文件缺失、文件不可解析、键缺失或为空 → None(不回退到其他候选文件)。
pub fn named_env_value(provider_ref: &str) -> Option<String> {
    let var = env_var_for_provider(provider_ref)?;
    let path = authoritative_env_file()?;
    parsed_env_vars(&path)?
        .into_iter()
        .find(|(key, _)| key == var)
        .map(|(_, value)| value)
        .filter(|value| !value.trim().is_empty())
}

/// 当前生效状态(不返回值)。优先级语义与 `named_env_value` 一致。
pub fn status(provider_ref: &str) -> CredentialStatus {
    if named_env_value(provider_ref).is_some() {
        CredentialStatus::EnvFile
    } else {
        CredentialStatus::NotConfigured
    }
}

/// 交付/故障诊断(只含类别,不含值):缺失文件、无效语法、变量缺失、已加载。
pub fn diagnose(provider_ref: &str) -> CredentialDiagnostic {
    let Some(var) = env_var_for_provider(provider_ref) else {
        // 白名单外 provider:按“无该变量”处理,不泄露任何信息
        return CredentialDiagnostic::AbsentVariable;
    };
    let Some(path) = authoritative_env_file() else {
        return CredentialDiagnostic::MissingFile;
    };
    match parsed_env_vars(&path) {
        None => CredentialDiagnostic::InvalidSyntax,
        Some(vars) => {
            if vars
                .iter()
                .any(|(key, value)| key == var && !value.trim().is_empty())
            {
                CredentialDiagnostic::Present
            } else {
                CredentialDiagnostic::AbsentVariable
            }
        }
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

    #[test]
    fn crlf_and_bom_fixtures_are_read_correctly() {
        let _g = env_guard();
        // PC → Mac 交付常见产物:CRLF 行尾 + 前导 UTF-8 BOM,键值必须可用
        let dir = fixture_env(
            "\u{feff}OPENROUTER_API_KEY=sk-fixture-crlf-or\r\nDEEPSEEK_API_KEY=sk-fixture-crlf-ds\r\n",
        );
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        assert_eq!(
            diagnose("openrouter"),
            CredentialDiagnostic::Present,
            "BOM+CRLF 文件必须可解析并命中键"
        );
        let v = named_env_value("openrouter").unwrap();
        assert_eq!(v, "sk-fixture-crlf-or");
        assert!(!v.contains('\r'), "值不得残留 CR(会污染 provider 请求头)");
        assert_eq!(
            named_env_value("deepseek").as_deref(),
            Some("sk-fixture-crlf-ds")
        );
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }

    #[test]
    fn invalid_syntax_is_reported_distinctly_and_value_never_read() {
        let _g = env_guard();
        // 一行语法无效 → 整个文件不可信:既不是 Present 也不是静默 Absent
        let dir = fixture_env("OPENROUTER_API_KEY=sk-fixture-or\nTHIS LINE IS BROKEN\n");
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        assert_eq!(diagnose("openrouter"), CredentialDiagnostic::InvalidSyntax);
        assert_eq!(named_env_value("openrouter"), None);
        assert_eq!(status("openrouter"), CredentialStatus::NotConfigured);
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }

    #[test]
    fn diagnostics_distinguish_missing_file_absent_variable_and_present() {
        let _g = env_guard();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        // 1) 文件缺失
        assert_eq!(diagnose("openrouter"), CredentialDiagnostic::MissingFile);
        // 2) 文件存在但变量缺失/为空
        std::fs::write(
            dir.path().join(".env"),
            "OTHER_VAR=1\nOPENROUTER_API_KEY=\n",
        )
        .unwrap();
        assert_eq!(diagnose("openrouter"), CredentialDiagnostic::AbsentVariable);
        assert_eq!(diagnose("deepseek"), CredentialDiagnostic::AbsentVariable);
        // 3) 变量出现 → Present(reload 成功判据)
        std::fs::write(dir.path().join(".env"), "OPENROUTER_API_KEY=sk-fixture-x\n").unwrap();
        assert_eq!(diagnose("openrouter"), CredentialDiagnostic::Present);
        assert_eq!(status("openrouter"), CredentialStatus::EnvFile);
        // 4) 空文件 → 变量缺失(文件本身可解析)
        std::fs::write(dir.path().join(".env"), "").unwrap();
        assert_eq!(diagnose("openrouter"), CredentialDiagnostic::AbsentVariable);
        // 白名单外 provider 永远只报 AbsentVariable(不泄露文件状态细节)
        assert_eq!(diagnose("custom"), CredentialDiagnostic::AbsentVariable);
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }

    #[test]
    fn env_path_takes_priority_over_data_dir_when_both_exist() {
        let _g = env_guard();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        // 两个候选都存在:第一个存在的候选(AICOUNCIL_ENV_PATH)是唯一权威
        let explicit = dir.path().join("explicit.env");
        std::fs::write(&explicit, "OPENROUTER_API_KEY=sk-fixture-explicit\n").unwrap();
        std::fs::write(
            dir.path().join(".env"),
            "OPENROUTER_API_KEY=sk-fixture-datadir\n",
        )
        .unwrap();
        std::env::set_var("AICOUNCIL_ENV_PATH", &explicit);
        assert_eq!(diagnose("openrouter"), CredentialDiagnostic::Present);
        assert_eq!(
            named_env_value("openrouter").as_deref(),
            Some("sk-fixture-explicit")
        );
        // 显式路径不存在 → 回落到数据目录候选(仍是“第一个存在的文件”)
        std::env::set_var("AICOUNCIL_ENV_PATH", dir.path().join("absent.env"));
        assert_eq!(
            named_env_value("openrouter").as_deref(),
            Some("sk-fixture-datadir")
        );
        std::env::remove_var("AICOUNCIL_ENV_PATH");
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }

    #[test]
    fn reload_changes_diagnostic_distinctly_without_value_output() {
        let _g = env_guard();
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        let path = dir.path().join(".env");
        // 全生命周期:缺失 → 语法无效 → 变量缺失 → 已加载 → 回到缺失
        assert_eq!(diagnose("deepseek"), CredentialDiagnostic::MissingFile);
        std::fs::write(&path, "NOT PARSEABLE ??=\n").unwrap();
        assert_eq!(diagnose("deepseek"), CredentialDiagnostic::InvalidSyntax);
        std::fs::write(&path, "OPENROUTER_API_KEY=sk-fixture-a\n").unwrap();
        assert_eq!(diagnose("deepseek"), CredentialDiagnostic::AbsentVariable);
        std::fs::write(
            &path,
            "OPENROUTER_API_KEY=sk-fixture-a\nDEEPSEEK_API_KEY=sk-fixture-b\n",
        )
        .unwrap();
        assert_eq!(diagnose("deepseek"), CredentialDiagnostic::Present);
        // 删除文件即回到 MissingFile(外部轮换语义,无需重启)
        std::fs::remove_file(&path).unwrap();
        assert_eq!(diagnose("deepseek"), CredentialDiagnostic::MissingFile);
        assert_eq!(status("deepseek"), CredentialStatus::NotConfigured);
        std::env::remove_var("AICOUNCIL_DATA_DIR");
    }
}
