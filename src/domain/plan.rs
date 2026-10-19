//! 发送计划(PLAN 3.2 / 4.1)。
//!
//! SendPlan 在发送时冻结:模型、参数、系统提示词、消息顺序、价格与
//! 主请求不受后续 UI 修改影响;原样重试沿用原请求内容。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::attachment::{compose_prompt, Attachment};
use super::ids::{BatchId, ConversationId, GenerationId, MessageId, ProviderId};
use super::money::Pricing;
use super::recovery::{DispatchPhase, FailureInfo};

/// 一次生成的完整快照(Generation 实体)。
#[derive(Debug, Clone)]
pub struct Generation {
    /// 本地请求 ID(唯一,重试产生新 ID 并经 retry_of 关联)。
    pub request_id: GenerationId,
    pub batch_id: BatchId,
    pub user_message_id: MessageId,
    pub assistant_message_id: MessageId,
    pub provider_id: ProviderId,
    pub model_id: String,
    pub status: super::GenerationStatus,
    /// 发送参数快照(Auto 模式下为实际合成结果)。
    pub params: Option<serde_json::Value>,
    pub system_prompt: Option<String>,
    /// 上下文消息 ID 顺序快照(审计用)。
    pub context_snapshot: Vec<MessageId>,
    /// 价格快照(发送时点的目录价格,预估依据)。
    pub price_snapshot: Option<Pricing>,
    /// OpenRouter generation ID(补查费用用)。
    pub remote_generation_id: Option<String>,
    /// 错误简述(旧字段;保留兼容)。结构化详情在 failure。
    pub error: Option<String>,
    pub retry_of: Option<GenerationId>,
    /// lineage 根(原样重试防重粒度;原请求 root 为自己)。
    pub retry_root_id: Option<GenerationId>,
    /// 结构化失败信息(版本化 JSON;读取失败安全降级)。
    pub failure: Option<FailureInfo>,
    /// 完整可重放请求快照(身份字段与内容字段分离;不含凭据)。
    pub request_snapshot: Option<RequestSnapshot>,
    /// 冷却截止(epoch 毫秒;服务端与 UI 以最新持久数据计算资格)。
    pub retry_not_before_ms: Option<i64>,
    /// 请求阶段(NotDispatched 才能确定未调用生成接口)。
    pub dispatch_phase: DispatchPhase,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// 完整可重放请求快照:原样重试默认沿用此内容;
/// 不含 Key、Authorization、网络凭据。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestSnapshot {
    pub provider_id: String,
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// 有序上下文(不含本轮 user/system)。
    pub history: Vec<ContextMessage>,
    /// 本轮 user 最终文本(附件已拼接)。
    pub prompt: String,
    /// 上下文消息 ID 顺序(审计)。
    pub context_ids: Vec<String>,
}

/// 续聊选择:发送前用户确定的分支与目标模型集合。
#[derive(Debug, Clone)]
pub enum ContinuationSelection {
    /// 单支:当前所选分支 + 一个目标模型。
    Single {
        leaf: Option<MessageId>,
        provider_id: ProviderId,
        model_id: String,
    },
    /// 同题 N 模型:一个分支,同一问题分别发给多个模型(共享一个 user 节点)。
    SameQuestionMulti {
        leaf: Option<MessageId>,
        targets: Vec<ModelTarget>,
        primary: usize,
    },
    /// 多支追问:每个选中分支各带一个目标模型(禁止隐式笛卡尔积)。
    MultiBranch {
        branches: Vec<(MessageId, ModelTarget)>,
        primary: usize,
    },
}

/// 目标模型与参数(Auto/Override 已合成)。
#[derive(Debug, Clone)]
pub struct ModelTarget {
    pub provider_id: ProviderId,
    pub model_id: String,
    /// None = Auto(服务端默认,不伪造可选参数)。
    pub params: Option<serde_json::Value>,
    pub price: Option<Pricing>,
}

