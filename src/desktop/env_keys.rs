//! 具名变量解析——运行时凭据唯一权威源(委托 GLM 的共享 loader)。
//!
//! 安全约束(user override):
//! - `.env`(`OPENROUTER_API_KEY` / `DEEPSEEK_API_KEY`)是两个提供方的
//!   唯一权威存储;本模块不实现第二套解析,统一走
//!   `storage::credentials::CredentialVault`(即时重读 `.env`);
//! - 只按**变量名**读取,绝不打印值;测试只用伪造 fixture `.env`;
//! - 无 Keychain、无进程覆盖存储。

use crate::storage::credentials;

pub const OPENROUTER_ENV_NAME: &str = "OPENROUTER_API_KEY";
pub const DEEPSEEK_ENV_NAME: &str = "DEEPSEEK_API_KEY";

const OPENROUTER_REF: &str = "openrouter";
const DEEPSEEK_REF: &str = "deepseek";

fn provider_ref_for_name(name: &str) -> Option<&'static str> {
    match name {
        OPENROUTER_ENV_NAME => Some(OPENROUTER_REF),
        DEEPSEEK_ENV_NAME => Some(DEEPSEEK_REF),
        _ => None,
    }
}

/// 从 `.env` 读取具名变量(不存在/为空 → None)。返回值只用于
/// 内存中的提供方客户端构造,不日志、不序列化。
pub fn load_named_key(name: &str) -> Option<String> {
    let provider_ref = provider_ref_for_name(name)?;
    credentials::named_env_value(provider_ref)
}

/// 具名变量是否已配置(只返回布尔;不暴露值)。
pub fn has_named_key(name: &str) -> bool {
    load_named_key(name).is_some()
}
