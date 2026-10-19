//! Markdown 导出(PLAN 4.4 / E1)。
//!
//! - scope:Main(当前主分支)/ Selected(选中节点)/ AllBranches(所有叶子)。
//! - 每节从根重建完整历史,顺序稳定;允许重复公共历史,便于独立阅读。
//! - 保留原始 Markdown 与 LaTeX;附角色、模型、时间、状态;
//!   失败/中断标记;不导出密钥/请求头/调试信息。
//! - 一致快照;UTF-8 临时文件写入后原子替换;文件名去平台非法字符。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::domain::plan::ContextMessage;
use crate::domain::{Generation, GenerationStatus, Message, MessageId, MessageTree, Role};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportScope {
    /// 当前主分支。
    Main,
    /// 用户选中的分支叶子(P4 多选)。
    Selected(Vec<MessageId>),
    /// 所有叶子,包括失败/中断记录。
    AllBranches,
}

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("会话无消息")]
    Empty,
}

pub struct ExportResult {
    pub path: PathBuf,
    pub sections: usize,
}

/// 单条消息的展示元数据。
pub struct MessageMeta {
    pub model: Option<String>,
    pub status: Option<GenerationStatus>,
}

/// 导出输入的一致快照。
pub struct ExportInput<'a> {
    pub title: &'a str,
    pub tree: &'a MessageTree,
    pub generations: &'a [Generation],
    /// 主分支叶子(会话 main_leaf)。
    pub main_leaf: Option<MessageId>,
}

impl<'a> ExportInput<'a> {
    fn meta_of(&self, id: MessageId) -> MessageMeta {
        // 最新一代(重试取最新)
        let g = self
            .generations
            .iter()
            .filter(|g| g.assistant_message_id == id)
            .max_by_key(|g| g.started_at.unwrap_or_else(crate::storage::now_dt));
        MessageMeta {
            model: g.map(|g| g.model_id.clone()),
            status: g.map(|g| g.status),
        }
    }
}

/// 生成 Markdown 文本(scope 决定节数)。
pub fn render_markdown(input: &ExportInput<'_>, scope: ExportScope) -> Result<String, ExportError> {
    let leaves: Vec<MessageId> = match scope {
        ExportScope::Main => {
            let Some(leaf) = input.main_leaf else {
                return Err(ExportError::Empty);
            };
            vec![leaf]
        }
        ExportScope::Selected(ids) => {
            if ids.is_empty() {
                return Err(ExportError::Empty);
            }
            ids
        }
        ExportScope::AllBranches => input.tree.leaves(),
    };
    if leaves.is_empty() {
        return Err(ExportError::Empty);
    }

    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", sanitize_title(input.title)));
    out.push_str(&format!(
        "> 导出时间:{} · 分支数:{}\n\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        leaves.len()
    ));

    for (i, leaf) in leaves.iter().enumerate() {
        let path = input
            .tree
            .path_from_root(*leaf)
            .map_err(|_| ExportError::Empty)?;
        if leaves.len() > 1 {
            out.push_str(&format!("## 分支 {}\n\n", i + 1));
        }
        for id in path {
            let Some(m) = input.tree.get(id) else {
                continue;
            };
            push_message(&mut out, m, input.meta_of(id), id);
        }
        out.push('\n');
    }
    Ok(out)
}

fn push_message(out: &mut String, m: &Message, meta: MessageMeta, _id: MessageId) {
    match m.role {
        Role::User => {
            out.push_str(&format!("**用户** · {}\n\n", fmt_time(m.created_at)));
            out.push_str(&m.content);
            // P7B:附件全文附在 user 文本后
            for (i, a) in m.attachments.iter().enumerate() {
                out.push_str(&format!(
                    "\n\n[附件 {}: {}]\n\n{}\n",
                    i + 1,
                    a.name,
                    a.content
                ));
            }
            out.push_str("\n\n");
        }
        Role::Assistant => {
            let mut header = String::from("**回答**");
            if let Some(model) = &meta.model {
                header.push_str(&format!(" · {model}"));
            }
            header.push_str(&format!(" · {}", fmt_time(m.created_at)));
            match meta.status {
                Some(GenerationStatus::Failed) => header.push_str(" · ❌ 失败"),
                Some(GenerationStatus::Cancelled) => header.push_str(" · ⏹ 已取消(部分内容)"),
                Some(GenerationStatus::Interrupted) => header.push_str(" · ⚡ 中断(部分内容)"),
                Some(GenerationStatus::Queued)
                | Some(GenerationStatus::Connecting)
                | Some(GenerationStatus::Streaming) => header.push_str(" · ⏳ 未完成"),
                _ => {}
            }
            out.push_str(&header);
            out.push_str("\n\n");
            if m.content.trim().is_empty() {
                let note = match meta.status {
                    Some(GenerationStatus::Failed) => "*(无内容)*",
                    _ => "*(未收到内容)*",
                };
                out.push_str(note);
            } else {
                out.push_str(&m.content);
            }
            out.push_str("\n\n");
        }
    }
}

