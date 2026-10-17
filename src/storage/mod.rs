//! SQLite 存储层:单一写入队列(Mutex 连接)+ 迁移 + 仓储。
//!
//! 约束(PLAN 2):
//! - UI 不直接拼 SQL;所有访问经 [`Db`] 方法。
//! - 一次发送的相关记录(user/assistant 占位、批次、Generation、用量)
//!   在**单个事务**中落库;保存成功才发请求。
//! - 异步上下文通过 `tokio::task::spawn_blocking` 调用(服务层负责)。

pub mod credentials;
pub mod repo;

use std::path::Path;
use std::sync::Mutex;

use crate::domain::{
    BatchId, Conversation, ConversationId, Generation, GenerationId, Group, GroupId, Message,
    MessageId, UsageRecord,
};

/// 数据库错误。
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("SQLite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("序列化: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("迁移失败: {0}")]
    Migration(String),
    #[error("未找到: {0}")]
    NotFound(String),
    #[error("IO: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, StorageError>;

const MIGRATIONS: &[(&str, &str)] =
    &[("0001_init", include_str!("../../migrations/0001_init.sql"))];

/// 打开(必要时创建)数据库并执行迁移。
pub fn open(path: &Path) -> Result<Db> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = rusqlite::Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(&conn)?;
    Ok(Db {
        conn: Mutex::new(conn),
    })
}

/// 内存数据库(测试用)。
pub fn open_in_memory() -> Result<Db> {
    let conn = rusqlite::Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrate(&conn)?;
    Ok(Db {
        conn: Mutex::new(conn),
    })
}