/// 冻结的发送计划。
#[derive(Debug, Clone)]
pub struct SendPlan {
    pub conversation_id: ConversationId,
    /// 本轮 user 原始输入(不含附件文本)。
    pub prompt: String,
    /// 本轮纯文本附件(发送时由 [`compose_prompt`] 拼接;P7B)。
    pub attachments: Vec<Attachment>,
    pub system_prompt: Option<String>,
    /// 每个分支:父叶子 + 该分支下的请求。共享 user 节点时 len==1。
    pub branches: Vec<PlannedBranch>,
    /// 主请求定位(branch_idx, request_idx);发送前确定,不受返回顺序影响。
    pub primary: (usize, usize),
    pub created_at: DateTime<Utc>,
}

/// 计划分支:在此父叶子上创建 user 节点并发出 N 个请求。
#[derive(Debug, Clone)]
pub struct PlannedBranch {
    /// 父叶子;会话为空(根会话)时为 None。
    pub parent_leaf: Option<MessageId>,
    /// 该分支的 user 节点(多模型共享;规划期分配 ID)。
    pub user_message_id: MessageId,
    pub requests: Vec<PlannedRequest>,
}

/// 单个请求。
#[derive(Debug, Clone)]
pub struct PlannedRequest {
    pub request_id: GenerationId,
    pub provider_id: ProviderId,
    pub model_id: String,
    pub params: Option<serde_json::Value>,
    pub price_snapshot: Option<Pricing>,
    /// 已按分支历史构建的有序上下文(不含本轮 user/system;
    /// 空失败占位已过滤)。
    pub history: Vec<ContextMessage>,
    /// 分支祖先链消息 ID(审计快照;含被过滤的占位)。
    pub context_ids: Vec<MessageId>,
}

/// 发往提供方的消息。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: String,
    pub content: String,
}

impl ContextMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".into(),
            content: content.into(),
        }
    }
}

/// 计划器:把续聊选择 + 提示词 + 上下文构建为冻结的 SendPlan。
pub struct Planner;

impl Planner {
    /// 分支上下文:根→叶子有序历史 + 完整祖先链 ID(审计)。
    /// user 消息按 P7B 把附件拼接进 content(追问轮上下文可见附件)。
    /// 状态感知:assistant 仅在有实际内容时进入上下文——空失败/取消/
    /// 中断占位与旧空 Completed 都被过滤(B3;错误说明绝不进模型上下文);
    /// 部分输出保留(B4)。展示树与导出不受此过滤影响(5.5(3))。
    fn branch_context(
        tree: &super::MessageTree,
        leaf: Option<MessageId>,
    ) -> Result<(Vec<ContextMessage>, Vec<MessageId>), super::DomainError> {
        let Some(leaf) = leaf else {
            return Ok((Vec::new(), Vec::new()));
        };
        let path = tree.path_from_root(leaf)?;
        let history = path
            .iter()
            .filter_map(|id| {
                let m = tree.get(*id)?;
                if m.role == super::Role::Assistant && m.content.trim().is_empty() {
                    return None; // 空占位:不把"未回答"伪装成已回答
                }
                let content = if m.role == super::Role::User {
                    compose_prompt(&m.content, &m.attachments)
                } else {
                    m.content.clone()
                };
                Some(ContextMessage {
                    role: m.role.as_str().to_string(),
                    content,
                })
            })
            .collect();
        Ok((history, path))
    }

