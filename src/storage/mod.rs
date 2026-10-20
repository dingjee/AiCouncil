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

use rusqlite::OptionalExtension;

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

/// `ui_state` 键:未发送的新会话槽位指针。
pub const PENDING_NEW_KEY: &str = "pending_new_conversation";

/// `ui_state` 的字符串值编码(与 [`Db::set_ui_state`] 的 JSON 形式一致)。
fn json_string(value: &str) -> String {
    serde_json::json!(value).to_string()
}

const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../../migrations/0001_init.sql")),
    (
        "0002_attachments",
        include_str!("../../migrations/0002_attachments.sql"),
    ),
    (
        "0003_title_metadata",
        include_str!("../../migrations/0003_title_metadata.sql"),
    ),
    (
        "0004_request_recovery",
        include_str!("../../migrations/0004_request_recovery.sql"),
    ),
    (
        "0005_message_removal",
        include_str!("../../migrations/0005_message_removal.sql"),
    ),
    (
        "0006_composer_drafts",
        include_str!("../../migrations/0006_composer_drafts.sql"),
    ),
];

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
            // 单迁移单事务:任一语句失败整体回滚,不部分升级(T08)
            if let Err(e) = conn.execute_batch(&format!("BEGIN IMMEDIATE;\n{sql}\nCOMMIT;")) {
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(StorageError::Migration(format!("{name}: {e}")));
            }
            conn.execute(
                "INSERT INTO schema_migrations(name, applied_at) VALUES (?1, ?2)",
                rusqlite::params![name, now_ms()],
            )?;
        }
    }
    Ok(())
}

pub fn now_dt() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
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
    /// 事务内一并落库的 main_leaf 意图(CAS;None 不动 leaf)。
    /// (期望当前值, 新值):期望不匹配则不移动,保护用户后续选择。
    pub intended_main_leaf: Option<(Option<MessageId>, MessageId)>,
    /// `(old turn root, expected current leaf)`, committed with replacement batch.
    pub replacement: Option<(MessageId, Option<MessageId>)>,
    /// 事务内一并消费的草稿(按 revision CAS;版本已变则保留新内容)。
    pub draft_consumption: Option<DraftConsumptionRequest>,
}

/// 草稿消费方式(发送事务内)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DraftConsumption {
    /// 消费普通草稿(正文与附件清空)。
    Normal,
    /// 消费“编辑最新问题”缓冲(普通草稿保留不动)。
    LatestEdit,
}

