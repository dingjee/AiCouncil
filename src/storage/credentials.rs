//! API Key 凭据存储:系统凭据库(keyring)优先,不可用时支持
//! 仅当前进程输入(P1.3)。任何情况下不落明文文件。

use std::collections::HashMap;
use std::sync::Mutex;

const SERVICE: &str = "AiCouncil";

/// 凭据存取。
pub trait CredentialStore: Send + Sync {
    fn save(&self, provider_ref: &str, key: &str) -> Result<(), CredentialError>;
    fn load(&self, provider_ref: &str) -> Result<Option<String>, CredentialError>;
    fn delete(&self, provider_ref: &str) -> Result<(), CredentialError>;
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// 系统凭据库不可用(未解锁/服务缺失);可回退进程内输入。
    #[error("系统凭据存储不可用: {0}")]
    Backend(String),
    #[error("未找到条目")]
    NotFound,
}

/// 系统凭据库实现(Windows 凭据管理器 / macOS 钥匙串 / Linux secret service)。
pub struct SystemCredentials;

impl CredentialStore for SystemCredentials {
    fn save(&self, provider_ref: &str, key: &str) -> Result<(), CredentialError> {
        keyring::Entry::new(SERVICE, provider_ref)
            .and_then(|e| e.set_password(key))
            .map_err(|e| CredentialError::Backend(e.to_string()))
    }

    fn load(&self, provider_ref: &str) -> Result<Option<String>, CredentialError> {
        match keyring::Entry::new(SERVICE, provider_ref).and_then(|e| e.get_password()) {
            Ok(k) => Ok(Some(k)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(CredentialError::Backend(e.to_string())),
        }
    }

    fn delete(&self, provider_ref: &str) -> Result<(), CredentialError> {
        match keyring::Entry::new(SERVICE, provider_ref).and_then(|e| e.delete_credential()) {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(CredentialError::Backend(e.to_string())),
        }
    }
}

/// 仅当前进程有效的内存凭据(凭据库不可用时的回退;重启后需重新输入)。
#[derive(Default)]
pub struct ProcessCredentials {
    inner: Mutex<HashMap<String, String>>,
}

impl ProcessCredentials {
    pub fn new() -> Self {
        Self::default()
    }
}

impl CredentialStore for ProcessCredentials {
    fn save(&self, provider_ref: &str, key: &str) -> Result<(), CredentialError> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(provider_ref.to_string(), key.to_string());
        Ok(())
    }

    fn load(&self, provider_ref: &str) -> Result<Option<String>, CredentialError> {
        Ok(self
            .inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(provider_ref)
            .cloned())
    }

    fn delete(&self, provider_ref: &str) -> Result<(), CredentialError> {
        self.inner
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(provider_ref);
        Ok(())
    }
}

/// 组合:系统库优先,失败读进程内;保存时尝试系统库,失败则进程内。
pub struct CredentialVault {
    system: SystemCredentials,
    process: ProcessCredentials,
    /// 系统库是否可用(首次探测结果缓存)。
    system_available: Mutex<Option<bool>>,
}

impl CredentialVault {
    pub fn new() -> Self {
        Self {
            system: SystemCredentials,
            process: ProcessCredentials::new(),
            system_available: Mutex::new(None),
        }
    }

    /// 探测系统凭据库是否可用(用一个探测条目)。
    pub fn system_available(&self) -> bool {
        let mut flag = self
            .system_available
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(v) = *flag {
            return v;
        }
        let probe = "__probe__";
        let ok = self
            .system
            .save(probe, "1")
            .and_then(|_| self.system.delete(probe))
            .is_ok();
        *flag = Some(ok);
        ok
    }

    pub fn save(&self, provider_ref: &str, key: &str) -> Result<bool, CredentialError> {
        if self.system_available() {
            match self.system.save(provider_ref, key) {
                Ok(()) => return Ok(true),
                Err(_) => {
                    // 系统库运行中失效:回退进程内并标记不可用
                    *self
                        .system_available
                        .lock()
                        .unwrap_or_else(|p| p.into_inner()) = Some(false);
                }
            }
        }
        self.process.save(provider_ref, key)?;
        Ok(false)
    }

    /// 读取:进程内优先(用户本次显式输入),否则系统库。
    pub fn load(&self, provider_ref: &str) -> Result<Option<String>, CredentialError> {
        if let Some(k) = self.process.load(provider_ref)? {
            return Ok(Some(k));
        }
        if self.system_available() {
            return self.system.load(provider_ref);
        }
        Ok(None)
    }

    pub fn delete(&self, provider_ref: &str) -> Result<(), CredentialError> {
        self.process.delete(provider_ref)?;
        if self.system_available() {
            self.system.delete(provider_ref)?;
        }
        Ok(())
    }
}

impl Default for CredentialVault {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_credentials_roundtrip() {
        let c = ProcessCredentials::new();
        c.save("p1", "sk-test").unwrap();
        assert_eq!(c.load("p1").unwrap().as_deref(), Some("sk-test"));
        assert_eq!(c.load("p2").unwrap(), None);
        c.delete("p1").unwrap();
        assert_eq!(c.load("p1").unwrap(), None);
    }

    #[test]
    fn vault_prefers_process_input() {
        // CI/无钥匙串环境下 system_available() 为 false,进程内路径生效
        let v = CredentialVault::new();
        v.save("p1", "sk-x").unwrap();
        assert_eq!(v.load("p1").unwrap().as_deref(), Some("sk-x"));
        v.delete("p1").unwrap();
        assert!(v.load("p1").unwrap().is_none());
    }
}