    /// 生成发送计划。`history_provider` 负责给出分支历史(通常为
    /// ContextBuilder + 存储读取的组合,测试中可直接传树)。
    /// `prompt` 与 `attachments` 均为空才视为无效。
    pub fn prepare(
        conversation_id: ConversationId,
        selection: &ContinuationSelection,
        prompt: &str,
        attachments: &[Attachment],
        system_prompt: Option<String>,
        tree: &super::MessageTree,
        now: DateTime<Utc>,
    ) -> Result<SendPlan, super::DomainError> {
        if prompt.trim().is_empty() && attachments.is_empty() {
            return Err(super::DomainError::Invalid("提示词为空".into()));
        }

        let mut branches: Vec<PlannedBranch> = Vec::new();
        let mut primary = (0usize, 0usize);

        match selection {
            ContinuationSelection::Single {
                leaf,
                provider_id,
                model_id,
            } => {
                let (history, context_ids) = Self::branch_context(tree, *leaf)?;
                branches.push(PlannedBranch {
                    parent_leaf: *leaf,
                    user_message_id: MessageId::new(),
                    requests: vec![PlannedRequest {
                        request_id: GenerationId::new(),
                        provider_id: *provider_id,
                        model_id: model_id.clone(),
                        params: None,
                        price_snapshot: None,
                        history,
                        context_ids,
                    }],
                });
            }
            ContinuationSelection::SameQuestionMulti {
                leaf,
                targets,
                primary: p,
            } => {
                if targets.is_empty() {
                    return Err(super::DomainError::Invalid("目标模型为空".into()));
                }
                if *p >= targets.len() {
                    return Err(super::DomainError::Invalid("主模型索引越界".into()));
                }
                // 所有请求使用同一历史(同一 user 节点)
                let (history, context_ids) = Self::branch_context(tree, *leaf)?;
                let user_message_id = MessageId::new();
                let requests = targets
                    .iter()
                    .map(|t| PlannedRequest {
                        request_id: GenerationId::new(),
                        provider_id: t.provider_id,
                        model_id: t.model_id.clone(),
                        params: t.params.clone(),
                        price_snapshot: t.price,
                        history: history.clone(),
                        context_ids: context_ids.clone(),
                    })
                    .collect();
                branches.push(PlannedBranch {
                    parent_leaf: *leaf,
                    user_message_id,
                    requests,
                });
                primary = (0, *p);
            }
            ContinuationSelection::MultiBranch {
                branches: sel,
                primary: p,
            } => {
                if sel.is_empty() {
                    return Err(super::DomainError::Invalid("未选择任何分支".into()));
                }
                if *p >= sel.len() {
                    return Err(super::DomainError::Invalid("主分支索引越界".into()));
                }
                // 每支一个内容相同但 ID 不同的 user 节点;禁止分支×模型笛卡尔积
                let mut seen = std::collections::HashSet::new();
                for (leaf, _) in sel {
                    if !seen.insert(*leaf) {
                        return Err(super::DomainError::Invalid(
                            "同一分支出现多次(禁止隐式笛卡尔积)".into(),
                        ));
                    }
                }
                for (leaf, target) in sel {
                    let (history, context_ids) = Self::branch_context(tree, Some(*leaf))?;
                    branches.push(PlannedBranch {
                        parent_leaf: Some(*leaf),
                        user_message_id: MessageId::new(),
                        requests: vec![PlannedRequest {
                            request_id: GenerationId::new(),
                            provider_id: target.provider_id,
                            model_id: target.model_id.clone(),
                            params: target.params.clone(),
                            price_snapshot: target.price,
                            history,
                            context_ids,
                        }],
                    });
                }
                primary = (*p, 0);
            }
        }

        Ok(SendPlan {
            conversation_id,
            prompt: prompt.to_string(),
            attachments: attachments.to_vec(),
            system_prompt,
            branches,
            primary,
            created_at: now,
        })
    }

    /// 重试兜底上下文:该 user 节点之前的祖先链(过滤空 assistant)。
    /// 服务层默认使用持久化快照;此函数仅用于旧数据无快照时的
    /// "按现有历史重新生成"(不用导出工具构建 retry 上下文,5)。
    pub fn history_before(
        tree: &super::MessageTree,
        user_message_id: MessageId,
    ) -> Vec<ContextMessage> {
        let parent = tree.get(user_message_id).and_then(|m| m.parent_id);
        Self::branch_context(tree, parent)
            .map(|(h, _)| h)
            .unwrap_or_default()
    }