fn migrate(conn: &rusqlite::Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            name TEXT PRIMARY KEY,
            applied_at INTEGER NOT NULL
        );",
    )?;
    for (name, sql) in MIGRATIONS {
        let applied: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE name = ?1",
                [name],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)?;
        if !applied {
            conn.execute_batch(sql)?;
            conn.execute(
                "INSERT INTO schema_migrations(name, applied_at) VALUES (?1, ?2)",
                rusqlite::params![name, now_ms()],
            )?;
        }
    }
    Ok(())
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn ms_to_dt(ms: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

/// 线程安全的数据库句柄(单写多读;WAL 下读不阻塞)。
pub struct Db {
    conn: Mutex<rusqlite::Connection>,
}

/// 一次发送的全部持久化内容(单事务写入)。
pub struct SendBatchRecord {
    pub batch_id: BatchId,
    pub conversation_id: ConversationId,
    pub primary_request_id: GenerationId,
    /// 该批的 user 消息(多支时每支一条,内容相同 ID 不同)。
    pub user_messages: Vec<Message>,
    /// assistant 占位(每个请求一条)。
    pub assistant_messages: Vec<Message>,
    pub generations: Vec<Generation>,
    /// 发送后应立即写为 pending 的用量记录(每请求一条)。
    pub usage: Vec<UsageRecord>,
}

impl Db {
    pub fn lock(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ----- 分组 -----

    pub fn groups(&self) -> Result<Vec<Group>> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT id, name, sort_order FROM groups ORDER BY sort_order, name")?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Group {
                    id: GroupId::from_uuid_str(r.get::<_, String>(0)?.as_str()),
                    name: r.get(1)?,
                    sort_order: r.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn upsert_group(&self, g: &Group) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO groups(id, name, sort_order) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET name = ?2, sort_order = ?3",
            rusqlite::params![g.id.as_str(), g.name, g.sort_order],
        )?;
        Ok(())
    }

    /// 删除分组:会话移回未分组(ON DELETE SET NULL),消息/费用不变。
    pub fn delete_group(&self, id: GroupId) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM groups WHERE id = ?1", [id.as_str()])?;
        Ok(())
    }

    // ----- 会话 -----

    pub fn conversations(&self) -> Result<Vec<Conversation>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, title, group_id, system_prompt, main_leaf_id, draft, created_at, updated_at
             FROM conversations ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Conversation {
                    id: ConversationId::from_uuid_str(r.get::<_, String>(0)?.as_str()),
                    title: r.get(1)?,
                    group_id: r
                        .get::<_, Option<String>>(2)?
                        .map(|s| GroupId::from_uuid_str(s.as_str())),
                    system_prompt: r.get(3)?,
                    main_leaf_id: r
                        .get::<_, Option<String>>(4)?
                        .map(|s| MessageId::from_uuid_str(s.as_str())),
                    draft: r.get(5)?,
                    created_at: ms_to_dt(r.get(6)?),
                    updated_at: ms_to_dt(r.get(7)?),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn upsert_conversation(&self, c: &Conversation) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO conversations
                (id, title, group_id, system_prompt, main_leaf_id, draft, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                title = ?2, group_id = ?3, system_prompt = ?4,
                main_leaf_id = ?5, draft = ?6, updated_at = ?8",
            rusqlite::params![
                c.id.as_str(),
                c.title,
                c.group_id.map(|g| g.as_str()),
                c.system_prompt,
                c.main_leaf_id.map(|m| m.as_str()),
                c.draft,
                c.created_at.timestamp_millis(),
                chrono::Utc::now().timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, id: ConversationId) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM conversations WHERE id = ?1", [id.as_str()])?;
        Ok(())
    }

    // ----- 消息 -----

    pub fn insert_message(&self, m: &Message) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO messages(id, conversation_id, parent_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                m.id.as_str(),
                m.conversation_id.as_str(),
                m.parent_id.map(|p| p.as_str()),
                m.role.as_str(),
                m.content,
                m.created_at.timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    /// 会话全部消息(按创建时间);树结构由内存构建。
    pub fn messages_of(&self, conversation: ConversationId) -> Result<Vec<Message>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, parent_id, role, content, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = stmt
            .query_map([conversation.as_str()], |r| {
                Ok(Message {
                    id: MessageId::from_uuid_str(r.get::<_, String>(0)?.as_str()),
                    conversation_id: ConversationId::from_uuid_str(r.get::<_, String>(1)?.as_str()),
                    parent_id: r
                        .get::<_, Option<String>>(2)?
                        .map(|s| MessageId::from_uuid_str(s.as_str())),
                    role: crate::domain::Role::from_str(&r.get::<_, String>(3)?)
                        .unwrap_or(crate::domain::Role::User),
                    content: r.get(4)?,
                    created_at: ms_to_dt(r.get(5)?),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 流式增量:整体替换 assistant 内容(幂等,服务层负责节流)。
    pub fn update_message_content(&self, id: MessageId, content: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE messages SET content = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), content],
        )?;
        Ok(())
    }

    // ----- 批次与生成(单事务) -----

    /// 事务写入一次发送的全部记录;任何一步失败整体回滚。
    pub fn save_send_batch(&self, rec: &SendBatchRecord) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO send_batches(id, conversation_id, primary_request_id, sent_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                rec.batch_id.as_str(),
                rec.conversation_id.as_str(),
                rec.primary_request_id.as_str(),
                now_ms()
            ],
        )?;
        for m in rec.user_messages.iter().chain(&rec.assistant_messages) {
            tx.execute(
                "INSERT INTO messages(id, conversation_id, parent_id, role, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    m.id.as_str(),
                    m.conversation_id.as_str(),
                    m.parent_id.map(|p| p.as_str()),
                    m.role.as_str(),
                    m.content,
                    m.created_at.timestamp_millis(),
                ],
            )?;
        }
        for g in &rec.generations {
            tx.execute(
                "INSERT INTO generations
                    (request_id, batch_id, user_message_id, assistant_message_id,
                     provider_id, model_id, status, params_json, system_prompt,
                     context_json, price_json, remote_generation_id, error, retry_of,
                     started_at, finished_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
                rusqlite::params![
                    g.request_id.as_str(),
                    g.batch_id.as_str(),
                    g.user_message_id.as_str(),
                    g.assistant_message_id.as_str(),
                    g.provider_id.as_str(),
                    g.model_id,
                    g.status.as_str(),
                    g.params.as_ref().map(|v| v.to_string()),
                    g.system_prompt,
                    serde_json::to_string(
                        &g.context_snapshot
                            .iter()
                            .map(|i| i.as_str())
                            .collect::<Vec<_>>()
                    )?,
                    g.price_snapshot
                        .map(|p| serde_json::to_string(&p))
                        .transpose()?,
                    g.remote_generation_id,
                    g.error,
                    g.retry_of.map(|r| r.as_str()),
                    g.started_at.map(|t| t.timestamp_millis()),
                    g.finished_at.map(|t| t.timestamp_millis()),
                ],
            )?;
        }
        for u in &rec.usage {
            tx.execute(
                "INSERT INTO usage_records
                    (request_id, prompt_tokens, completion_tokens, total_tokens,
                     cost_text, cost_source, status, remote_generation_id, queried_at)
                 VALUES (?1,NULL,NULL,NULL,NULL,NULL,'pending',?2,NULL)",
                rusqlite::params![u.request_id.as_str(), u.remote_generation_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_generation_status(
        &self,
        request: GenerationId,
        status: crate::domain::GenerationStatus,
        error: Option<&str>,
        remote_generation_id: Option<&str>,
    ) -> Result<()> {
        let conn = self.lock();
        let now = now_ms();
        conn.execute(
            "UPDATE generations SET status = ?2, error = ?3, remote_generation_id = COALESCE(?4, remote_generation_id),
                started_at = CASE WHEN ?2 IN ('connecting','streaming','completed','cancelled','failed') AND started_at IS NULL THEN ?5 ELSE started_at END,
                finished_at = CASE WHEN ?2 IN ('completed','cancelled','failed','interrupted') THEN ?5 ELSE finished_at END
             WHERE request_id = ?1",
            rusqlite::params![request.as_str(), status.as_str(), error, remote_generation_id, now],
        )?;
        Ok(())
    }

    pub fn generations_of(&self, conversation: ConversationId) -> Result<Vec<Generation>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT g.request_id, g.batch_id, g.user_message_id, g.assistant_message_id,
                    g.provider_id, g.model_id, g.status, g.params_json, g.system_prompt,
                    g.context_json, g.price_json, g.remote_generation_id, g.error, g.retry_of,
                    g.started_at, g.finished_at
             FROM generations g
             JOIN send_batches b ON b.id = g.batch_id
             WHERE b.conversation_id = ?1
             ORDER BY b.sent_at, g.request_id",
        )?;
        let rows = stmt
            .query_map([conversation.as_str()], row_to_generation)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 重试关联查询:某 assistant 消息最新的 Generation。
    pub fn generation_for_assistant(&self, assistant: MessageId) -> Result<Option<Generation>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT g.request_id, g.batch_id, g.user_message_id, g.assistant_message_id,
                    g.provider_id, g.model_id, g.status, g.params_json, g.system_prompt,
                    g.context_json, g.price_json, g.remote_generation_id, g.error, g.retry_of,
                    g.started_at, g.finished_at
             FROM generations g WHERE g.assistant_message_id = ?1
             ORDER BY g.started_at DESC LIMIT 1",
        )?;
        let mut rows = stmt
            .query_map([assistant.as_str()], row_to_generation)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.pop())
    }

    /// 启动恢复:遗留 Queued/Connecting/Streaming -> Interrupted。
    /// 返回受影响请求数。
    pub fn recover_interrupted(&self) -> Result<usize> {
        let conn = self.lock();
        let n = conn.execute(
            "UPDATE generations SET status = 'interrupted',
                finished_at = COALESCE(finished_at, ?1),
                error = COALESCE(error, '程序退出时未完成')
             WHERE status IN ('queued','connecting','streaming')",
            [now_ms()],
        )?;
        Ok(n)
    }

    // ----- 用量 -----

    pub fn usage_of(&self, conversation: ConversationId) -> Result<Vec<UsageRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT u.request_id, u.prompt_tokens, u.completion_tokens, u.total_tokens,
                    u.cost_text, u.cost_source, u.status, u.remote_generation_id, u.queried_at
             FROM usage_records u
             JOIN generations g ON g.request_id = u.request_id
             JOIN send_batches b ON b.id = g.batch_id
             WHERE b.conversation_id = ?1
             ORDER BY b.sent_at",
        )?;
        let rows = stmt
            .query_map([conversation.as_str()], row_to_usage)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 幂等入账:UPSERT 单行,已 Known 且新数据无费用时不回退。
    pub fn upsert_usage(&self, rec: &UsageRecord) -> Result<()> {
        /// (request_id, prompt, completion, cost, cost_source, status)
        type ExistingRow = (
            String,
            Option<u64>,
            Option<u64>,
            Option<String>,
            Option<String>,
            String,
        );
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let existing: Option<ExistingRow> = tx
            .query_row(
                "SELECT request_id, prompt_tokens, completion_tokens, cost_text, cost_source, status
                 FROM usage_records WHERE request_id = ?1",
                [rec.request_id.as_str()],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;

        let cost_text = rec.cost.map(|c| c.to_string());
        match existing {
            None => {
                tx.execute(
                    "INSERT INTO usage_records
                        (request_id, prompt_tokens, completion_tokens, total_tokens,
                         cost_text, cost_source, status, remote_generation_id, queried_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    rusqlite::params![
                        rec.request_id.as_str(),
                        rec.prompt_tokens,
                        rec.completion_tokens,
                        rec.total_tokens,
                        cost_text,
                        rec.cost_source.map(|s| s.as_str()),
                        rec.status.as_str(),
                        rec.remote_generation_id,
                        rec.queried_at.map(|t| t.timestamp_millis()),
                    ],
                )?;
            }
            Some((_, _ex_prompt, _ex_completion, ex_cost, _ex_src, ex_status)) => {
                // 不回退:已知费用优先
                let final_cost = rec.cost.map(|c| c.to_string()).or(ex_cost);
                let final_status = if final_cost.is_some() {
                    "known"
                } else if ex_status == "unavailable" {
                    "unavailable"
                } else {
                    rec.status.as_str()
                };
                let final_source = rec.cost_source.map(|s| s.as_str().to_string()).or(_ex_src);
                tx.execute(
                    "UPDATE usage_records SET
                        prompt_tokens = COALESCE(?2, prompt_tokens),
                        completion_tokens = COALESCE(?3, completion_tokens),
                        total_tokens = COALESCE(?4, total_tokens),
                        cost_text = ?5,
                        cost_source = ?6,
                        status = ?7,
                        remote_generation_id = COALESCE(?8, remote_generation_id),
                        queried_at = ?9
                     WHERE request_id = ?1",
                    rusqlite::params![
                        rec.request_id.as_str(),
                        rec.prompt_tokens,
                        rec.completion_tokens,
                        rec.total_tokens,
                        final_cost,
                        final_source,
                        final_status,
                        rec.remote_generation_id,
                        rec.queried_at
                            .map(|t| t.timestamp_millis())
                            .unwrap_or_else(now_ms),
                    ],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 待补查的用量记录(Pending 且有远端 ID,或全部 Pending)。
    pub fn pending_usage(&self) -> Result<Vec<UsageRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT u.request_id, u.prompt_tokens, u.completion_tokens, u.total_tokens,
                    u.cost_text, u.cost_source, u.status, u.remote_generation_id, u.queried_at
             FROM usage_records u WHERE u.status = 'pending'",
        )?;
        let rows = stmt
            .query_map([], row_to_usage)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ----- UI 状态 -----

    pub fn set_ui_state(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO ui_state(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = ?2",
            rusqlite::params![key, value.to_string()],
        )?;
        Ok(())
    }

    pub fn get_ui_state(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let conn = self.lock();
        let v = conn
            .query_row("SELECT value FROM ui_state WHERE key = ?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        match v {
            None => Ok(None),
            Some(s) => Ok(Some(serde_json::from_str(&s)?)),
        }
    }
}

fn row_to_generation(r: &rusqlite::Row<'_>) -> rusqlite::Result<Generation> {
    let ctx: String = r.get(9)?;
    let ctx_ids: Vec<String> = serde_json::from_str(&ctx).unwrap_or_default();
    let price_json: Option<String> = r.get(10)?;
    Ok(Generation {
        request_id: GenerationId::from_uuid_str(r.get::<_, String>(0)?.as_str()),
        batch_id: BatchId::from_uuid_str(r.get::<_, String>(1)?.as_str()),
        user_message_id: MessageId::from_uuid_str(r.get::<_, String>(2)?.as_str()),
        assistant_message_id: MessageId::from_uuid_str(r.get::<_, String>(3)?.as_str()),
        provider_id: crate::domain::ProviderId::from_uuid_str(r.get::<_, String>(4)?.as_str()),
        model_id: r.get(5)?,
        status: crate::domain::GenerationStatus::from_str(&r.get::<_, String>(6)?)
            .unwrap_or(crate::domain::GenerationStatus::Interrupted),
        params: r
            .get::<_, Option<String>>(7)?
            .and_then(|s| serde_json::from_str(&s).ok()),
        system_prompt: r.get(8)?,
        context_snapshot: ctx_ids
            .into_iter()
            .map(|s| MessageId::from_uuid_str(s.as_str()))
            .collect(),
        price_snapshot: price_json.and_then(|s| serde_json::from_str(&s).ok()),
        remote_generation_id: r.get(11)?,
        error: r.get(12)?,
        retry_of: r
            .get::<_, Option<String>>(13)?
            .map(|s| GenerationId::from_uuid_str(s.as_str())),
        started_at: r.get::<_, Option<i64>>(14)?.map(ms_to_dt),
        finished_at: r.get::<_, Option<i64>>(15)?.map(ms_to_dt),
    })
}

fn row_to_usage(r: &rusqlite::Row<'_>) -> rusqlite::Result<UsageRecord> {
    let cost_text: Option<String> = r.get(4)?;
    Ok(UsageRecord {
        request_id: GenerationId::from_uuid_str(r.get::<_, String>(0)?.as_str()),
        prompt_tokens: r.get(1)?,
        completion_tokens: r.get(2)?,
        total_tokens: r.get(3)?,
        cost: cost_text
            .as_deref()
            .and_then(|s| rust_decimal::Decimal::from_str_radix(s, 10).ok()),
        cost_source: r
            .get::<_, Option<String>>(5)?
            .as_deref()
            .and_then(crate::domain::CostSource::from_str),
        status: crate::domain::UsageStatus::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
        remote_generation_id: r.get(7)?,
        queried_at: r.get::<_, Option<i64>>(8)?.map(ms_to_dt),
    })
}
