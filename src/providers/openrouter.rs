//! OpenRouter 适配器(P2.1/P2.2)。
//!
//! - 默认地址 `https://openrouter.ai/api/v1`;路径拼接去重 `/v1`。
//! - `/models` 拉目录;`/chat/completions` SSE 流;`/key` 与 `/credits`
//!   查 Key 配额与账户余额(权限不足正常降级)。
//! - `HTTP-Referer`/`X-Title` 可选头;Authorization 只在请求头,不进日志。

use futures_util::StreamExt;
use rust_decimal::Decimal;

use crate::domain::ProviderId;

use super::cancel::CancellationToken;
use super::catalog::parse_openrouter_models;
use super::sse::{SseMessage, SseParser};
use super::{
    CatalogSnapshot, EventStream, FinishState, GenEvent, ImmutableRequest, KeyStatus, Provider,
    ProviderError, UsageResult,
};

pub const DEFAULT_BASE_URL: &str = "https://openrouter.ai/api/v1";

/// 成功终态后的 usage 尾帧等待(5.4:尾部超时只终止等待,不翻失败)。
const USAGE_TAIL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 无内容进展上限(keepalive 不无限掩盖无进展)。
const NO_PROGRESS_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);
/// 参数合成不可覆盖的保留键(5.2(2):不能借 params 改 model 绕过门禁)。
const RESERVED_PARAM_KEYS: &[&str] = &["model", "messages", "stream"];

