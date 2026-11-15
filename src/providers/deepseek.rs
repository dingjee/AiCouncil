//! DeepSeek 标题辅助客户端(PLAN 7.3)。
//!
//! - 凭据:`.env` 为唯一权威凭据源(读取具名变量;不探测进程环境)
//!   路径中的同名变量(开发=项目根,发布=exe 同级);**不导入全部
//!   dotenv 环境**,密钥不进 Debug/tracing/config/SQL/UI。
//! - `POST {base}/chat/completions`,`model=deepseek-flash`(可 env 覆盖),
//!   `stream=false`,`thinking={"type":"disabled"}`,输出上限 80 tokens;
//!   连接 5s / 总 12s;endpoint 可注入(本地 mock)。
//! - 返回检查:仅接受一行非空文本,去首尾空白与成对外层引号,
//!   ≤48 grapheme;多段/解释/空响应/仅 reasoning_content 视为失败。

use std::time::Duration;

/// 官方推荐模型 ID(实施时核对;可用 AICOUNCIL_TITLE_MODEL 覆盖)。
pub const DEFAULT_TITLE_MODEL: &str = "deepseek-flash";
pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
/// 标题最大长度(grapheme)。
pub const MAX_TITLE_GRAPHEMES: usize = 48;
/// 标题请求连接超时(实测本网络下 DeepSeek 首字节即可达 ~4.9s,
/// 旧的 5s 连接超时把可用请求判成 timeout,故放宽)。
pub const TITLE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// 标题请求总超时(单次尝试;不自动重试付费请求)。
pub const TITLE_TOTAL_TIMEOUT: Duration = Duration::from_secs(45);

/// 失败类别(只存类别,不存密钥/提问全文/原始 response)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleErrorKind {
    NoCredentials,
    Unauthorized,
    RateLimited,
    Server,
    Timeout,
    Network,
    InvalidResponse,
}

impl TitleErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NoCredentials => "no_credentials",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate_limited",
            Self::Server => "server",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

/// 解析标题 API Key:`.env` 为唯一权威凭据源(统一走共享
/// credentials loader;只读取具名变量,不打印值)。
pub fn load_api_key() -> Option<String> {
    crate::storage::credentials::named_env_value("deepseek")
}

/// 可注入 endpoint 的标题客户端(测试/离线 mock:`new` 传 base)。
pub struct DeepSeekTitleClient {
    http: reqwest::Client,
    base: String,
    key: String,
    model: String,
    /// 单次请求总超时(测试可调短;生产 = TITLE_TOTAL_TIMEOUT)。
    total_timeout: Duration,
}

#[derive(Debug, serde::Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    stream: bool,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<serde_json::Value>,
    messages: [ChatMessage<'a>; 2],
}

#[derive(Debug, serde::Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, serde::Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, serde::Deserialize)]
struct ChatChoice {
    message: ChatReply,
}

#[derive(Debug, serde::Deserialize)]
struct ChatReply {
    content: Option<String>,
    #[serde(default)]
    #[allow(dead_code)] // 仅需识别字段存在(反序列化兼容)
    reasoning_content: Option<String>,
}

/// system prompt(7.3):只输出标题、与提问同语言、不回答不解释。
pub const SYSTEM_PROMPT: &str = "Generate a short chat title for the user's question. Reply with ONLY the title text: same language as the question, no answer, no explanation, no quotes, no Markdown. Chinese: 8-16 characters. Other languages: 3-8 words.";

impl DeepSeekTitleClient {
    /// 生产构造;无凭据返回 None(上层走 fallback + 下次重试)。
    pub fn from_env() -> Option<Self> {
        let key = load_api_key()?;
        let base = std::env::var("AICOUNCIL_TITLE_BASE_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Some(Self::new(base, key))
    }

    pub fn new(base: String, key: String) -> Self {
        let model = std::env::var("AICOUNCIL_TITLE_MODEL")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_TITLE_MODEL.to_string());
        // 标题专用连接超时(15s;实测首字节即需 ~4.9s,旧的 5s 会误杀)。
        let limits = crate::services::network::NetworkLimits {
            connect_timeout: TITLE_CONNECT_TIMEOUT,
            ..Default::default()
        };
        let http = crate::services::network::NetworkConfig {
            // 与检测/生成同一代理策略:AICOUNCIL_PROXY 显式代理统一生效,
            // 无显式代理时尊重环境/系统代理(不再各建一套配置)。
            proxy: crate::services::network::explicit_proxy_from_env(),
            limits,
        }
        .generation_client()
        .expect("标题客户端构建失败(网络配置非法,拒绝静默直连)");
        Self {
            http,
            base: base.trim_end_matches('/').to_string(),
            key,
            model,
            total_timeout: TITLE_TOTAL_TIMEOUT,
        }
    }

    /// 台账记录用模型名(不入 UI tooltip 等敏感面)。
    pub fn model_name(&self) -> &str {
        &self.model
    }
}

