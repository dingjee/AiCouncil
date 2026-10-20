//! 配置文件(非敏感连接配置与通用设置)+ 平台数据目录。
//!
//! 数据源边界(PLAN 3.1):配置文件保存提供方非敏感连接配置与通用设置;
//! 数据库保存会话/偏好/目录缓存;系统凭据保存 Key。同一字段不双写。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::domain::ProviderId;

/// 通用应用配置(data_dir/config.json)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AppConfig {
    pub version: u32,
    /// 自定义提供方(内置 OpenRouter 之外)。
    pub providers: Vec<CustomProvider>,
    pub general: GeneralSettings,
    /// UIState(当前会话、侧栏宽度等)序列化;与会话相关的树状态在 DB。
    pub window: WindowState,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GeneralSettings {
    /// 并发上限(默认 4)。
    pub max_concurrent_requests: u32,
    /// 假定输出 token(预估用,默认 1000)。
    pub assumed_output_tokens: u64,
    /// 连接超时秒。
    pub connect_timeout_secs: u64,
    /// 读取空闲超时秒。
    pub read_idle_timeout_secs: u64,
    /// 正文字号(逻辑 px;12–22;None=默认 15;PLAN 3.2)。
    pub body_text_size: Option<f32>,
    /// UI 语言(默认 en;未支持回退 en;PLAN 3.3)。
    pub locale: String,
    /// 主题(本轮固定 soft_sage;预留扩展)。
    pub theme_id: String,
}