/// OpenRouter 提供方。`http` 可注入 mock 客户端(测试)。
pub struct OpenRouter {
    id: ProviderId,
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl OpenRouter {
    pub fn new(id: ProviderId, api_key: String) -> Self {
        Self::with_base_url(id, DEFAULT_BASE_URL.to_string(), api_key)
    }

    pub fn with_base_url(id: ProviderId, base_url: String, api_key: String) -> Self {
        let http = crate::services::network::NetworkConfig::default()
            .generation_client()
            .unwrap_or_else(|e| {
                // 统一网络配置失败:显式暴露错误,不静默退回默认直连
                tracing::error!(%e, "生成客户端构建失败");
                reqwest::Client::new()
            });
        Self {
            id,
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    pub fn with_client(
        id: ProviderId,
        http: reqwest::Client,
        base_url: String,
        api_key: String,
    ) -> Self {
        Self {
            id,
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
        }
    }

    /// 路径拼接:输入可能带或不带 /v1,避免重复。
    pub fn url(&self, path: &str) -> String {
        let base = &self.base_url;
        if path == "/models"
            || path.starts_with("/chat")
            || path.starts_with("/generation")
            || path.starts_with("/generations")
            || path == "/key"
            || path == "/credits"
        {
            // 业务路径直接拼(规范入口已含 /v1)
            format!("{base}{path}")
        } else {
            format!("{base}/{path}")
        }
    }

    fn auth(&self) -> Option<(&'static str, String)> {
        if self.api_key.is_empty() {
            None
        } else {
            Some(("Authorization", format!("Bearer {}", self.api_key)))
        }
    }

    async fn get_json(&self, url: &str) -> Result<serde_json::Value, ProviderError> {
        let mut req = self.http.get(url);
        if let Some((k, v)) = self.auth() {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| map_reqwest_err(&e))?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;
        if !(200..300).contains(&status) {
            return Err(ProviderError::from_status(status, truncate(&body, 400)));
        }
        serde_json::from_str(&body).map_err(|e| ProviderError::Parse(e.to_string()))
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let mut end = n;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

fn map_reqwest_err(e: &reqwest::Error) -> ProviderError {
    if e.is_timeout() {
        ProviderError::Timeout(e.to_string())
    } else {
        ProviderError::Network(e.to_string())
    }
}

impl Provider for OpenRouter {
    fn id(&self) -> ProviderId {
        self.id
    }

    async fn list_models(&self) -> Result<CatalogSnapshot, ProviderError> {
        let json = self.get_json(&self.url("/models")).await?;
        let entries = parse_openrouter_models(&json);
        if entries.is_empty() {
            // 空目录视为无效,不替换缓存
            return Err(ProviderError::Parse("目录为空".into()));
        }
        Ok(CatalogSnapshot {
            provider_id: self.id,
            entries,
            fetched_at: chrono::Utc::now(),
        })
    }

    fn stream(&self, req: ImmutableRequest, cancel: CancellationToken) -> EventStream {
        let http = self.http.clone();
        let url = self.url("/chat/completions");
        let api_key = self.api_key.clone();

        Box::pin(async_stream::try_stream! {
            if cancel.is_cancelled() {
                Err(ProviderError::Cancelled)?;
            }
            yield GenEvent::Started;

            let mut body = serde_json::json!({
                "model": req.model,
                "stream": true,
                "messages": build_messages(&req),
                "stream_options": {"include_usage": true},
            });
            if let Some(params) = &req.params {
                if let Some(obj) = params.as_object() {
                    for (k, v) in obj {
                        // 保留键不可被参数覆盖(不能借 params 改 model 绕过门禁)
                        if RESERVED_PARAM_KEYS.contains(&k.as_str()) {
                            tracing::warn!(param = %k, "忽略保留键覆盖");
                            continue;
                        }
                        body[k] = v.clone();
                    }
                }
            }

            let mut request = http.post(&url).json(&body).header("Accept", "text/event-stream");
            if !api_key.is_empty() {
                request = request.header("Authorization", format!("Bearer {api_key}"));
            }

            // select 分支内不能使用 `?`;先归一为普通值再处理
            let send_result = tokio::select! {
                r = request.send() => r.map_err(|e| map_reqwest_err(&e)),
                _ = cancel.cancelled() => Err(ProviderError::Cancelled),
            };
            let resp = send_result?;

            let status = resp.status().as_u16();
            let resp = if (200..300).contains(&status) {
                resp
            } else {
                let retry_after = resp
                    .headers()
                    .get("Retry-After")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok());
                let text = resp.text().await.unwrap_or_default();
                Err(ProviderError::from_http_error(status, &text, retry_after))?
            };
            let resp = resp;

            let mut parser = SseParser::new();
            let mut stream = resp.bytes_stream();
            // 终态判定:成功 Finish 只记一次;error 立即终止
            let mut finish: Option<FinishState> = None;
            let mut done_marker = false;
            let mut got_text = false;
            let mut first_remote_id: Option<String> = None;
            // 内容进展时钟:keepalive 注释不重置(C08)
            let mut last_progress = tokio::time::Instant::now();
            let mut terminal_err: Option<ProviderError> = None;

            'outer: loop {
                // 成功终态后的 usage 尾帧等待:超时只结束等待,不翻失败(C06)
                let next = if finish.is_some() {
                    match tokio::time::timeout(USAGE_TAIL_TIMEOUT, stream.next()).await {
                        Ok(n) => n,
                        Err(_) => break, // 尾部等待超时:正常结束
                    }
                } else {
                    stream.next().await
                };
                let chunk = tokio::select! {
                    c = futures_util::future::ready(next) => c,
                    _ = cancel.cancelled() => None, // 停止:保留部分内容;服务端计费不由本地判定
                };
                if cancel.is_cancelled() {
                    terminal_err = Some(ProviderError::Cancelled);
                    break;
                }
                let Some(chunk) = chunk else { break };
                let chunk = match chunk {
                    Ok(c) => c,
                    Err(e) => {
                        terminal_err = Some(ProviderError::StreamAborted(e.to_string()));
                        break;
                    }
                };

                for msg in parser.feed(&chunk) {
                    match msg {
                        SseMessage::Comment(_) => { /* keep-alive:不算内容进展 */ }
                        SseMessage::Data(data) => {
                            if data == "[DONE]" {
                                done_marker = true;
                                break;
                            }
                            if first_remote_id.is_none() {
                                if let Some(id) = extract_remote_generation_id(&data) {
                                    first_remote_id = Some(id.clone());
                                    yield GenEvent::RemoteGenerationId {
                                        remote_generation_id: id,
                                    };
                                }
                            }
                            // 一个 JSON 帧可同时含文本/usage/finish/error(5.4)
                            let events = match decode_chat_events(&data) {
                                Ok(evts) => evts,
                                Err(e) => {
                                    terminal_err = Some(e);
                                    break 'outer;
                                }
                            };
                            for ev in events {
                                match ev {
                                    ChatStreamEvent::Delta(text) => {
                                        got_text = true;
                                        last_progress = tokio::time::Instant::now();
                                        yield GenEvent::TextDelta(text)
                                    }
                                    ChatStreamEvent::Usage(u) => {
                                        yield GenEvent::UsageUpdated {
                                            prompt_tokens: u.prompt_tokens,
                                            completion_tokens: u.completion_tokens,
                                            cost: u.cost,
                                        }
                                    }
                                    ChatStreamEvent::Finish { remote_generation_id, state } => {
                                        if finish.is_none() {
                                            finish = Some(state.clone());
                                            yield GenEvent::Finished {
                                                remote_generation_id: first_remote_id.clone().or(remote_generation_id),
                                                finish_state: state,
                                            };
                                        }
                                        // usage 帧重复 finish_reason:不产生第二次终态(C05)
                                    }
                                }
                            }
                        }
                    }
                }
                if done_marker { break; }
                // 无内容进展上限:keepalive 不无限掩盖停滞(C08)
                if finish.is_none() && last_progress.elapsed() > NO_PROGRESS_LIMIT {
                    terminal_err = Some(ProviderError::Timeout(
                        "长时间无内容进展(仅 keepalive)".into(),
                    ));
                    break;
                }
            }

            if let Some(e) = terminal_err {
                Err(e)?;
            }
            match finish {
                Some(_) => { /* 成功终态;usage 已尽量等待 */ }
                None => {
                    if done_marker {
                        if got_text {
                            // 已有文本但缺成功 finish:部分中断,不当完整回答(C02)
                            Err(ProviderError::StreamAborted(
                                "流以 [DONE] 结束但没有成功终态(已有部分内容)".into(),
                            ))?;
                        }
                        // 单独 [DONE] 且无内容/成功 finish:明确空响应(C03)
                        Err(ProviderError::EmptyResponse(
                            "流以 [DONE] 结束但没有内容与成功终态".into(),
                        ))?;
                    }
                    // 连接关闭且无任何终态:流中断(不保守写 Completed)
                    Err(ProviderError::StreamAborted("连接在终态前关闭".into()))?;
                }
            }
        })
    }

    async fn query_usage(
        &self,
        remote_generation_id: &str,
    ) -> Result<Option<UsageResult>, ProviderError> {
        let mut url = reqwest::Url::parse(&self.url("/generation"))
            .map_err(|e| ProviderError::Protocol(e.to_string()))?;
        url.query_pairs_mut()
            .append_pair("id", remote_generation_id);
        let json = self.get_json(url.as_str()).await?;
        let gen = json.get("data");
        let Some(gen) = gen else {
            return Ok(None);
        };
        let nested_usage = gen.get("usage").filter(|u| u.is_object());
        let get_num = |keys: &[&str]| {
            keys.iter().find_map(|k| {
                gen.get(*k)
                    .or_else(|| nested_usage.and_then(|u| u.get(*k)))
                    .and_then(|x| {
                        x.as_u64()
                            .or_else(|| x.as_str().and_then(|s| s.parse::<u64>().ok()))
                    })
            })
        };
        let cost = gen
            .get("total_cost")
            .and_then(json_decimal)
            .or_else(|| gen.get("usage").and_then(json_decimal));
        Ok(Some(UsageResult {
            prompt_tokens: get_num(&["tokens_prompt", "native_tokens_prompt", "prompt_tokens"]),
            completion_tokens: get_num(&[
                "tokens_completion",
                "native_tokens_completion",
                "completion_tokens",
            ]),
            cost,
        }))
    }

    async fn query_key_status(&self) -> Result<KeyStatus, ProviderError> {
        let json = match self.get_json(&self.url("/key")).await {
            Ok(j) => j,
            Err(ProviderError::Unauthorized(_)) | Err(ProviderError::ModelUnavailable(_)) => {
                return Ok(KeyStatus::PermissionDenied)
            }
            Err(ProviderError::Server(..)) => return Ok(KeyStatus::Unsupported),
            Err(e) => return Err(e),
        };
        let data = json.get("data").cloned().unwrap_or_default();
        let dec = |v: Option<&serde_json::Value>| v.and_then(json_decimal);
        Ok(KeyStatus::Ok {
            label: data.get("label").and_then(|l| l.as_str()).map(String::from),
            usage: dec(data.get("usage")),
            limit: dec(data.get("limit")),
            limit_remaining: dec(data.get("limit_remaining")),
        })
    }

    async fn test_connection(&self) -> Result<(), ProviderError> {
        self.get_json(&self.url("/models")).await.map(|_| ())
    }
}

/// JSON 数值转 Decimal:字符串字面量直接解析;数字按其序列化字面解析,
/// 避免 f64 中间表示引入精度噪声。
fn json_decimal(v: &serde_json::Value) -> Option<Decimal> {
    use std::str::FromStr;
    match v {
        serde_json::Value::String(s) => Decimal::from_str(s).ok(),
        serde_json::Value::Number(n) => Decimal::from_str(&n.to_string()).ok(),
        _ => None,
    }
}

fn build_messages(req: &ImmutableRequest) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    if let Some(sys) = &req.system_prompt {
        if !sys.trim().is_empty() {
            out.push(serde_json::json!({"role": "system", "content": sys}));
        }
    }
    for m in &req.history {
        out.push(serde_json::json!({"role": m.role, "content": m.content}));
    }
    out.push(serde_json::json!({"role": "user", "content": req.prompt}));
    out
}

#[derive(Debug)]
enum ChatStreamEvent {
    Delta(String),
    Usage(UsageResult),
    Finish {
        remote_generation_id: Option<String>,
        state: FinishState,
    },
}

/// 解析 chat/completions 的一个 data JSON(5.4:多事件)。
///
/// 处理顺序:解析结构 → 提取 usage → 追加有效文本 → 判 error/finish。
/// 一个帧可同时返回文本、usage 与终态;error 优先决定终态。
/// 无效 JSON data 是 Parse 失败(注释/keepalive 不进本函数)。
fn decode_chat_events(data: &str) -> Result<Vec<ChatStreamEvent>, ProviderError> {
    let v: serde_json::Value = serde_json::from_str(data)
        .map_err(|e| ProviderError::Parse(format!("data 帧非合法 JSON:{e}")))?;

    let mut out: Vec<ChatStreamEvent> = Vec::new();

    // 顶层 error(HTTP 200 内的 SSE 错误)→ 失败
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return Err(sse_error_to_provider_error(err));
    }

    // usage(可与文本/finish 同帧;也可作为末尾独立 chunk)
    if let Some(usage) = v.get("usage").filter(|u| !u.is_null()) {
        let get = |k: &str| usage.get(k).and_then(|x| x.as_u64());
        let cost = usage
            .get("cost")
            .and_then(json_decimal)
            .or_else(|| v.get("cost").and_then(json_decimal));
        out.push(ChatStreamEvent::Usage(UsageResult {
            prompt_tokens: get("prompt_tokens"),
            completion_tokens: get("completion_tokens"),
            cost,
        }));
    }

    // 文本(终态帧上的最后一段文本不丢)
    if let Some(text) = v
        .pointer("/choices/0/delta/content")
        .and_then(|t| t.as_str())
    {
        if !text.is_empty() {
            out.push(ChatStreamEvent::Delta(text.to_string()));
        }
    }

    // choice 级 error(OpenAI 兼容端点)
    if let Some(err) = v.pointer("/choices/0/error").filter(|e| !e.is_null()) {
        return Err(sse_error_to_provider_error(err));
    }

    // finish_reason(在文本之后判定;stop 之外的合法原因区别处理)
    if let Some(fin) = v
        .pointer("/choices/0/finish_reason")
        .filter(|f| !f.is_null())
    {
        let reason = fin.as_str().unwrap_or("");
        let remote = v.get("id").and_then(|i| i.as_str()).map(String::from);
        match reason {
            "stop" | "tool_calls" => {
                out.push(ChatStreamEvent::Finish {
                    remote_generation_id: remote,
                    state: if reason == "stop" {
                        FinishState::Stop
                    } else {
                        FinishState::Other(reason.to_string())
                    },
                });
            }
            "length" => out.push(ChatStreamEvent::Finish {
                remote_generation_id: remote,
                state: FinishState::Length,
            }),
            "content_filter" => out.push(ChatStreamEvent::Finish {
                remote_generation_id: remote,
                state: FinishState::ContentFilter,
            }),
            "error" => {
                // finish_reason=error:失败,不是完成
                return Err(ProviderError::Protocol(format!(
                    "finish_reason=error:{}",
                    remote.unwrap_or_default()
                )));
            }
            other => {
                // 未知非空 finish:保守当完成,保留原因
                out.push(ChatStreamEvent::Finish {
                    remote_generation_id: remote,
                    state: FinishState::Other(other.to_string()),
                });
            }
        }
    }

    Ok(out)
}

fn extract_remote_generation_id(data: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(data)
        .ok()?
        .get("id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// SSE 错误载荷 → ProviderError(数字/字符串 code 兼容;保留原因)。
fn sse_error_to_provider_error(err: &serde_json::Value) -> ProviderError {
    let message = err
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("未知错误")
        .to_string();
    let code = err.get("code").map(|c| match c {
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        _ => String::new(),
    });
    let error_type = err
        .pointer("/metadata/error_type")
        .and_then(|m| m.as_str())
        .map(String::from);
    let mut msg = String::new();
    if let Some(et) = &error_type {
        msg.push_str(&format!("[{et}] "));
    }
    msg.push_str(&message);
    match code.as_deref() {
        Some(c) if !c.is_empty() => match c.parse::<u16>() {
            Ok(status) => ProviderError::from_status(status, msg),
            Err(_) => {
                // 非数字 provider code:按 500 类服务端错误保留 code 与原因
                ProviderError::Server(0, format!("[code {c}] {msg}"))
            }
        },
        _ => ProviderError::Server(0, msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{BatchId, ConversationId, GenerationId, MessageId};

    #[test]
    fn url_joins_without_duplicate_v1() {
        let p = OpenRouter::new(ProviderId::new(), "k".into());
        assert_eq!(p.url("/models"), "https://openrouter.ai/api/v1/models");
        let custom = OpenRouter::with_base_url(
            ProviderId::new(),
            "http://localhost:9/v1/".into(),
            "k".into(),
        );
        assert_eq!(custom.url("/models"), "http://localhost:9/v1/models");
        assert_eq!(
            custom.url("/chat/completions"),
            "http://localhost:9/v1/chat/completions"
        );
    }

    #[test]
    fn decode_multi_event_frame_and_usage_finish() {
        // 同帧 text+usage+finish(C04):三者都不丢
        let evts = decode_chat_events(
            r#"{"id":"gen-x","choices":[{"delta":{"content":"尾段"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5},"cost":0.0012}"#,
        )
        .unwrap();
        assert_eq!(evts.len(), 3, "{evts:?}");
        assert!(
            matches!(&evts[0], ChatStreamEvent::Usage(u) if u.prompt_tokens == Some(10) && u.cost == Some(Decimal::new(12, 4)))
        );
        assert!(matches!(&evts[1], ChatStreamEvent::Delta(t) if t == "尾段"));
        assert!(matches!(
            &evts[2],
            ChatStreamEvent::Finish { remote_generation_id: Some(r), state: FinishState::Stop } if r == "gen-x"
        ));

        let nested = decode_chat_events(
            r#"{"id":"gen-nested","usage":{"prompt_tokens":3,"completion_tokens":2,"cost":"0.00491"},"choices":[]}"#,
        ).unwrap();
        assert!(
            matches!(&nested[0], ChatStreamEvent::Usage(u) if u.cost == Some(Decimal::new(491, 5)))
        );
        assert_eq!(
            extract_remote_generation_id(r#"{"id":"gen-nested","error":{"message":"x"}}"#)
                .as_deref(),
            Some("gen-nested")
        );

        // Older mocks/providers used a top-level cost; keep compatibility.
        let legacy = decode_chat_events(r#"{"usage":{"prompt_tokens":1},"cost":"0.02"}"#).unwrap();
        assert!(
            matches!(&legacy[0], ChatStreamEvent::Usage(u) if u.cost == Some(Decimal::new(2, 2)))
        );

        let evts = decode_chat_events(r#"{"choices":[{"delta":{"content":"你好"}}]}"#).unwrap();
        assert!(matches!(&evts[0], ChatStreamEvent::Delta(t) if t == "你好"));

        // length/content_filter 语义保留(C07)
        let evts =
            decode_chat_events(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#).unwrap();
        assert!(matches!(
            &evts[0],
            ChatStreamEvent::Finish {
                state: FinishState::Length,
                ..
            }
        ));
    }

    #[test]
    fn decode_errors_are_failures_not_success() {
        // HTTP 200 顶层 error(C01)
        let e =
            decode_chat_events(r#"{"error":{"message":"rate limited","code":429}}"#).unwrap_err();
        assert!(matches!(e, ProviderError::RateLimited { .. }), "{e:?}");
        // choice 级 error(OpenAI 兼容)
        let e = decode_chat_events(r#"{"choices":[{"error":{"message":"bad","code":401}}]}"#)
            .unwrap_err();
        assert!(matches!(e, ProviderError::Unauthorized(_)), "{e:?}");
        // finish_reason=error 无顶层 error(C01)
        let e = decode_chat_events(r#"{"choices":[{"finish_reason":"error"}]}"#).unwrap_err();
        assert!(matches!(e, ProviderError::Protocol(_)), "{e:?}");
        // 无效 JSON(C03)
        let e = decode_chat_events("{not json").unwrap_err();
        assert!(matches!(e, ProviderError::Parse(_)), "{e:?}");
        // 字符串 code 兼容
        let e = decode_chat_events(r#"{"error":{"message":"m","code":"insufficient_credits"}}"#)
            .unwrap_err();
        assert!(
            matches!(e, ProviderError::Server(_, ref m) if m.contains("insufficient_credits")),
            "{e:?}"
        );
    }

    #[test]
    fn reserved_params_cannot_override_model() {
        // 5.2(2):构造保留键参数,验证 body 构建不覆盖(通过 stream 不可测,
        // 此处验证 RESERVED_PARAM_KEYS 过滤逻辑本身)
        let params = serde_json::json!({
            "model": "openai/gpt-4o",
            "messages": [],
            "stream": false,
            "temperature": 0.7
        });
        let mut body = serde_json::json!({
            "model": "anthropic/claude-opus-5.5",
            "stream": true,
            "messages": [1],
        });
        for (k, v) in params.as_object().unwrap() {
            if RESERVED_PARAM_KEYS.contains(&k.as_str()) {
                continue;
            }
            body[k] = v.clone();
        }
        assert_eq!(body["model"], "anthropic/claude-opus-5.5");
        assert_eq!(body["stream"], true);
        assert_eq!(body["temperature"], 0.7);
    }

    #[test]
    fn messages_order_system_history_prompt() {
        let req = ImmutableRequest {
            conversation_id: ConversationId::new(),
            batch_id: BatchId::new(),
            request_id: GenerationId::new(),
            assistant_message_id: MessageId::new(),
            model: "m".into(),
            params: None,
            system_prompt: Some("SYS".into()),
            history: vec![
                crate::domain::plan::ContextMessage::user("Q1"),
                crate::domain::plan::ContextMessage::assistant("A1"),
            ],
            prompt: "Q2".into(),
        };
        let msgs = build_messages(&req);
        let roles: Vec<&str> = msgs.iter().filter_map(|m| m["role"].as_str()).collect();
        assert_eq!(roles, vec!["system", "user", "assistant", "user"]);
        assert_eq!(msgs.last().unwrap()["content"].as_str(), Some("Q2"));
    }

    #[tokio::test]
    async fn query_usage_uses_generation_endpoint_and_official_schema() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 2048];
            let n = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..n]).to_string();
            let body = r#"{"data":{"total_cost":"0.012345","tokens_prompt":123,"tokens_completion":45,"native_tokens_prompt":120,"native_tokens_completion":43}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            request
        });
        let provider = OpenRouter::with_base_url(
            ProviderId::new(),
            format!("http://{addr}/v1"),
            "test".into(),
        );
        let usage = provider
            .query_usage("gen-1&other=bad")
            .await
            .unwrap()
            .unwrap();
        let request = server.await.unwrap();
        assert!(
            request.starts_with("GET /v1/generation?id=gen-1%26other%3Dbad "),
            "{request}"
        );
        assert_eq!(usage.cost, Some(Decimal::new(12345, 6)));
        assert_eq!(usage.prompt_tokens, Some(123));
        assert_eq!(usage.completion_tokens, Some(45));
    }

    #[tokio::test]
    async fn stream_keeps_first_frame_id_when_finish_and_usage_omit_it() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).await.unwrap();
            let body = concat!(
                "data: {\"id\":\"gen-first\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2,\"cost\":\"0.00491\"},\"choices\":[]}\n\n",
                "data: [DONE]\n\n"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let provider = OpenRouter::with_base_url(
            ProviderId::new(),
            format!("http://{addr}/v1"),
            "test".into(),
        );
        let req = ImmutableRequest {
            conversation_id: ConversationId::new(),
            batch_id: BatchId::new(),
            request_id: GenerationId::new(),
            assistant_message_id: MessageId::new(),
            model: "test-model".into(),
            params: None,
            system_prompt: None,
            history: Vec::new(),
            prompt: "hello".into(),
        };
        let mut stream = provider.stream(req, CancellationToken::new());
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event.unwrap());
        }
        server.await.unwrap();
        assert!(events.iter().any(|event| matches!(event,
            crate::providers::GenEvent::RemoteGenerationId { remote_generation_id } if remote_generation_id == "gen-first")));
        assert!(events.iter().any(|event| matches!(event,
            crate::providers::GenEvent::Finished { remote_generation_id: Some(id), .. } if id == "gen-first")));
        assert!(events.iter().any(|event| matches!(event,
            crate::providers::GenEvent::UsageUpdated { cost: Some(cost), .. } if *cost == Decimal::new(491, 5))));
    }
}