fn fmt_time(t: chrono::DateTime<chrono::Utc>) -> String {
    t.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn sanitize_title(t: &str) -> String {
    let t = t.trim();
    if t.is_empty() {
        "未命名会话".to_string()
    } else {
        t.to_string()
    }
}

/// 写入目标:UTF-8 临时文件 + 原子替换;失败不破坏已有文件。
pub fn write_markdown(path: &Path, content: &str) -> Result<(), ExportError> {
    let tmp = path.with_extension("md.tmp");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.flush()?;
    drop(f);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 导出文件名:标题 + 时间戳,去除平台非法字符。
pub fn default_filename(title: &str, scope: ExportScope) -> String {
    let base = sanitize_title(title);
    let base: String = base
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    let tag = match scope {
        ExportScope::Main => "main",
        ExportScope::Selected(_) => "selected",
        ExportScope::AllBranches => "all",
    };
    format!(
        "{base}-{tag}-{}.md",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    )
}

/// 组合:渲染 + 选择路径 + 写入。
pub fn export(
    input: &ExportInput<'_>,
    scope: ExportScope,
    path: &Path,
) -> Result<ExportResult, ExportError> {
    let content = render_markdown(input, scope.clone())?;
    write_markdown(path, &content)?;
    let sections = match scope {
        ExportScope::Main => 1,
        ExportScope::Selected(ids) => ids.len(),
        ExportScope::AllBranches => input.tree.leaves().len(),
    };
    Ok(ExportResult {
        path: path.to_path_buf(),
        sections,
    })
}

/// 供上层重建分支历史的便捷(与 Planner 一致的顺序;user 消息组合附件)。
pub fn history_of(tree: &MessageTree, leaf: Option<MessageId>) -> Vec<ContextMessage> {
    let Some(leaf) = leaf else { return Vec::new() };
    tree.path_from_root(leaf)
        .ok()
        .map(|path| {
            path.into_iter()
                .filter_map(|id| {
                    tree.get(id).map(|m| {
                        let content = if m.role == Role::User {
                            crate::domain::attachment::compose_prompt(&m.content, &m.attachments)
                        } else {
                            m.content.clone()
                        };
                        ContextMessage {
                            role: m.role.as_str().to_string(),
                            content,
                        }
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{BatchId, ConversationId, GenerationId, ProviderId};

    fn tree_meta() -> (MessageTree, Vec<Generation>) {
        let mut tree = MessageTree::new();
        let conv = ConversationId::new();
        let now = chrono::Utc::now();
        let q = MessageId::new();
        let a1 = MessageId::new();
        let a2 = MessageId::new();
        let q2 = MessageId::new();
        let a3 = MessageId::new();
        for (id, parent, role, content) in [
            (q, None, Role::User, "问题 $x^2$"),
            (a1, Some(q), Role::Assistant, "回答A \\frac{1}{2}"),
            (a2, Some(q), Role::Assistant, "回答B"),
            (q2, Some(a1), Role::User, "追问"),
            (a3, Some(q2), Role::Assistant, "回答A2"),
        ] {
            tree.insert(Message {
                id,
                conversation_id: conv,
                parent_id: parent,
                role,
                content: content.into(),
                attachments: Vec::new(),
                created_at: now,
            })
            .unwrap();
        }
        let base = || {
            let request_id = GenerationId::new();
            Generation {
                request_id,
                batch_id: BatchId::new(),
                user_message_id: q,
                assistant_message_id: MessageId::new(),
                provider_id: ProviderId::new(),
                model_id: String::new(),
                status: GenerationStatus::Completed,
                params: None,
                system_prompt: None,
                context_snapshot: vec![],
                price_snapshot: None,
                remote_generation_id: None,
                error: None,
                retry_of: None,
                retry_root_id: Some(request_id),
                failure: None,
                request_snapshot: None,
                retry_not_before_ms: None,
                dispatch_phase: crate::domain::DispatchPhase::NotDispatched,
                started_at: Some(now),
                finished_at: Some(now),
            }
        };
        let gens = vec![
            Generation {
                assistant_message_id: a1,
                model_id: "model/a".into(),
                ..base()
            },
            Generation {
                assistant_message_id: a2,
                model_id: "model/b".into(),
                status: GenerationStatus::Failed,
                error: Some("429".into()),
                ..base()
            },
        ];
        (tree, gens)
    }

    #[test]
    fn main_branch_only() {
        let (tree, gens) = tree_meta();
        // 主分支叶子 = 包含"追问"的链末端(回答A2)
        let a3 = *tree
            .leaves()
            .iter()
            .find(|id| {
                tree.path_from_root(**id)
                    .unwrap()
                    .iter()
                    .any(|p| tree.get(*p).map(|m| m.content == "追问").unwrap_or(false))
            })
            .unwrap();
        let input = ExportInput {
            title: "测试",
            tree: &tree,
            generations: &gens,
            main_leaf: Some(a3),
        };
        let md = render_markdown(&input, ExportScope::Main).unwrap();
        assert!(md.contains("回答A2"));
        assert!(!md.contains("回答B"), "非主支不出现:{}", md);
        assert!(md.contains("$x^2$"), "LaTeX 原文保留");
        assert!(md.contains("model/a"), "模型名");
    }

    #[test]
    fn user_attachments_exported_and_history_composed() {
        let mut tree = MessageTree::new();
        let conv = ConversationId::new();
        let now = chrono::Utc::now();
        let q = MessageId::new();
        let a1 = MessageId::new();
        for (id, parent, role, content, atts) in [
            (
                q,
                None,
                Role::User,
                "问题",
                vec![crate::domain::Attachment {
                    name: "f.txt".into(),
                    content: "FTXT".into(),
                }],
            ),
            (a1, Some(q), Role::Assistant, "回答", vec![]),
        ] {
            tree.insert(Message {
                id,
                conversation_id: conv,
                parent_id: parent,
                role,
                content: content.into(),
                attachments: atts,
                created_at: now,
            })
            .unwrap();
        }
        let input = ExportInput {
            title: "t",
            tree: &tree,
            generations: &[],
            main_leaf: Some(a1),
        };
        let md = render_markdown(&input, ExportScope::Main).unwrap();
        assert!(md.contains("[附件 1: f.txt]"), "{md}");
        assert!(md.contains("FTXT"));
        // history_of 与 Planner 一致:user 消息组合附件
        let hist = history_of(&tree, Some(a1));
        assert!(hist[0].content.contains("===== 附件 1: f.txt ====="));
        assert!(hist[0].content.contains("FTXT"));
    }

    #[test]
    fn all_branches_includes_failed() {
        let (tree, gens) = tree_meta();
        let input = ExportInput {
            title: "测试",
            tree: &tree,
            generations: &gens,
            main_leaf: None,
        };
        let md = render_markdown(&input, ExportScope::AllBranches).unwrap();
        assert!(md.contains("回答B"));
        assert!(md.contains("失败"), "失败叶子标记:{}", md);
        assert!(md.contains("分支"));
    }

    #[test]
    fn atomic_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.md");
        write_markdown(&path, "内容").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "内容");
        // 覆盖写
        write_markdown(&path, "新内容").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "新内容");
        assert!(!dir.path().join("out.md.tmp").exists(), "临时文件已替换");
    }

    #[test]
    fn filename_sanitized() {
        // a/b:c*? -> a_b_c__(两个非法尾字符各换一个下划线)
        assert!(
            default_filename("a/b:c*?", ExportScope::Main).starts_with("a_b_c__-main-"),
            "{}",
            default_filename("a/b:c*?", ExportScope::Main)
        );
    }
}