impl DeepSeekTitleClient {
    /// 请求标题(数据边界:只发有限片段,不发历史/系统提示/附件正文)。
    pub async fn generate_title(&self, question: &str) -> Result<String, TitleErrorKind> {
        // 数据边界:只发有限片段,不发历史/系统提示/附件正文(7.3)
        let excerpt = excerpt_of(question, 2000);
        let body = ChatRequest {
            model: &self.model,
            stream: false,
            max_tokens: 80,
            thinking: Some(serde_json::json!({"type": "disabled"})),
            messages: [
                ChatMessage {
                    role: "system",
                    content: SYSTEM_PROMPT,
                },
                ChatMessage {
                    role: "user",
                    content: &excerpt,
                },
            ],
        };
        let url = format!("{}/chat/completions", self.base);
        // 总预算强制(45s 生产;含连接、响应与 body 读取)。
        let fut = self.do_request(&url, &body);
        match tokio::time::timeout(self.total_timeout, fut).await {
            Ok(r) => r,
            Err(_) => Err(TitleErrorKind::Timeout),
        }
    }

    async fn do_request(
        &self,
        url: &str,
        body: &ChatRequest<'_>,
    ) -> Result<String, TitleErrorKind> {
        let resp = self
            .http
            .post(url)
            .bearer_auth(&self.key)
            .json(body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    TitleErrorKind::Timeout
                } else {
                    TitleErrorKind::Network
                }
            })?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(TitleErrorKind::Unauthorized);
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(TitleErrorKind::RateLimited);
        }
        if !status.is_success() {
            return Err(TitleErrorKind::Server);
        }
        let parsed: ChatResponse = resp.json().await.map_err(|e| {
            // body 阶段超时也是 Timeout,不再误报 InvalidResponse。
            if e.is_timeout() {
                TitleErrorKind::Timeout
            } else {
                TitleErrorKind::InvalidResponse
            }
        })?;
        let content = parsed
            .choices
            .first()
            .and_then(|c| c.message.content.clone())
            .unwrap_or_default();
        // 仅 reasoning_content / 空响应 → 失败
        sanitize_title(&content).ok_or(TitleErrorKind::InvalidResponse)
    }
}

/// 有限片段(合计上限 max graphemes;首行优先)。
fn excerpt_of(text: &str, max: usize) -> String {
    let first_line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let mut out: String = first_line.chars().take(max).collect();
    if out.is_empty() {
        out = text.chars().take(max).collect();
    }
    out
}

/// 清洗模型返回:单行、去首尾空白与成对外层引号、≤48 grapheme。
pub fn sanitize_title(raw: &str) -> Option<String> {
    let mut t = raw.trim();
    // 多段内容视为解释 → 失败
    if t.lines().count() > 1 {
        return None;
    }
    let pairs = [('"', '"'), ('“', '”'), ('‘', '’'), ('\'', '\'')];
    for (open, close) in pairs {
        if t.starts_with(open) && t.ends_with(close) && t.chars().count() >= 2 {
            t = t[open.len_utf8()..t.len() - close.len_utf8()].trim();
            break;
        }
    }
    if t.is_empty() {
        return None;
    }
    let count = t.chars().count();
    if count > MAX_TITLE_GRAPHEMES {
        return None;
    }
    Some(t.to_string())
}

/// fallback 标题(7.2):首个非空行、合并空白、32 grapheme 截断省略;
/// 仅附件时取首个附件名;兜底空(用本地化 New chat)。
pub fn fallback_title(prompt: &str, attachments: &[crate::domain::Attachment]) -> Option<String> {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.is_empty() {
        return attachments.first().map(|a| a.name.clone());
    }
    let merged: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = merged.chars().collect();
    if chars.len() <= 32 {
        Some(merged)
    } else {
        Some(format!("{}…", chars[..32].iter().collect::<String>()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_accepts_single_line_strips_quotes() {
        assert_eq!(sanitize_title("  简洁标题  ").as_deref(), Some("简洁标题"));
        assert_eq!(sanitize_title("\"quoted\"").as_deref(), Some("quoted"));
        assert_eq!(sanitize_title("“中文引号”").as_deref(), Some("中文引号"));
    }

    #[test]
    fn sanitize_rejects_multi_line_empty_and_overlong() {
        assert!(sanitize_title("").is_none());
        assert!(sanitize_title("   ").is_none());
        assert!(sanitize_title("第一行\n第二行").is_none());
        let long = "字".repeat(MAX_TITLE_GRAPHEMES + 1);
        assert!(sanitize_title(&long).is_none());
        // 非 ASCII 语言不因长度规则丢失(48 以内)
        let ok = "длинный заголовок".to_string();
        assert_eq!(sanitize_title(&ok).as_deref(), Some(ok.as_str()));
    }

    #[test]
    fn fallback_first_line_truncated_to_32_graphemes() {
        assert_eq!(
            fallback_title("  第一行\n第二行", &[]).as_deref(),
            Some("第一行")
        );
        let long = "非".repeat(40);
        let f = fallback_title(&long, &[]).unwrap();
        assert_eq!(f.chars().count(), 33); // 32 + …
        assert!(f.ends_with('…'));
        // 仅附件 → 附件名
        let att = vec![crate::domain::Attachment {
            name: "notes.md".into(),
            content: "x".into(),
        }];
        assert_eq!(fallback_title("", &att).as_deref(), Some("notes.md"));
        assert!(fallback_title("", &[]).is_none());
    }

    #[test]
    fn excerpt_is_bounded() {
        let long = "问".repeat(5000);
        assert_eq!(excerpt_of(&long, 2000).chars().count(), 2000);
    }
}
