//! 纯文本附件(P7B,0.1.2)。
//!
//! 附件不改变 OpenRouter 请求格式:发送时以长分隔符拼接进 prompt,
//! 对全部模型透明(DESIGN 范围修订:首版"纯文本附件")。
//! 随消息 / 会话草稿序列化存储(JSON 列)。

use serde::{Deserialize, Serialize};

/// 单个附件大小上限(256 KB)。
pub const MAX_ATTACHMENT_BYTES: usize = 256 * 1024;
/// 附件数量上限。
pub const MAX_ATTACHMENTS: usize = 10;
/// 附件总大小上限(1 MB)。
pub const MAX_TOTAL_BYTES: usize = 1024 * 1024;

/// 纯文本附件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Attachment {
    pub name: String,
    pub content: String,
}

/// 把附件加入集合;超限返回错误说明(调用方转 toast)。
pub fn try_push(existing: &mut Vec<Attachment>, att: Attachment) -> Result<(), String> {
    if existing.len() >= MAX_ATTACHMENTS {
        return Err(format!("附件最多 {MAX_ATTACHMENTS} 个,已跳过 {}", att.name));
    }
    if att.content.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!("{} 超过单文件 256KB,已跳过", att.name));
    }
    let total: usize = existing.iter().map(|a| a.content.len()).sum();
    if total + att.content.len() > MAX_TOTAL_BYTES {
        return Err(format!("附件总计超过 1MB,已跳过 {}", att.name));
    }
    existing.push(att);
    Ok(())
}

/// 组合 prompt 与附件;无附件时原样返回。
/// 格式:`{prompt}\n\n===== 附件 N: name =====\n{content}`(长分隔符防冲突)。
pub fn compose_prompt(prompt: &str, attachments: &[Attachment]) -> String {
    let mut out = String::from(prompt);
    for (i, a) in attachments.iter().enumerate() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format!("===== 附件 {}: {} =====\n", i + 1, a.name));
        out.push_str(&a.content);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str, content: &str) -> Attachment {
        Attachment {
            name: name.into(),
            content: content.into(),
        }
    }

    #[test]
    fn no_attachments_returns_prompt_verbatim() {
        assert_eq!(compose_prompt("你好", &[]), "你好");
        assert_eq!(compose_prompt("", &[]), "");
    }

    #[test]
    fn single_attachment_uses_separator_format() {
        let out = compose_prompt("总结附件", &[att("demo.txt", "第一行")]);
        assert_eq!(out, "总结附件\n\n===== 附件 1: demo.txt =====\n第一行");
    }

    #[test]
    fn multiple_attachments_numbered_in_order() {
        let out = compose_prompt("问题", &[att("a.txt", "A"), att("b.md", "B")]);
        assert!(out.starts_with("问题\n\n===== 附件 1: a.txt =====\nA"));
        assert!(out.contains("\n\n===== 附件 2: b.md =====\nB"));
    }

    #[test]
    fn empty_prompt_no_leading_separator() {
        let out = compose_prompt("", &[att("only.txt", "内容")]);
        assert_eq!(out, "===== 附件 1: only.txt =====\n内容");
    }

    #[test]
    fn try_push_enforces_limits() {
        let mut v = Vec::new();
        assert!(try_push(&mut v, att("ok.txt", "x")).is_ok());
        // 单文件超限
        let big = att("big.txt", &"x".repeat(MAX_ATTACHMENT_BYTES + 1));
        assert!(try_push(&mut v, big).is_err());
        // 数量超限(首个之外还能加 9 个)
        for i in 0..MAX_ATTACHMENTS - 1 {
            assert!(try_push(&mut v, att(&format!("f{i}.txt"), "y")).is_ok());
        }
        assert_eq!(v.len(), MAX_ATTACHMENTS);
        assert!(try_push(&mut v, att("more.txt", "y")).is_err());
    }
}
