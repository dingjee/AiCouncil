//! API Key 凭据存储:Windows 使用 DPAPI 加密文件并可导入旧 keyring；
//! 其他平台仅保留于当前进程。任何情况下不落明文文件。

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
    /// 加密存储不可用；仅当前进程缓存有效。
    #[error("安全凭据存储不可用: {0}")]
    Backend(String),
    #[error("未找到条目")]
    NotFound,
}

/// 旧版系统凭据库读取适配器，仅用于 Windows 首次导入。
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

/// Windows 使用本地 DPAPI 文件，其他平台使用仅进程内存储。
pub struct CredentialVault {
    system: SystemCredentials,
    process: ProcessCredentials,
    /// 系统库是否可用(首次探测结果缓存)。
    system_available: Mutex<Option<bool>>,
    /// DPAPI 文件路径;仅 Windows 构建读取(非 Windows 目标上无读者)。
    #[cfg_attr(not(windows), allow(dead_code))]
    local_file: Option<std::path::PathBuf>,
}

impl CredentialVault {
    pub fn new() -> Self {
        Self {
            system: SystemCredentials,
            process: ProcessCredentials::new(),
            system_available: Mutex::new(None),
            local_file: crate::storage::repo::data_dir()
                .ok()
                .map(|p| p.join("credentials.dpapi")),
        }
    }

    #[cfg(windows)]
    fn read_local(&self) -> Result<Option<HashMap<String, String>>, CredentialError> {
        let Some(path) = &self.local_file else {
            return Ok(None);
        };
        let encrypted = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(CredentialError::Backend(e.to_string())),
        };
        let plain = dpapi_unprotect(&encrypted).map_err(CredentialError::Backend)?;
        serde_json::from_slice(&plain)
            .map(Some)
            .map_err(|e| CredentialError::Backend(e.to_string()))
    }

    #[cfg(windows)]
    fn write_local(&self, values: &HashMap<String, String>) -> Result<(), CredentialError> {
        let Some(path) = &self.local_file else {
            return Err(CredentialError::Backend("exe 数据目录不可用".into()));
        };
        let plain =
            serde_json::to_vec(values).map_err(|e| CredentialError::Backend(e.to_string()))?;
        let encrypted = dpapi_protect(&plain).map_err(CredentialError::Backend)?;
        let tmp = path.with_extension("dpapi.tmp");
        std::fs::write(&tmp, encrypted).map_err(|e| CredentialError::Backend(e.to_string()))?;
        std::fs::rename(tmp, path).map_err(|e| CredentialError::Backend(e.to_string()))
    }

    /// 检测旧版凭据后端是否可用。
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
        #[cfg(windows)]
        {
            self.process.save(provider_ref, key)?;
            let mut values = match self.read_local() {
                Ok(Some(values)) => values,
                Ok(None) => HashMap::new(),
                Err(_) => return Ok(false),
            };
            values.insert(provider_ref.to_string(), key.to_string());
            Ok(self.write_local(&values).is_ok())
        }
        #[cfg(not(windows))]
        {
            self.process.save(provider_ref, key)?;
            Ok(false)
        }
    }

    /// 读取:进程内优先(用户本次显式输入),否则系统库。
    pub fn load(&self, provider_ref: &str) -> Result<Option<String>, CredentialError> {
        #[cfg(windows)]
        {
            if let Some(key) = self.process.load(provider_ref)? {
                return Ok(Some(key));
            }
            if let Ok(Some(values)) = self.read_local() {
                return Ok(values.get(provider_ref).cloned());
            }
            if std::env::var_os("AICOUNCIL_DATA_DIR").is_some()
                || std::env::var_os("AICOUNCIL_DEMO_MODE").is_some()
            {
                return Ok(None);
            }
            // One-time import from the legacy Windows Credential Manager. The
            // encrypted local file becomes authoritative after the first save.
            let legacy = self.system.load(provider_ref)?;
            if let Some(key) = &legacy {
                let _ = self.save(provider_ref, key)?;
            }
            Ok(legacy)
        }
        #[cfg(not(windows))]
        {
            if let Some(k) = self.process.load(provider_ref)? {
                return Ok(Some(k));
            }
            Ok(None)
        }
    }

    pub fn delete(&self, provider_ref: &str) -> Result<(), CredentialError> {
        self.process.delete(provider_ref)?;
        #[cfg(windows)]
        {
            let mut values = match self.read_local() {
                Ok(Some(values)) => values,
                Ok(None) => HashMap::new(),
                Err(error) => return Err(error),
            };
            values.remove(provider_ref);
            self.write_local(&values)
        }
        #[cfg(not(windows))]
        {
            Ok(())
        }
    }
}

#[cfg(windows)]
#[repr(C)]
struct DataBlob {
    size: u32,
    data: *mut u8,
}

#[cfg(windows)]
#[link(name = "Crypt32")]
unsafe extern "system" {
    fn CryptProtectData(
        input: *const DataBlob,
        description: *const u16,
        entropy: *const DataBlob,
        reserved: *mut std::ffi::c_void,
        prompt: *mut std::ffi::c_void,
        flags: u32,
        output: *mut DataBlob,
    ) -> i32;
    fn CryptUnprotectData(
        input: *const DataBlob,
        description: *mut *mut u16,
        entropy: *const DataBlob,
        reserved: *mut std::ffi::c_void,
        prompt: *mut std::ffi::c_void,
        flags: u32,
        output: *mut DataBlob,
    ) -> i32;
}
#[cfg(windows)]
#[link(name = "Kernel32")]
unsafe extern "system" {
    fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
}

#[cfg(windows)]
fn dpapi_protect(input: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(input, true)
}
#[cfg(windows)]
fn dpapi_unprotect(input: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(input, false)
}
#[cfg(windows)]
fn dpapi(input: &[u8], protect: bool) -> Result<Vec<u8>, String> {
    let source = DataBlob {
        size: input.len() as u32,
        data: input.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        size: 0,
        data: std::ptr::null_mut(),
    };
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &source,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &source,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.data, output.size as usize).to_vec() };
    unsafe {
        LocalFree(output.data.cast());
    }
    Ok(bytes)
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
        let dir = tempfile::tempdir().unwrap();
        let v = CredentialVault {
            system: SystemCredentials,
            process: ProcessCredentials::new(),
            system_available: Mutex::new(Some(false)),
            local_file: Some(dir.path().join("credentials.dpapi")),
        };
        v.save("p1", "sk-x").unwrap();
        assert_eq!(v.load("p1").unwrap().as_deref(), Some("sk-x"));
        v.delete("p1").unwrap();
        assert!(v.load("p1").unwrap().is_none());
        drop(v);
        drop(dir);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_is_encrypted() {
        let plain = b"a fake test credential";
        let encrypted = dpapi_protect(plain).unwrap();
        assert_ne!(encrypted, plain);
        assert_eq!(dpapi_unprotect(&encrypted).unwrap(), plain);
    }
}