/// 发送事务内的草稿消费指令。
#[derive(Debug, Clone)]
pub struct DraftConsumptionRequest {
    /// 草稿所属会话(必须与批次的会话一致)。
    pub conversation_id: ConversationId,
    /// 提交时冻结的草稿版本;行内版本已变则不消费(保留用户新输入)。
    pub expected_revision: i64,
    pub mode: DraftConsumption,
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
            "SELECT id, title, group_id, system_prompt, main_leaf_id, draft,
                    draft_attachments_json, draft_revision,
                    edit_draft, edit_draft_attachments_json, edit_target_id,
                    created_at, updated_at,
                    title_source, title_status, title_revision, title_attempt_count
             FROM conversations ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map([], row_to_conversation)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 读取单会话的**权威行**(切换/恢复用;不读内存副本)。
    pub fn conversation_of(&self, id: ConversationId) -> Result<Option<Conversation>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT id, title, group_id, system_prompt, main_leaf_id, draft,
                        draft_attachments_json, draft_revision,
                        edit_draft, edit_draft_attachments_json, edit_target_id,
                        created_at, updated_at,
                        title_source, title_status, title_revision, title_attempt_count
                 FROM conversations WHERE id = ?1",
                [id.as_str()],
                row_to_conversation,
            )
            .optional()?;
        Ok(row)
    }

    /// 插入或更新会话的**非标题**字段。
    ///
    /// 标题只经 [`Db::rename_conversation_title`] /
    /// [`Db::apply_generated_title`] 字段级写入(PLAN 7.4:草稿/leaf
    /// 更新不得携带旧 title 全量覆盖)。
    pub fn upsert_conversation(&self, c: &Conversation) -> Result<()> {
        let conn = self.lock();
        let attachments_json = serde_json::to_string(&c.draft_attachments)?;
        conn.execute(
            "INSERT INTO conversations
                (id, title, group_id, system_prompt, main_leaf_id, draft,
                 draft_attachments_json, draft_revision,
                 edit_draft, edit_draft_attachments_json, edit_target_id,
                 created_at, updated_at,
                 title_source, title_status, title_revision, title_attempt_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                     ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET
                group_id = ?3, system_prompt = ?4,
                main_leaf_id = ?5,
                draft = CASE WHEN draft <> ?6 OR draft_attachments_json <> ?7
                             THEN ?6 ELSE draft END,
                draft_attachments_json = CASE WHEN draft <> ?6 OR draft_attachments_json <> ?7
                             THEN ?7 ELSE draft_attachments_json END,
                draft_revision = draft_revision + CASE
                             WHEN draft <> ?6 OR draft_attachments_json <> ?7 THEN 1 ELSE 0 END,
                updated_at = ?13",
            rusqlite::params![
                c.id.as_str(),
                c.title,
                c.group_id.map(|g| g.as_str()),
                c.system_prompt,
                c.main_leaf_id.map(|m| m.as_str()),
                c.draft,
                attachments_json,
                c.draft_revision,
                c.edit_draft,
                serde_json::to_string(&c.edit_draft_attachments)?,
                c.edit_target_id.map(|m| m.as_str()),
                c.created_at.timestamp_millis(),
                chrono::Utc::now().timestamp_millis(),
                c.title_source.as_str(),
                c.title_status.as_str(),
                c.title_revision,
                c.title_attempt_count,
            ],
        )?;
        Ok(())
    }

    /// 手动改名:字段级 + 乐观锁。成功返回权威状态;版本不符返回 None。
    pub fn rename_conversation_title(
        &self,
        id: ConversationId,
        title: &str,
        expected_revision: i64,
    ) -> Result<Option<Conversation>> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let n = tx.execute(
            "UPDATE conversations SET
                title = ?2, title_source = 'manual', title_status = 'ready',
                title_revision = title_revision + 1, updated_at = ?3
             WHERE id = ?1 AND title_revision = ?4",
            rusqlite::params![id.as_str(), title, now_ms(), expected_revision],
        )?;
        let conv = if n > 0 {
            Some(read_conversation(&tx, id)?)
        } else {
            None
        };
        tx.commit()?;
        Ok(conv)
    }

    /// 自动标题落库:仅在版本匹配且来源仍允许自动时写入。
    /// 迟到的旧结果(CAS 失败)返回 None,不覆盖手动标题(T07)。
    pub fn apply_generated_title(
        &self,
        id: ConversationId,
        expected_revision: i64,
        title: &str,
    ) -> Result<Option<Conversation>> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let n = tx.execute(
            "UPDATE conversations SET
                title = ?2, title_source = 'generated', title_status = 'ready',
                title_revision = title_revision + 1
             WHERE id = ?1 AND title_revision = ?3
               AND title_source IN ('empty','fallback')
               AND title_status = 'in_flight'",
            rusqlite::params![id.as_str(), title, expected_revision],
        )?;
        let conv = if n > 0 {
            Some(read_conversation(&tx, id)?)
        } else {
            None
        };
        tx.commit()?;
        Ok(conv)
    }

    /// fallback 标题:仅来源允许自动时写入(首问落库后立即调用,7.2)。
    pub fn set_fallback_title(
        &self,
        id: ConversationId,
        title: &str,
    ) -> Result<Option<Conversation>> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let n = tx.execute(
            "UPDATE conversations SET
                title = ?2, title_source = 'fallback',
                title_revision = title_revision + 1, updated_at = ?3
             WHERE id = ?1 AND title_source IN ('empty','fallback')",
            rusqlite::params![id.as_str(), title, now_ms()],
        )?;
        let conv = if n > 0 {
            Some(read_conversation(&tx, id)?)
        } else {
            None
        };
        tx.commit()?;
        Ok(conv)
    }

    /// 标题尝试登记:in_flight + auxiliary_requests 唯一约束(事务)。
    /// 返回 false = 已有在途/登记冲突(同 turn 去重)。
    pub fn begin_title_attempt(
        &self,
        id: ConversationId,
        turn_id: Option<&str>,
        token: &str,
        model: &str,
        expected_revision: i64,
    ) -> Result<bool> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let n = tx.execute(
            "UPDATE conversations SET title_status = 'in_flight',
                title_attempt_count = title_attempt_count + 1,
                title_attempted_at = ?2
             WHERE id = ?1 AND title_revision = ?3 AND title_status = 'needs_generation'",
            rusqlite::params![id.as_str(), now_ms(), expected_revision],
        )?;
        if n == 0 {
            return Ok(false);
        }
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO auxiliary_requests
                (request_token, conversation_id, purpose, turn_id, provider, model,
                 status, started_at, cost_status)
             VALUES (?1, ?2, 'title', ?3, 'deepseek', ?4, 'in_flight', ?5, 'unknown')",
            rusqlite::params![token, id.as_str(), turn_id, model, now_ms()],
        )?;
        if inserted == 0 {
            // 唯一冲突:回滚整个尝试
            return Ok(false);
        }
        tx.commit()?;
        Ok(true)
    }

    /// 标题请求失败:恢复 needs_generation(下次提问再试),记录错误类别。
    pub fn fail_title_attempt(
        &self,
        id: ConversationId,
        token: &str,
        error_kind: &str,
    ) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE conversations SET title_status = 'needs_generation'
             WHERE id = ?1 AND title_status = 'in_flight'",
            [id.as_str()],
        )?;
        tx.execute(
            "UPDATE auxiliary_requests SET status = 'failed', finished_at = ?2,
                error_kind = ?3, cost_status = 'unknown'
             WHERE request_token = ?1",
            rusqlite::params![token, now_ms(), error_kind],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 标题请求成功后的台账收尾(标题本体由 apply_generated_title 写)。
    pub fn complete_aux_request(
        &self,
        token: &str,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        cost_text: Option<String>,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE auxiliary_requests SET status = 'completed', finished_at = ?2,
                prompt_tokens = ?3, completion_tokens = ?4,
                cost_text = ?5, cost_status = CASE WHEN ?5 IS NULL THEN 'unknown' ELSE 'known' END
             WHERE request_token = ?1",
            rusqlite::params![token, now_ms(), prompt_tokens, completion_tokens, cost_text],
        )?;
        Ok(())
    }

    /// 重启恢复:in_flight 标题 → needs_generation(不自动立即重试,7.2);
    /// 在途辅助请求记为 interrupted 失败。
    pub fn recover_title_in_flight(&self) -> Result<usize> {
        let conn = self.lock();
        let n = conn.execute(
            "UPDATE conversations SET title_status = 'needs_generation'
             WHERE title_status = 'in_flight'",
            [],
        )?;
        conn.execute(
            "UPDATE auxiliary_requests SET status = 'failed', finished_at = ?1,
                error_kind = 'interrupted'
             WHERE status = 'in_flight'",
            [now_ms()],
        )?;
        Ok(n)
    }

    /// 辅助请求用量(标题调用;UI 用量详情)。
    pub fn aux_usage_of(&self, conversation: ConversationId) -> Result<Vec<AuxRequestRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT request_token, purpose, turn_id, provider, model, status,
                    started_at, finished_at, prompt_tokens, completion_tokens,
                    cost_text, cost_status, error_kind
             FROM auxiliary_requests WHERE conversation_id = ?1 ORDER BY started_at",
        )?;
        let rows = stmt
            .query_map([conversation.as_str()], |r| {
                Ok(AuxRequestRecord {
                    request_token: r.get(0)?,
                    purpose: r.get(1)?,
                    turn_id: r.get(2)?,
                    provider: r.get(3)?,
                    model: r.get(4)?,
                    status: r.get(5)?,
                    started_at: r.get::<_, i64>(6)?,
                    finished_at: r.get::<_, Option<i64>>(7)?,
                    prompt_tokens: r.get(8)?,
                    completion_tokens: r.get(9)?,
                    cost_text: r.get(10)?,
                    cost_status: r.get(11)?,
                    error_kind: r.get(12)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 模型使用排名(PLAN 6):有效已启用模型内按“到达 Started 的人工
    /// 聊天请求数”降序;同次数按最近使用;再按模型 ID 稳定排序。
    /// 标题等辅助请求独立台账,不参与本排名;并行各路各计一次(每
    /// generation 一行);重试是新 generation 行,按实际发生计。
    pub fn model_usage_rank(
        &self,
        provider: crate::domain::ProviderId,
    ) -> Result<Vec<(String, i64, Option<i64>)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT g.model_id, COUNT(*) AS used, MAX(g.started_at) AS last_used
             FROM generations g
             JOIN send_batches b ON b.id = g.batch_id
             WHERE g.provider_id = ?1 AND g.started_at IS NOT NULL
             GROUP BY g.model_id
             ORDER BY used DESC, last_used DESC, g.model_id ASC",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get(1)?,
                    r.get::<_, Option<i64>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn delete_conversation(&self, id: ConversationId) -> Result<()> {
        let conn = self.lock();
        conn.execute("DELETE FROM conversations WHERE id = ?1", [id.as_str()])?;
        Ok(())
    }

    // ----- 会话字段级更新(不携带 title / draft / leaf 互相覆盖,7.4) -----

    pub fn set_conversation_draft(
        &self,
        id: ConversationId,
        draft: &str,
        attachments_json: &str,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE conversations SET draft = ?2, draft_attachments_json = ?3,
                updated_at = ?4
             WHERE id = ?1",
            rusqlite::params![id.as_str(), draft, attachments_json, now_ms()],
        )?;
        Ok(())
    }

    // ----- Composer 草稿(字段级;单一写入点) -----

    /// 保存普通 composer 草稿。
    ///
    /// `changed` 表示本次调用代表一次**真实的内容变化**(版本 +1):
    /// 即使用户重新输入了与库中相同的正文,也视为一次变化(否则发送
    /// 事务的 CAS 会把新输入误当旧草稿清掉)。
    /// `changed = false` 且内容与库中一致时不写库、不涨版本
    /// (纯光标/选区/滚动与冗余 flush)。
    /// 返回写入后的版本号。
    pub fn save_draft_content(
        &self,
        id: ConversationId,
        text: &str,
        attachments_json: &str,
        changed: bool,
    ) -> Result<i64> {
        let conn = self.lock();
        let current: (String, String, i64) = conn.query_row(
            "SELECT draft, draft_attachments_json, draft_revision
             FROM conversations WHERE id = ?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let differs = current.0 != text || current.1 != attachments_json;
        if !differs && !changed {
            return Ok(current.2);
        }
        conn.execute(
            "UPDATE conversations SET draft = ?2, draft_attachments_json = ?3,
                draft_revision = draft_revision + ?4, updated_at = ?5
             WHERE id = ?1",
            rusqlite::params![
                id.as_str(),
                text,
                attachments_json,
                if differs || changed { 1 } else { 0 },
                now_ms()
            ],
        )?;
        Ok(current.2 + i64::from(differs || changed))
    }

    /// 保存“编辑最新问题”缓冲(与普通草稿相互独立)。
    ///
    /// 语义与 [`Db::save_draft_content`] 相同。
    pub fn save_edit_buffer(
        &self,
        id: ConversationId,
        text: &str,
        attachments_json: &str,
        target: Option<MessageId>,
        changed: bool,
    ) -> Result<i64> {
        let conn = self.lock();
        let current: (String, String, Option<String>, i64) = conn.query_row(
            "SELECT edit_draft, edit_draft_attachments_json, edit_target_id, draft_revision
             FROM conversations WHERE id = ?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        let target_text = target.map(|t| t.as_str());
        let differs = current.0 != text
            || current.1 != attachments_json
            || current.2.as_deref() != target_text.as_deref();
        if !differs && !changed {
            return Ok(current.3);
        }
        conn.execute(
            "UPDATE conversations SET edit_draft = ?2, edit_draft_attachments_json = ?3,
                edit_target_id = ?4, draft_revision = draft_revision + ?5, updated_at = ?6
             WHERE id = ?1",
            rusqlite::params![
                id.as_str(),
                text,
                attachments_json,
                target_text,
                if differs || changed { 1 } else { 0 },
                now_ms()
            ],
        )?;
        Ok(current.3 + i64::from(differs || changed))
    }

    /// 清空“编辑最新问题”缓冲(普通草稿不动)。已为空则不写库、不涨版本。
    pub fn clear_edit_buffer(&self, id: ConversationId) -> Result<i64> {
        let conn = self.lock();
        let current: (String, String, Option<String>, i64) = conn.query_row(
            "SELECT edit_draft, edit_draft_attachments_json, edit_target_id, draft_revision
             FROM conversations WHERE id = ?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        if current.0.is_empty() && current.1 == "[]" && current.2.is_none() {
            return Ok(current.3);
        }
        conn.execute(
            "UPDATE conversations SET edit_draft = '', edit_draft_attachments_json = '[]',
                edit_target_id = NULL, draft_revision = draft_revision + 1, updated_at = ?2
             WHERE id = ?1",
            rusqlite::params![id.as_str(), now_ms()],
        )?;
        Ok(current.3 + 1)
    }

    pub fn set_conversation_main_leaf(
        &self,
        id: ConversationId,
        leaf: Option<MessageId>,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE conversations SET main_leaf_id = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), leaf.map(|m| m.as_str()), now_ms()],
        )?;
        Ok(())
    }

    pub fn set_conversation_group(&self, id: ConversationId, group: Option<GroupId>) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE conversations SET group_id = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), group.map(|g| g.as_str()), now_ms()],
        )?;
        Ok(())
    }

    pub fn set_conversation_system_prompt(&self, id: ConversationId, prompt: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE conversations SET system_prompt = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id.as_str(), prompt, now_ms()],
        )?;
        Ok(())
    }

    // ----- 消息 -----

    pub fn insert_message(&self, m: &Message) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO messages(id, conversation_id, parent_id, role, content, attachments_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                m.id.as_str(),
                m.conversation_id.as_str(),
                m.parent_id.map(|p| p.as_str()),
                m.role.as_str(),
                m.content,
                serde_json::to_string(&m.attachments)?,
                m.created_at.timestamp_millis(),
            ],
        )?;
        Ok(())
    }

    /// 会话全部消息(按创建时间);树结构由内存构建。
    pub fn messages_of(&self, conversation: ConversationId) -> Result<Vec<Message>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, parent_id, role, content, attachments_json, created_at, removed_at
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at, id",
        )?;
        let rows = stmt
            .query_map([conversation.as_str()], |r| {
                Ok((
                    Message {
                        id: MessageId::from_uuid_str(r.get::<_, String>(0)?.as_str()),
                        conversation_id: ConversationId::from_uuid_str(
                            r.get::<_, String>(1)?.as_str(),
                        ),
                        parent_id: r
                            .get::<_, Option<String>>(2)?
                            .map(|s| MessageId::from_uuid_str(s.as_str())),
                        role: crate::domain::Role::from_str(&r.get::<_, String>(3)?)
                            .unwrap_or(crate::domain::Role::User),
                        content: r.get(4)?,
                        attachments: r
                            .get::<_, Option<String>>(5)?
                            .and_then(|s| serde_json::from_str(&s).ok())
                            .unwrap_or_default(),
                        created_at: ms_to_dt(r.get(6)?),
                    },
                    r.get::<_, Option<i64>>(7)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let parents: std::collections::HashMap<_, _> =
            rows.iter().map(|(m, _)| (m.id, m.parent_id)).collect();
        let removed: std::collections::HashSet<_> = rows
            .iter()
            .filter(|(_, removed_at)| removed_at.is_some())
            .map(|(m, _)| m.id)
            .collect();
        Ok(rows
            .into_iter()
            .filter_map(|(mut message, removed_at)| {
                if removed_at.is_some() {
                    return None;
                }
                while message
                    .parent_id
                    .is_some_and(|parent| removed.contains(&parent))
                {
                    message.parent_id = parents.get(&message.parent_id?).copied().flatten();
                }
                Some(message)
            })
            .collect())
    }

    /// Hide one message from future context while retaining its generations and usage ledger.
    pub fn remove_message(&self, id: MessageId) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let conversation: String = tx.query_row(
            "SELECT conversation_id, parent_id FROM messages WHERE id = ?1 AND removed_at IS NULL",
            [id.as_str()],
            |r| r.get(0),
        )?;
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM generations WHERE (user_message_id = ?1 OR assistant_message_id = ?1) AND status IN ('queued','connecting','streaming'))",
            [id.as_str()],
            |r| r.get(0),
        )?;
        if active {
            return Err(StorageError::NotFound(
                "cannot remove a message with an active request".into(),
            ));
        }
        tx.execute(
            "UPDATE messages SET removed_at = ?2 WHERE id = ?1",
            rusqlite::params![id.as_str(), now_ms()],
        )?;
        let conv = ConversationId::from_uuid_str(&conversation);
        let leaf: Option<String> = tx.query_row(
            "SELECT main_leaf_id FROM conversations WHERE id = ?1",
            [conversation.as_str()],
            |r| r.get(0),
        )?;
        if leaf.as_deref() == Some(id.as_str().as_str()) {
            let mut ancestor: Option<String> = tx.query_row(
                "SELECT parent_id FROM messages WHERE id = ?1",
                [id.as_str()],
                |r| r.get(0),
            )?;
            while let Some(candidate) = ancestor.clone() {
                let removed_at: Option<i64> = tx.query_row(
                    "SELECT removed_at FROM messages WHERE id = ?1",
                    [candidate.as_str()],
                    |r| r.get(0),
                )?;
                if removed_at.is_none() {
                    break;
                }
                ancestor = tx.query_row(
                    "SELECT parent_id FROM messages WHERE id = ?1",
                    [candidate.as_str()],
                    |r| r.get(0),
                )?;
            }
            tx.execute(
                "UPDATE conversations SET main_leaf_id = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![conv.as_str(), ancestor, now_ms()],
            )?;
        }
        tx.commit()?;
        Ok(())
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
        // 本批之前该会话已有的消息数(0 = 该会话的首发,用于释放 New 槽位)。
        let prior_messages: i64 = tx.query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1",
            [rec.conversation_id.as_str()],
            |r| r.get(0),
        )?;
        if let Some((old_root, expected_leaf)) = &rec.replacement {
            let root_role: String = tx
                .query_row(
                    "SELECT role FROM messages WHERE id = ?1 AND conversation_id = ?2 AND removed_at IS NULL",
                    rusqlite::params![old_root.as_str(), rec.conversation_id.as_str()],
                    |r| r.get(0),
                )
                .map_err(|_| StorageError::NotFound("edited question no longer exists".into()))?;
            if root_role != "user" {
                return Err(StorageError::NotFound(
                    "only a user question can be edited".into(),
                ));
            }
            let current_leaf: Option<String> = tx.query_row(
                "SELECT main_leaf_id FROM conversations WHERE id = ?1",
                [rec.conversation_id.as_str()],
                |r| r.get(0),
            )?;
            let expected_leaf_text = expected_leaf.as_ref().map(|id| id.as_str().to_string());
            if current_leaf != expected_leaf_text {
                return Err(StorageError::NotFound(
                    "conversation moved while editing the question".into(),
                ));
            }
            let active: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM generations WHERE user_message_id = ?1 AND status IN ('queued','connecting','streaming'))",
                [old_root.as_str()],
                |r| r.get(0),
            )?;
            if active {
                return Err(StorageError::NotFound(
                    "the edited question still has an active request".into(),
                ));
            }
            if let Some(leaf) = expected_leaf {
                let latest_user: Option<String> = tx.query_row(
                    "WITH RECURSIVE path(id,parent_id,role,removed_at,depth) AS (
                        SELECT id,parent_id,role,removed_at,0 FROM messages WHERE id = ?1 AND conversation_id = ?2
                        UNION ALL SELECT p.id,p.parent_id,p.role,p.removed_at,path.depth+1 FROM messages p JOIN path ON p.id = path.parent_id
                     ) SELECT id FROM path WHERE role = 'user' AND removed_at IS NULL ORDER BY depth LIMIT 1",
                    rusqlite::params![leaf.as_str(), rec.conversation_id.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
                if latest_user.as_deref() != Some(old_root.as_str().as_str()) {
                    return Err(StorageError::NotFound(
                        "only the latest question can be edited".into(),
                    ));
                }
                let has_later_question: bool = tx.query_row(
                    "WITH RECURSIVE descendants(id) AS (
                        SELECT id FROM messages WHERE parent_id = ?1 AND conversation_id = ?2
                        UNION ALL SELECT m.id FROM messages m JOIN descendants d ON m.parent_id = d.id
                     ) SELECT EXISTS(
                        SELECT 1 FROM messages m JOIN descendants d ON d.id = m.id
                        WHERE m.role = 'user' AND m.removed_at IS NULL
                     )",
                    rusqlite::params![old_root.as_str(), rec.conversation_id.as_str()],
                    |r| r.get(0),
                )?;
                if has_later_question {
                    return Err(StorageError::NotFound(
                        "the edited question has a later visible question in another branch".into(),
                    ));
                }
            }
        }
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
                "INSERT INTO messages(id, conversation_id, parent_id, role, content, attachments_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    m.id.as_str(),
                    m.conversation_id.as_str(),
                    m.parent_id.map(|p| p.as_str()),
                    m.role.as_str(),
                    m.content,
                    serde_json::to_string(&m.attachments)?,
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
                     failure_json, request_snapshot_json, retry_root_id,
                     retry_not_before_ms, dispatch_phase,
                     started_at, finished_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)",
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
                    g.failure.as_ref().map(|f| f.to_json()),
                    g.request_snapshot
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()?,
                    g.retry_root_id.map(|r| r.as_str()),
                    g.retry_not_before_ms,
                    g.dispatch_phase.as_str(),
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
        if let Some((old_root, expected_leaf)) = &rec.replacement {
            let Some((_, new_leaf)) = &rec.intended_main_leaf else {
                return Err(StorageError::Migration(
                    "replacement batch has no new leaf".into(),
                ));
            };
            let updated = match expected_leaf {
                Some(expected) => tx.execute(
                    "UPDATE conversations SET main_leaf_id = ?2, updated_at = ?3 WHERE id = ?1 AND main_leaf_id = ?4",
                    rusqlite::params![rec.conversation_id.as_str(), new_leaf.as_str(), now_ms(), expected.as_str()],
                )?,
                None => tx.execute(
                    "UPDATE conversations SET main_leaf_id = ?2, updated_at = ?3 WHERE id = ?1 AND main_leaf_id IS NULL",
                    rusqlite::params![rec.conversation_id.as_str(), new_leaf.as_str(), now_ms()],
                )?,
            };
            if updated != 1 {
                return Err(StorageError::NotFound(
                    "conversation changed before the edited turn could be replaced".into(),
                ));
            }
            tx.execute(
                "WITH RECURSIVE subtree(id) AS (
                    SELECT id FROM messages WHERE id = ?1 AND conversation_id = ?2 AND removed_at IS NULL
                    UNION ALL SELECT m.id FROM messages m JOIN subtree s ON m.parent_id = s.id
                 ) UPDATE messages SET removed_at = ?3 WHERE id IN (SELECT id FROM subtree)",
                rusqlite::params![old_root.as_str(), rec.conversation_id.as_str(), now_ms()],
            )?;
        } else if let Some((expected, new)) = &rec.intended_main_leaf {
            match expected {
                Some(exp) => {
                    tx.execute(
                        "UPDATE conversations SET main_leaf_id = ?2, updated_at = ?3
                         WHERE id = ?1 AND main_leaf_id = ?4",
                        rusqlite::params![
                            rec.conversation_id.as_str(),
                            new.as_str(),
                            now_ms(),
                            exp.as_str()
                        ],
                    )?;
                }
                None => {
                    tx.execute(
                        "UPDATE conversations SET main_leaf_id = ?2, updated_at = ?3
                         WHERE id = ?1 AND main_leaf_id IS NULL",
                        rusqlite::params![rec.conversation_id.as_str(), new.as_str(), now_ms()],
                    )?;
                }
            }
        }
        // 草稿消费(与批次同事务):版本仍等于提交时冻结的版本才清空,
        // 用户在提交后又输入了新内容则保留(CAS;不因草稿变化而回滚发送)。
        if let Some(request) = &rec.draft_consumption {
            if request.conversation_id == rec.conversation_id {
                match request.mode {
                    DraftConsumption::Normal => {
                        tx.execute(
                            "UPDATE conversations SET draft = '', draft_attachments_json = '[]',
                                updated_at = ?2
                             WHERE id = ?1 AND draft_revision = ?3",
                            rusqlite::params![
                                rec.conversation_id.as_str(),
                                now_ms(),
                                request.expected_revision
                            ],
                        )?;
                    }
                    DraftConsumption::LatestEdit => {
                        tx.execute(
                            "UPDATE conversations SET edit_draft = '',
                                    edit_draft_attachments_json = '[]',
                                    edit_target_id = NULL, updated_at = ?2
                             WHERE id = ?1 AND draft_revision = ?3",
                            rusqlite::params![
                                rec.conversation_id.as_str(),
                                now_ms(),
                                request.expected_revision
                            ],
                        )?;
                    }
                }
            }
        }
        // 首次发送落库成功 → 释放与该会话匹配的 New 槽位指针
        // (只清除匹配的指针;不影响后来创建的另一个新会话)。
        if prior_messages == 0 {
            tx.execute(
                "DELETE FROM ui_state WHERE key = ?1 AND value = ?2",
                rusqlite::params![
                    PENDING_NEW_KEY,
                    json_string(rec.conversation_id.as_str().as_str())
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 非终态推进(queued→connecting→streaming):无条件写状态,
    /// started_at 只补不覆盖。
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

    /// 终态 CAS:只有活动态可转终态;重复终态不覆盖 finished_at、
    /// 不抹掉 error。返回是否发生了转换(首个终态)。
    pub fn finish_generation(
        &self,
        request: GenerationId,
        status: crate::domain::GenerationStatus,
        error: Option<&str>,
        failure_json: Option<&str>,
        retry_not_before_ms: Option<i64>,
        remote_generation_id: Option<&str>,
    ) -> Result<bool> {
        debug_assert!(status.is_terminal());
        let conn = self.lock();
        let now = now_ms();
        let n = conn.execute(
            "UPDATE generations SET
                status = ?2,
                error = COALESCE(?3, error),
                failure_json = COALESCE(?4, failure_json),
                retry_not_before_ms = COALESCE(?5, retry_not_before_ms),
                remote_generation_id = COALESCE(?6, remote_generation_id),
                finished_at = ?7
             WHERE request_id = ?1 AND status IN ('queued','connecting','streaming')",
            rusqlite::params![
                request.as_str(),
                status.as_str(),
                error,
                failure_json,
                retry_not_before_ms,
                remote_generation_id,
                now
            ],
        )?;
        Ok(n > 0)
    }

    /// 请求阶段推进(DispatchAttempted 起 may_be_billed)。
    pub fn set_dispatch_phase(
        &self,
        request: GenerationId,
        phase: crate::domain::DispatchPhase,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE generations SET dispatch_phase = ?2 WHERE request_id = ?1
             AND (dispatch_phase = 'not_dispatched' OR ?2 = 'remote_accepted')",
            rusqlite::params![request.as_str(), phase.as_str()],
        )?;
        Ok(())
    }

    /// main_leaf CAS:期望当前值列表任一匹配才移动(None 期望 = 当前为
    /// NULL)。晚到的完成/重试回执不得抢回用户已移动的分支。
    pub fn cas_main_leaf(
        &self,
        conversation: ConversationId,
        expected: CasExpectation<'_>,
        new: MessageId,
    ) -> Result<bool> {
        let conn = self.lock();
        let n = match expected {
            CasExpectation::Null => conn.execute(
                "UPDATE conversations SET main_leaf_id = ?2, updated_at = ?3
                 WHERE id = ?1 AND main_leaf_id IS NULL",
                rusqlite::params![conversation.as_str(), new.as_str(), now_ms()],
            )?,
            CasExpectation::AnyOf(ids) => {
                let list = ids
                    .iter()
                    .map(|i| format!("'{}'", i.as_str()))
                    .collect::<Vec<_>>()
                    .join(",");
                conn.execute(
                    &format!(
                        "UPDATE conversations SET main_leaf_id = ?2, updated_at = ?3
                         WHERE id = ?1 AND main_leaf_id IN ({list})"
                    ),
                    rusqlite::params![conversation.as_str(), new.as_str(), now_ms()],
                )?
            }
        };
        Ok(n > 0)
    }

    /// 当前 main_leaf(读取,CAS 判定用)。
    pub fn main_leaf_of(&self, conversation: ConversationId) -> Result<Option<MessageId>> {
        let conn = self.lock();
        let v = conn
            .query_row(
                "SELECT main_leaf_id FROM conversations WHERE id = ?1",
                [conversation.as_str()],
                |r| r.get::<_, Option<String>>(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?
            .flatten();
        Ok(v.map(|s| MessageId::from_uuid_str(s.as_str())))
    }

    /// lineage 是否有活动尝试(防重兜底;与唯一索引配合)。
    pub fn lineage_has_active(&self, root: GenerationId) -> Result<bool> {
        let conn = self.lock();
        let n = conn.query_row(
            "SELECT COUNT(*) FROM generations
             WHERE retry_root_id = ?1 AND status IN ('queued','connecting','streaming')",
            [root.as_str()],
            |r| r.get::<_, i64>(0),
        )?;
        Ok(n > 0)
    }

    /// lineage 最新一次失败(冷却/补救计算用最新持久数据)。
    pub fn lineage_latest_failure(
        &self,
        root: GenerationId,
    ) -> Result<Option<crate::domain::FailureInfo>> {
        let conn = self.lock();
        let v: Option<String> = conn
            .query_row(
                "SELECT failure_json FROM generations
                 WHERE retry_root_id = ?1 AND failure_json IS NOT NULL
                 ORDER BY COALESCE(finished_at, started_at, 0) DESC LIMIT 1",
                [root.as_str()],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        Ok(v.as_deref().and_then(crate::domain::FailureInfo::from_json))
    }

    /// lineage 尝试次数(退避指数输入;含原请求)。
    pub fn lineage_attempt_count(&self, root: GenerationId) -> Result<u32> {
        let conn = self.lock();
        let n = conn.query_row(
            "SELECT COUNT(*) FROM generations WHERE retry_root_id = ?1",
            [root.as_str()],
            |r| r.get::<_, i64>(0),
        )?;
        Ok(n.clamp(0, u32::MAX as i64) as u32)
    }

    /// 按请求定位其 lineage 尝试次数(失败写入时计算退避用)。
    pub fn lineage_attempt_count_of(&self, request: GenerationId) -> Result<Option<u32>> {
        let conn = self.lock();
        let root: Option<String> = conn
            .query_row(
                "SELECT retry_root_id FROM generations WHERE request_id = ?1",
                [request.as_str()],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        let Some(root) = root else {
            return Ok(None);
        };
        let n = conn.query_row(
            "SELECT COUNT(*) FROM generations WHERE retry_root_id = ?1",
            [root],
            |r| r.get::<_, i64>(0),
        )?;
        Ok(Some(n.clamp(0, u32::MAX as i64) as u32))
    }

    /// 本地未发送的失败:用量不留 pending("费用查询中")。
    pub fn mark_usage_unavailable(&self, request: GenerationId, _note: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "UPDATE usage_records SET status = 'unavailable', queried_at = ?2
             WHERE request_id = ?1 AND status = 'pending'",
            rusqlite::params![request.as_str(), now_ms()],
        )?;
        Ok(())
    }

    /// 按 lineage 根聚合(重试尝试展示)。
    pub fn generations_of_lineage(
        &self,
        root: GenerationId,
    ) -> Result<Vec<(GenerationId, crate::domain::GenerationStatus)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT request_id, status FROM generations
             WHERE retry_root_id = ?1 ORDER BY COALESCE(started_at, 0), request_id",
        )?;
        let rows = stmt
            .query_map([root.as_str()], |r| {
                Ok((
                    GenerationId::from_uuid_str(r.get::<_, String>(0)?.as_str()),
                    crate::domain::GenerationStatus::from_str(&r.get::<_, String>(1)?)
                        .unwrap_or(crate::domain::GenerationStatus::Interrupted),
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn generations_of(&self, conversation: ConversationId) -> Result<Vec<Generation>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT g.request_id, g.batch_id, g.user_message_id, g.assistant_message_id,
                    g.provider_id, g.model_id, g.status, g.params_json, g.system_prompt,
                    g.context_json, g.price_json, g.remote_generation_id, g.error, g.retry_of,
                    g.started_at, g.finished_at, g.failure_json, g.request_snapshot_json,
                    g.retry_root_id, g.retry_not_before_ms, g.dispatch_phase
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
                    g.started_at, g.finished_at, g.failure_json, g.request_snapshot_json,
                    g.retry_root_id, g.retry_not_before_ms, g.dispatch_phase
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

    pub fn usage_record(&self, request: GenerationId) -> Result<Option<UsageRecord>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                "SELECT request_id, prompt_tokens, completion_tokens, total_tokens,
                        cost_text, cost_source, status, remote_generation_id, queried_at
                 FROM usage_records WHERE request_id = ?1",
                [request.as_str()],
                row_to_usage,
            )
            .optional()?;
        Ok(row)
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
            Some((_, _ex_prompt, _ex_completion, ex_cost, ex_source, ex_status)) => {
                // 不回退:已知费用优先
                let incoming_is_api =
                    rec.cost_source == Some(crate::domain::CostSource::ApiReported);
                let existing_is_api = ex_source.as_deref() == Some("api");
                let keep_existing = ex_cost.is_some() && (!incoming_is_api && existing_is_api);
                let final_cost = if keep_existing {
                    ex_cost
                } else {
                    rec.cost.map(|c| c.to_string()).or(ex_cost)
                };
                let final_status = if final_cost.is_some() {
                    "known"
                } else if ex_status == "unavailable" {
                    "unavailable"
                } else {
                    rec.status.as_str()
                };
                let final_source = if keep_existing {
                    ex_source
                } else {
                    rec.cost_source
                        .map(|s| s.as_str().to_string())
                        .or(ex_source)
                };
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

    /// 最多补查的终态用量。包含远端 ID 尚未取到的记录,由服务层标记 unavailable。
    pub fn terminal_pending_usage(&self, limit: usize) -> Result<Vec<UsageRecord>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT u.request_id, u.prompt_tokens, u.completion_tokens, u.total_tokens,
                    u.cost_text, u.cost_source, u.status,
                    COALESCE(u.remote_generation_id, g.remote_generation_id), u.queried_at
             FROM usage_records u JOIN generations g ON g.request_id = u.request_id
             WHERE u.status = 'pending'
               AND g.status IN ('completed','cancelled','failed','interrupted')
               AND (u.queried_at IS NULL OR u.queried_at <= ?1)
             ORDER BY COALESCE(g.finished_at, g.started_at, 0) LIMIT ?2",
        )?;
        let cutoff = now_ms() - 60_000;
        let rows = stmt
            .query_map(
                rusqlite::params![cutoff, limit.min(50) as i64],
                row_to_usage,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 立即保存远端 ID,覆盖 generation 与 usage 中缺失值,永不替换首个值。
    pub fn record_remote_generation_id(
        &self,
        request: GenerationId,
        remote_generation_id: &str,
    ) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE generations SET remote_generation_id = COALESCE(remote_generation_id, ?2)
             WHERE request_id = ?1",
            rusqlite::params![request.as_str(), remote_generation_id],
        )?;
        tx.execute(
            "UPDATE usage_records SET remote_generation_id = COALESCE(remote_generation_id, ?2)
             WHERE request_id = ?1",
            rusqlite::params![request.as_str(), remote_generation_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ----- 目录与偏好 -----

    /// 提供方注册(内置 OpenRouter 幂等存在)。
    pub fn ensure_provider(
        &self,
        id: crate::domain::ProviderId,
        kind: &str,
        base_url: &str,
    ) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "INSERT INTO providers(id, kind, base_url, credential_ref) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET kind = ?2, base_url = ?3",
            rusqlite::params![id.as_str(), kind, base_url, "openrouter"],
        )?;
        Ok(())
    }

    /// 原子替换目录缓存(单事务;失败保留旧数据)。
    pub fn replace_catalog(
        &self,
        provider: crate::domain::ProviderId,
        entries: &[crate::providers::CatalogEntry],
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM catalog_models WHERE provider_id = ?1",
            [provider.as_str()],
        )?;
        let mut stmt = tx.prepare(
            "INSERT INTO catalog_models
                (provider_id, model_id, name, context_window, max_output,
                 pricing_json, capabilities_json, defaults_json, available, updated_at, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'{}')",
        )?;
        for e in entries {
            stmt.execute(rusqlite::params![
                provider.as_str(),
                e.model_id,
                e.name,
                e.context_window,
                e.max_output,
                serde_json::to_string(&e.pricing)?,
                serde_json::to_string(&e.capabilities)?,
                e.server_defaults.as_ref().map(|v| v.to_string()),
                e.available,
                now.timestamp_millis(),
            ])?;
        }
        drop(stmt);
        tx.commit()?;
        Ok(())
    }

    pub fn catalog(
        &self,
        provider: crate::domain::ProviderId,
    ) -> Result<Vec<crate::providers::CatalogEntry>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT model_id, name, context_window, max_output, pricing_json,
                    capabilities_json, defaults_json, available
             FROM catalog_models WHERE provider_id = ?1 ORDER BY name",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| {
                Ok(crate::providers::CatalogEntry {
                    model_id: r.get(0)?,
                    name: r.get(1)?,
                    context_window: r.get(2)?,
                    max_output: r.get(3)?,
                    pricing: r
                        .get::<_, Option<String>>(4)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default(),
                    capabilities: r
                        .get::<_, Option<String>>(5)?
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_default(),
                    server_defaults: r
                        .get::<_, Option<String>>(6)?
                        .and_then(|s| serde_json::from_str(&s).ok()),
                    available: r.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn catalog_updated_at(
        &self,
        provider: crate::domain::ProviderId,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let conn = self.lock();
        let v = conn
            .query_row(
                "SELECT MAX(updated_at) FROM catalog_models WHERE provider_id = ?1",
                [provider.as_str()],
                |r| r.get::<_, Option<i64>>(0),
            )
            .unwrap_or(None);
        Ok(v.map(ms_to_dt))
    }

    pub fn upsert_preference(
        &self,
        provider: crate::domain::ProviderId,
        model: &str,
        pref: &crate::providers::catalog::ModelPreference,
    ) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO model_preferences(provider_id, model_id, enabled, sort_order, is_default, overrides_json)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(provider_id, model_id) DO UPDATE SET
                enabled = ?3, sort_order = ?4, is_default = ?5, overrides_json = ?6",
            rusqlite::params![
                provider.as_str(),
                model,
                pref.enabled,
                pref.sort_order,
                pref.is_default,
                pref.overrides.as_ref().map(|v| v.to_string()),
            ],
        )?;
        if pref.is_default {
            // 唯一默认
            tx.execute(
                "UPDATE model_preferences SET is_default = 0
                 WHERE provider_id = ?1 AND model_id != ?2",
                rusqlite::params![provider.as_str(), model],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn preferences(
        &self,
        provider: crate::domain::ProviderId,
    ) -> Result<Vec<(String, crate::providers::catalog::ModelPreference)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT model_id, enabled, sort_order, is_default, overrides_json
             FROM model_preferences WHERE provider_id = ?1 ORDER BY sort_order",
        )?;
        let rows = stmt
            .query_map([provider.as_str()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    crate::providers::catalog::ModelPreference {
                        enabled: r.get::<_, i64>(1)? != 0,
                        sort_order: r.get(2)?,
                        is_default: r.get::<_, i64>(3)? != 0,
                        overrides: r
                            .get::<_, Option<String>>(4)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                    },
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ----- UI 状态 -----

    /// 未成功提交首条提问的“新会话槽位”指针。
    ///
    /// New 按钮优先回到该槽位(连点不制造多个空壳);首次发送落库成功后
    /// 由发送事务释放;删除会话或启动校验失败时清理。
    pub fn pending_new_conversation(&self) -> Result<Option<ConversationId>> {
        Ok(self
            .get_ui_state(PENDING_NEW_KEY)?
            .and_then(|v| v.as_str().map(ConversationId::from_uuid_str)))
    }

    /// 记录新会话槽位。
    pub fn set_pending_new_conversation(&self, id: ConversationId) -> Result<()> {
        self.set_ui_state(PENDING_NEW_KEY, &serde_json::json!(id.as_str()))
    }

    /// 清除新会话槽位。
    ///
    /// `Some(id)` 只在指针指向该会话时清除(不影响后来创建的另一个槽位);
    /// `None` 无条件清除(删除会话/启动校验修复)。
    pub fn clear_pending_new_conversation(&self, id: Option<ConversationId>) -> Result<()> {
        let conn = self.lock();
        match id {
            Some(id) => {
                conn.execute(
                    "DELETE FROM ui_state WHERE key = ?1 AND value = ?2",
                    rusqlite::params![PENDING_NEW_KEY, json_string(id.as_str().as_str())],
                )?;
            }
            None => {
                conn.execute("DELETE FROM ui_state WHERE key = ?1", [PENDING_NEW_KEY])?;
            }
        }
        Ok(())
    }

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

/// 辅助请求台账行(标题等;与普通聊天 generations 分离,PLAN 7.1)。
#[derive(Debug, Clone)]
pub struct AuxRequestRecord {
    pub request_token: String,
    pub purpose: String,
    pub turn_id: Option<String>,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub cost_text: Option<String>,
    pub cost_status: String,
    pub error_kind: Option<String>,
}

fn row_to_conversation(r: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
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
        draft_attachments: r
            .get::<_, Option<String>>(6)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        draft_revision: r.get(7)?,
        edit_draft: r.get(8)?,
        edit_draft_attachments: r
            .get::<_, Option<String>>(9)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        edit_target_id: r
            .get::<_, Option<String>>(10)?
            .map(|s| MessageId::from_uuid_str(s.as_str())),
        created_at: ms_to_dt(r.get(11)?),
        updated_at: ms_to_dt(r.get(12)?),
        title_source: crate::domain::TitleSource::from_str(&r.get::<_, String>(13)?)
            .unwrap_or_default(),
        title_status: crate::domain::TitleStatus::from_str(&r.get::<_, String>(14)?)
            .unwrap_or_default(),
        title_revision: r.get(15)?,
        title_attempt_count: r.get(16)?,
    })
}

/// 事务内读取单会话(改名/自动标题落库后取权威状态)。
fn read_conversation(
    tx: &rusqlite::Transaction<'_>,
    id: ConversationId,
) -> rusqlite::Result<Conversation> {
    tx.query_row(
        "SELECT id, title, group_id, system_prompt, main_leaf_id, draft,
                draft_attachments_json, draft_revision,
                edit_draft, edit_draft_attachments_json, edit_target_id,
                created_at, updated_at,
                title_source, title_status, title_revision, title_attempt_count
         FROM conversations WHERE id = ?1",
        [id.as_str()],
        row_to_conversation,
    )
}

fn row_to_generation(r: &rusqlite::Row<'_>) -> rusqlite::Result<Generation> {
    let ctx: String = r.get(9)?;
    let ctx_ids: Vec<String> = serde_json::from_str(&ctx).unwrap_or_default();
    let price_json: Option<String> = r.get(10)?;
    let failure_json: Option<String> = r.get(16)?;
    let snapshot_json: Option<String> = r.get(17)?;
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
        // 新列(0004):读取失败安全降级,不阻止会话读取
        retry_root_id: r
            .get::<_, Option<String>>(18)?
            .map(|s| GenerationId::from_uuid_str(s.as_str())),
        failure: failure_json
            .as_deref()
            .and_then(crate::domain::FailureInfo::from_json),
        request_snapshot: snapshot_json
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok()),
        retry_not_before_ms: r.get(19)?,
        dispatch_phase: crate::domain::DispatchPhase::from_str(
            &r.get::<_, Option<String>>(20)?.unwrap_or_default(),
        ),
    })
}

/// main_leaf CAS 期望。
pub enum CasExpectation<'a> {
    /// 当前必须为 NULL。
    Null,
    /// 当前值在列表内任一匹配。
    AnyOf(&'a [MessageId]),
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