    /// 原样重试:沿用原请求内容(模型/参数/上下文不变),新 request_id。
    pub fn retry(plan: &SendPlan, branch_idx: usize, request_idx: usize) -> PlannedRequest {
        let req = &plan.branches[branch_idx].requests[request_idx];
        PlannedRequest {
            request_id: GenerationId::new(),
            provider_id: req.provider_id,
            model_id: req.model_id.clone(),
            params: req.params.clone(),
            price_snapshot: req.price_snapshot,
            history: req.history.clone(),
            context_ids: req.context_ids.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{DomainError, Message, Role};

    fn tree_qab() -> (super::super::MessageTree, MessageId, MessageId, MessageId) {
        let mut tree = super::super::MessageTree::new();
        let conv = ConversationId::new();
        let q = MessageId::new();
        let a1 = MessageId::new();
        let b1 = MessageId::new();
        let now = Utc::now();
        for (id, parent, role, content) in [
            (q, None, Role::User, "Q"),
            (a1, Some(q), Role::Assistant, "A1"),
            (b1, Some(q), Role::Assistant, "B1"),
        ] {
            tree.insert(Message {
                id,
                conversation_id: conv,
                parent_id: parent,
                role,
                content: content.to_string(),
                attachments: Vec::new(),
                created_at: now,
            })
            .unwrap();
        }
        (tree, q, a1, b1)
    }

    fn target(model: &str) -> ModelTarget {
        ModelTarget {
            provider_id: ProviderId::new(),
            model_id: model.to_string(),
            params: None,
            price: None,
        }
    }

    #[test]
    fn single_branch_history() {
        let (tree, q, a1, _) = tree_qab();
        let plan = Planner::prepare(
            ConversationId::new(),
            &ContinuationSelection::Single {
                leaf: Some(a1),
                provider_id: ProviderId::new(),
                model_id: "m".into(),
            },
            "Q2",
            &[],
            None,
            &tree,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(plan.branches.len(), 1);
        assert_eq!(plan.branches[0].parent_leaf, Some(a1));
        let hist = &plan.branches[0].requests[0].history;
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[0].content, "Q");
        assert_eq!(hist[1].content, "A1");
        let _ = q;
    }

    #[test]
    fn multi_branch_isolation_b2() {
        // B2 只含 B1,不含 A1;两支各一个请求(无隐式四请求)
        let (tree, _, a1, b1) = tree_qab();
        let plan = Planner::prepare(
            ConversationId::new(),
            &ContinuationSelection::MultiBranch {
                branches: vec![(a1, target("A")), (b1, target("C"))],
                primary: 0,
            },
            "Q2",
            &[],
            None,
            &tree,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(plan.branches.len(), 2);
        assert_eq!(plan.branches[0].requests.len(), 1);
        assert_eq!(plan.branches[1].requests.len(), 1);
        let b2_hist = &plan.branches[1].requests[0].history;
        assert_eq!(b2_hist.len(), 2);
        assert_eq!(b2_hist[1].content, "B1");
        assert!(b2_hist.iter().all(|m| m.content != "A1"));
        // user 节点 ID 不同
        assert_ne!(
            plan.branches[0].user_message_id,
            plan.branches[1].user_message_id
        );
    }

    #[test]
    fn same_question_multi_shares_history_and_user_node() {
        let (tree, _, a1, _) = tree_qab();
        let plan = Planner::prepare(
            ConversationId::new(),
            &ContinuationSelection::SameQuestionMulti {
                leaf: Some(a1),
                targets: vec![target("A"), target("B"), target("C")],
                primary: 2,
            },
            "Q2",
            &[],
            None,
            &tree,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(plan.branches.len(), 1);
        assert_eq!(plan.branches[0].requests.len(), 3);
        assert_eq!(plan.primary, (0, 2));
        let h0 = &plan.branches[0].requests[0].history;
        let h2 = &plan.branches[0].requests[2].history;
        assert_eq!(h0, h2);
    }

    #[test]
    fn duplicate_branch_rejected() {
        let (tree, _, a1, _) = tree_qab();
        let err = Planner::prepare(
            ConversationId::new(),
            &ContinuationSelection::MultiBranch {
                branches: vec![(a1, target("A")), (a1, target("B"))],
                primary: 0,
            },
            "Q2",
            &[],
            None,
            &tree,
            Utc::now(),
        );
        assert!(matches!(err, Err(DomainError::Invalid(_))));
    }

    #[test]
    fn retry_reuses_content_with_new_id() {
        let (tree, _, a1, _) = tree_qab();
        let plan = Planner::prepare(
            ConversationId::new(),
            &ContinuationSelection::Single {
                leaf: Some(a1),
                provider_id: ProviderId::new(),
                model_id: "m".into(),
            },
            "Q2",
            &[],
            None,
            &tree,
            Utc::now(),
        )
        .unwrap();
        let retried = Planner::retry(&plan, 0, 0);
        assert_ne!(retried.request_id, plan.branches[0].requests[0].request_id);
        assert_eq!(retried.model_id, "m");
        assert_eq!(retried.history.len(), 2);
    }

    #[test]
    fn empty_prompt_rejected() {
        let (tree, _, a1, _) = tree_qab();
        let err = Planner::prepare(
            ConversationId::new(),
            &ContinuationSelection::Single {
                leaf: Some(a1),
                provider_id: ProviderId::new(),
                model_id: "m".into(),
            },
            "   ",
            &[],
            None,
            &tree,
            Utc::now(),
        );
        assert!(matches!(err, Err(DomainError::Invalid(_))));
    }

    #[test]
    fn attachments_without_prompt_accepted() {
        let (tree, _, a1, _) = tree_qab();
        let plan = Planner::prepare(
            ConversationId::new(),
            &ContinuationSelection::Single {
                leaf: Some(a1),
                provider_id: ProviderId::new(),
                model_id: "m".into(),
            },
            "",
            &[crate::domain::Attachment {
                name: "n.txt".into(),
                content: "c".into(),
            }],
            None,
            &tree,
            Utc::now(),
        )
        .unwrap();
        assert_eq!(plan.branches.len(), 1);
        assert_eq!(plan.attachments.len(), 1);
    }

    #[test]
    fn empty_assistant_placeholder_filtered_from_history() {
        // B3:U1 + 空失败 A1,随后 U2 —— 上下文不含空 assistant;
        // B4:有部分内容的失败 assistant 保留
        let mut tree = super::super::MessageTree::new();
        let conv = ConversationId::new();
        let u1 = MessageId::new();
        let a1 = MessageId::new();
        let now = Utc::now();
        for (id, parent, role, content) in [
            (u1, None, Role::User, "U1"),
            (a1, Some(u1), Role::Assistant, ""),
        ] {
            tree.insert(Message {
                id,
                conversation_id: conv,
                parent_id: parent,
                role,
                content: content.to_string(),
                attachments: Vec::new(),
                created_at: now,
            })
            .unwrap();
        }
        let plan = Planner::prepare(
            conv,
            &ContinuationSelection::Single {
                leaf: Some(a1),
                provider_id: ProviderId::new(),
                model_id: "m".into(),
            },
            "U2",
            &[],
            None,
            &tree,
            Utc::now(),
        )
        .unwrap();
        let hist = &plan.branches[0].requests[0].history;
        assert_eq!(hist.len(), 1, "空失败占位不进上下文");
        assert_eq!(hist[0].content, "U1");

        // 部分内容保留
        let mut m = tree.messages.get_mut(&a1).unwrap().clone();
        m.content = "部分回答".into();
        tree.messages.insert(a1, m);
        let plan = Planner::prepare(
            conv,
            &ContinuationSelection::Single {
                leaf: Some(a1),
                provider_id: ProviderId::new(),
                model_id: "m".into(),
            },
            "U2",
            &[],
            None,
            &tree,
            Utc::now(),
        )
        .unwrap();
        let hist = &plan.branches[0].requests[0].history;
        assert_eq!(hist.len(), 2);
        assert_eq!(hist[1].content, "部分回答");
    }

    #[test]
    fn history_composes_user_attachments() {
        let (mut tree, _, a1, _) = tree_qab();
        // 给根 user 节点(Q)挂附件
        let root = tree.path_from_root(a1).unwrap()[0];
        let mut m = tree.messages.get(&root).unwrap().clone();
        m.attachments = vec![crate::domain::Attachment {
            name: "a.txt".into(),
            content: "附件正文".into(),
        }];
        tree.messages.insert(root, m);

        let plan = Planner::prepare(
            ConversationId::new(),
            &ContinuationSelection::Single {
                leaf: Some(a1),
                provider_id: ProviderId::new(),
                model_id: "m".into(),
            },
            "追问",
            &[],
            None,
            &tree,
            Utc::now(),
        )
        .unwrap();
        let hist = &plan.branches[0].requests[0].history;
        assert!(
            hist[0].content.contains("===== 附件 1: a.txt ====="),
            "{}",
            hist[0].content
        );
        assert!(hist[0].content.contains("附件正文"));
        // 本轮附件独立于历史附件
        assert!(plan.attachments.is_empty());
    }
}
