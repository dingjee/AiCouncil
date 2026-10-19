//! 自动标题服务(PLAN 7.2):独立元数据服务,不伪造消息/generation。
//!
//! 触发规则:
//! - 首次用户 send **成功落库**后设 fallback,并异步申请标题;
//! - 仅 source ∈ {empty, fallback} 且 needs_generation 时尝试;
//! - 一次 send 是一个 turn(并行/多支不产生 N 次请求);assistant
//!   重试不算新提问;失败保留 fallback,下次提问再试(本轮内不
//!   自动连续付费重试);
//! - 同会话至多一条在途;跨会话低并发队列(信号量 2);
//! - 成功后固定;手动改名始终优先(DB CAS 兜底);删除会话由
//!   外键级联清理台账,迟到结果被 CAS 丢弃。

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex, Semaphore};

use crate::domain::{Conversation, ConversationId, TitleStatus};
use crate::providers::deepseek::{fallback_title, DeepSeekTitleClient, TitleErrorKind};
use crate::storage::Db;

/// 标题服务事件(UI 按 ConversationId 路由,不按当前窗口;G0 合同)。
#[derive(Debug, Clone)]
pub struct TitleEvent {
    pub conversation_id: ConversationId,
    pub request_token: String,
    pub outcome: Result<(), TitleErrorKind>,
}

pub struct TitleService {
    db: Arc<Db>,
    client: Option<Arc<DeepSeekTitleClient>>,
    events: broadcast::Sender<TitleEvent>,
    /// 同会话在途集合(进程内快查;权威资格在 DB 事务内)。
    in_flight: Mutex<HashSet<ConversationId>>,
    /// 跨会话低并发(不占满聊天信号量)。
    slots: Arc<Semaphore>,
}

impl TitleService {
    /// 构造;事件通道由调用方持有(App 经 Subscription 订阅)。
    pub fn new(
        db: Arc<Db>,
        client: Option<DeepSeekTitleClient>,
        events: broadcast::Sender<TitleEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            client: client.map(Arc::new),
            events,
            in_flight: Mutex::new(HashSet::new()),
            slots: Arc::new(Semaphore::new(2)),
        })
    }

    pub fn has_credentials(&self) -> bool {
        self.client.is_some()
    }

    /// 首问成功落库:立即设 fallback(来源允许时),并异步申请标题。
    /// 不等待、不阻塞发送;仅乐观 UI 未落库时调用方不得调用。
    pub fn on_user_turn_persisted(
        self: &Arc<Self>,
        conversation: ConversationId,
        turn_id: String,
        prompt: &str,
        attachments: &[crate::domain::Attachment],
    ) {
        // 1) fallback 标题(来源允许时;手动/生成成功不受影响)
        if let Some(title) = fallback_title(prompt, attachments) {
            let _ = self.db.set_fallback_title(conversation, &title);
        }
        // 2) 异步申请(无凭据保持 needs_generation,下次提问仍会尝试)
        let Some(client) = self.client.clone() else {
            return;
        };
        let service = self.clone();
        let prompt = prompt.to_string();
        tokio::spawn(async move {
            service
                .run_request(conversation, turn_id, prompt, client)
                .await;
        });
    }

    /// 单次请求流程:资格登记(DB 事务)→ 锁外 HTTP → CAS 落库/失败。
    async fn run_request(
        self: Arc<Self>,
        conversation: ConversationId,
        turn_id: String,
        prompt: String,
        client: Arc<DeepSeekTitleClient>,
    ) {
        let db = &self.db;
        let conv: Conversation = match db
            .conversations()
            .ok()
            .and_then(|cs| cs.into_iter().find(|c| c.id == conversation))
        {
            Some(c) => c,
            None => return,
        };
        if !conv.title_source.allows_auto() || conv.title_status != TitleStatus::NeedsGeneration {
            return;
        }
        let token = format!("title-{}", uuid::Uuid::new_v4());
        let revision = conv.title_revision;
        let ok = db
            .begin_title_attempt(
                conversation,
                Some(&turn_id),
                &token,
                client.model_name(),
                revision,
            )
            .unwrap_or(false);
        if !ok {
            return; // 已有在途/重复 turn/版本过期
        }
        // 进程内在途去重
        {
            let mut guard = self.in_flight.lock().await;
            if !guard.insert(conversation) {
                let _ = db.fail_title_attempt(conversation, &token, "duplicate");
                return;
            }
        }
        // 低并发槽(SQLite 锁外等待)
        let _permit = self.slots.acquire().await;
        let result = client.generate_title(&prompt).await;
        match result {
            Ok(title) => {
                let applied = db
                    .apply_generated_title(conversation, revision, &title)
                    .map(|o| o.is_some())
                    .unwrap_or(false);
                if applied {
                    let _ = db.complete_aux_request(&token, None, None, None);
                    let _ = self.events.send(TitleEvent {
                        conversation_id: conversation,
                        request_token: token.clone(),
                        outcome: Ok(()),
                    });
                } else {
                    // 迟到/被手动抢先:CAS 丢弃(7.4)
                    let _ = db.fail_title_attempt(conversation, &token, "superseded");
                }
            }
            Err(kind) => {
                let _ = db.fail_title_attempt(conversation, &token, kind.as_str());
                let _ = self.events.send(TitleEvent {
                    conversation_id: conversation,
                    request_token: token.clone(),
                    outcome: Err(kind),
                });
            }
        }
        self.in_flight.lock().await.remove(&conversation);
    }
}