impl GeneralSettings {
    pub fn with_defaults() -> Self {
        Self {
            max_concurrent_requests: 4,
            assumed_output_tokens: 1000,
            connect_timeout_secs: 15,
            read_idle_timeout_secs: 120,
            body_text_size: None,
            locale: "en".to_string(),
            theme_id: "soft_sage".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WindowState {
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: ProviderId,
    pub kind: String, // "openai_compatible"
    pub base_url: String,
    pub display_name: String,
}

/// 默认使用 exe 同级 data；测试/离线验收可显式指定隔离目录。
pub fn data_dir() -> std::io::Result<PathBuf> {
    let dir = if let Some(test_dir) = std::env::var_os("AICOUNCIL_DATA_DIR") {
        PathBuf::from(test_dir)
    } else {
        portable_data_dir(&std::env::current_exe()?)?
    };
    std::fs::create_dir_all(&dir)?;
    migrate_legacy(&dir)?;
    Ok(dir)
}

fn portable_data_dir(exe: &std::path::Path) -> std::io::Result<PathBuf> {
    Ok(exe
        .parent()
        .ok_or_else(|| std::io::Error::other("无法确定程序目录"))?
        .join("data"))
}

fn migrate_legacy(local: &std::path::Path) -> std::io::Result<()> {
    if std::env::var_os("AICOUNCIL_DATA_DIR").is_some()
        || std::env::var_os("AICOUNCIL_DEMO_MODE").is_some()
    {
        return Ok(());
    }
    let Some(old) =
        directories::ProjectDirs::from("", "", "AiCouncil").map(|p| p.data_dir().to_path_buf())
    else {
        return Ok(());
    };
    let dest = local.join("aicouncil.db");
    let src = old.join("aicouncil.db");
    if !dest.exists() && src.exists() {
        migrate_database(&src, &dest)?;
    }
    let config = local.join("config.json");
    let old_config = old.join("config.json");
    if !config.exists() && old_config.exists() {
        std::fs::copy(old_config, config)?;
    }
    Ok(())
}

fn migrate_database(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<()> {
    if dest.exists() {
        return Ok(());
    }
    let tmp = dest.with_extension("db.migrating");
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        let from =
            rusqlite::Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(std::io::Error::other)?;
        from.backup(rusqlite::DatabaseName::Main, &tmp, None)
            .map_err(std::io::Error::other)?;
        if dest.exists() {
            return Ok(());
        }
        std::fs::rename(&tmp, dest)
    })();
    if result.is_err() || dest.exists() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// 数据库文件路径。
pub fn db_path() -> std::io::Result<PathBuf> {
    Ok(data_dir()?.join("aicouncil.db"))
}

impl AppConfig {
    pub fn load_or_default() -> std::io::Result<Self> {
        let path = data_dir()?.join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).map_err(std::io::Error::other),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                general: GeneralSettings::with_defaults(),
                ..Default::default()
            }),
            Err(e) => Err(e),
        }
    }

    /// 原子写:先写临时文件再替换,失败不破坏已有文件。
    pub fn save(&self) -> std::io::Result<()> {
        let path = data_dir()?.join("config.json");
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_string_pretty(self).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_via_temp_dir() {
        // 不动真实数据目录:序列化/反序列化往返即可覆盖格式稳定性
        let mut cfg = AppConfig {
            general: GeneralSettings::with_defaults(),
            ..Default::default()
        };
        cfg.general.max_concurrent_requests = 6;
        let s = serde_json::to_string(&cfg).unwrap();
        let back: AppConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.general.max_concurrent_requests, 6);
    }

    #[test]
    fn unknown_fields_survive_default() {
        let s = r#"{"general": {"max_concurrent_requests": 9}, "future_field": 1}"#;
        let cfg: AppConfig = serde_json::from_str(s).unwrap();
        assert_eq!(cfg.general.max_concurrent_requests, 9);
    }

    #[test]
    fn migration_uses_consistent_wal_snapshot_and_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("old.db");
        let dst_path = dir.path().join("new.db");
        let src = rusqlite::Connection::open(&src_path).unwrap();
        src.pragma_update(None, "journal_mode", "WAL").unwrap();
        src.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        src.execute_batch(
            "CREATE TABLE records(value TEXT); INSERT INTO records VALUES ('wal-only-row');",
        )
        .unwrap();
        assert!(dir.path().join("old.db-wal").exists());
        migrate_database(&src_path, &dst_path).unwrap();
        let copied: String = rusqlite::Connection::open(&dst_path)
            .unwrap()
            .query_row("SELECT value FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(copied, "wal-only-row");
        let source_count: i64 = src
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(source_count, 1);
    }

    #[test]
    fn migration_never_overwrites_existing_local_database() {
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("old.db");
        let dst_path = dir.path().join("new.db");
        rusqlite::Connection::open(&src_path)
            .unwrap()
            .execute_batch("CREATE TABLE source_only(value TEXT);")
            .unwrap();
        rusqlite::Connection::open(&dst_path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE local_only(value TEXT); INSERT INTO local_only VALUES ('keep');",
            )
            .unwrap();
        migrate_database(&src_path, &dst_path).unwrap();
        let local: String = rusqlite::Connection::open(&dst_path)
            .unwrap()
            .query_row("SELECT value FROM local_only", [], |r| r.get(0))
            .unwrap();
        assert_eq!(local, "keep");
    }

    #[test]
    fn data_root_is_next_to_executable_and_independent_of_cwd() {
        let exe = if cfg!(windows) {
            std::path::Path::new("C:\\portable\\AiCouncil\\aicouncil.exe")
        } else {
            std::path::Path::new("/opt/AiCouncil/aicouncil")
        };
        assert_eq!(
            portable_data_dir(exe).unwrap(),
            exe.parent().unwrap().join("data")
        );
    }

    #[test]
    fn attachment_roundtrip_messages_and_drafts() {
        use crate::domain::{
            BatchId, Conversation, ConversationId, Generation, GenerationId, Message, MessageId,
        };
        let db = crate::storage::open_in_memory().unwrap();
        let conv = Conversation::new(ConversationId::new(), chrono::Utc::now());
        db.upsert_conversation(&conv).unwrap();

        let att = crate::domain::Attachment {
            name: "a.txt".into(),
            content: "附件正文".into(),
        };
        let msg = Message {
            id: MessageId::new(),
            conversation_id: conv.id,
            parent_id: None,
            role: crate::domain::Role::User,
            content: "带附件".into(),
            attachments: vec![att.clone()],
            created_at: chrono::Utc::now(),
        };
        db.insert_message(&msg).unwrap();
        let loaded = db.messages_of(conv.id).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].attachments, vec![att.clone()]);

        // save_send_batch 路径:user 消息带附件
        let user = Message {
            id: MessageId::new(),
            conversation_id: conv.id,
            parent_id: None,
            role: crate::domain::Role::User,
            content: "批次".into(),
            attachments: vec![att.clone()],
            created_at: chrono::Utc::now(),
        };
        let assistant = Message {
            id: MessageId::new(),
            conversation_id: conv.id,
            parent_id: Some(user.id),
            role: crate::domain::Role::Assistant,
            content: String::new(),
            attachments: Vec::new(),
            created_at: chrono::Utc::now(),
        };
        let request_id = GenerationId::new();
        let gen = Generation {
            request_id,
            batch_id: BatchId::new(),
            user_message_id: user.id,
            assistant_message_id: assistant.id,
            provider_id: crate::domain::ProviderId::new(),
            model_id: "m".into(),
            status: crate::domain::GenerationStatus::Queued,
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
            started_at: None,
            finished_at: None,
        };
        let batch_id = gen.batch_id;
        db.save_send_batch(&crate::storage::SendBatchRecord {
            batch_id,
            conversation_id: conv.id,
            primary_request_id: gen.request_id,
            user_messages: vec![user],
            assistant_messages: vec![assistant],
            generations: vec![gen],
            usage: vec![],
            intended_main_leaf: None,
            replacement: None,
            draft_consumption: None,
        })
        .unwrap();
        let msgs = db.messages_of(conv.id).unwrap();
        let batch_user = msgs.iter().find(|m| m.content == "批次").unwrap();
        assert_eq!(batch_user.attachments.len(), 1);
        assert_eq!(batch_user.attachments[0].name, "a.txt");

        // 会话草稿附件 roundtrip
        let mut c = conv;
        c.draft_attachments = vec![att];
        db.upsert_conversation(&c).unwrap();
        let back = db.conversations().unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].draft_attachments.len(), 1);
        assert_eq!(back[0].draft_attachments[0].name, "a.txt");
    }

    #[test]
    fn removed_message_is_hidden_and_visible_descendants_are_reparented() {
        use crate::domain::{Conversation, ConversationId, Message, MessageId, Role};
        let db = crate::storage::open_in_memory().unwrap();
        let conv = Conversation::new(ConversationId::new(), chrono::Utc::now());
        db.upsert_conversation(&conv).unwrap();
        let root = MessageId::new();
        let removed = MessageId::new();
        let leaf = MessageId::new();
        for (id, parent, role, content) in [
            (root, None, Role::User, "first"),
            (removed, Some(root), Role::Assistant, "removed answer"),
            (leaf, Some(removed), Role::User, "followup"),
        ] {
            db.insert_message(&Message {
                id,
                conversation_id: conv.id,
                parent_id: parent,
                role,
                content: content.into(),
                attachments: Vec::new(),
                created_at: chrono::Utc::now(),
            })
            .unwrap();
        }
        db.remove_message(removed).unwrap();
        let visible = db.messages_of(conv.id).unwrap();
        assert_eq!(visible.len(), 2);
        assert!(!visible.iter().any(|m| m.content == "removed answer"));
        assert_eq!(
            visible.iter().find(|m| m.id == leaf).unwrap().parent_id,
            Some(root)
        );
        db.set_conversation_main_leaf(conv.id, Some(leaf)).unwrap();
        db.remove_message(root).unwrap();
        db.remove_message(leaf).unwrap();
        assert_eq!(db.main_leaf_of(conv.id).unwrap(), None);
    }

    #[test]
    fn migration_0002_upgrades_old_database() {
        use crate::domain::ConversationId;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.db");
        // 仅应用 0001:模拟 0.1.1 旧库(0001 已登记在 schema_migrations)
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(include_str!("../../migrations/0001_init.sql"))
                .unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    name TEXT PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                );
                INSERT INTO schema_migrations(name, applied_at) VALUES ('0001_init', 0);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO conversations(id, title, system_prompt, draft, created_at, updated_at)
                 VALUES ('00000000-0000-4000-8000-0000000000c1','','','', 1, 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages(id, conversation_id, role, content, created_at)
                 VALUES ('00000000-0000-4000-8000-0000000000m1',
                         '00000000-0000-4000-8000-0000000000c1','user','旧消息', 1)",
                [],
            )
            .unwrap();
        }
        // storage::open 自动补 0002 迁移
        let db = crate::storage::open(&path).unwrap();
        let conv = ConversationId::from_uuid_str("00000000-0000-4000-8000-0000000000c1");
        let msgs = db.messages_of(conv).unwrap();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].attachments.is_empty(), "旧消息附件为空");
        let convs = db.conversations().unwrap();
        assert_eq!(convs.len(), 1);
        assert!(convs[0].draft_attachments.is_empty());
        // 迁移后可正常写入并读回附件
        let mut c = convs[0].clone();
        c.draft_attachments = vec![crate::domain::Attachment {
            name: "n.txt".into(),
            content: "x".into(),
        }];
        db.upsert_conversation(&c).unwrap();
        assert_eq!(db.conversations().unwrap()[0].draft_attachments.len(), 1);
    }
}
