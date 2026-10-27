//! 平台数据目录(macOS 优先)与非敏感配置。
//!
//! 数据源边界(PLAN 3.1):配置文件保存提供方非敏感连接配置与通用设置;
//! 数据库保存会话/偏好/目录缓存;系统凭据保存 Key。同一字段不双写。
//!
//! macOS 数据位置契约(0.2.0 起):
//! - `AICOUNCIL_DATA_DIR` 设置时:使用该目录(测试/验收隔离;不做旧数据导入)。
//! - .app 包内运行(…/Foo.app/Contents/MacOS/exe):稳定目录
//!   `~/Library/Application Support/AiCouncil`(与包位置无关,升级/搬家不变)。
//!   首次启动从包同级 `data/`(如 `dist/data`)WAL 安全导入旧库与 config,
//!   不覆盖已有目标文件,幂等;绝不导入/解密 Windows DPAPI 凭据文件。
//! - 非包内运行(cargo dev/test):exe 同级 `data/`(隔离于真实用户数据)。

use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
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

/// 运行时数据目录。见模块文档的位置契约。
pub fn data_dir() -> std::io::Result<PathBuf> {
    if let Some(test_dir) = std::env::var_os("AICOUNCIL_DATA_DIR") {
        let dir = PathBuf::from(test_dir);
        std::fs::create_dir_all(&dir)?;
        return Ok(dir);
    }
    let exe = std::env::current_exe()?;
    let dir = default_data_dir(&exe)?;
    std::fs::create_dir_all(&dir)?;
    if cfg!(target_os = "macos") {
        import_legacy_data(&exe, &dir)?;
    }
    Ok(dir)
}

/// .app 包根目录(…/Foo.app/Contents/MacOS/exe → …/Foo.app);非包内运行返回 None。
fn bundled_app_root(exe: &Path) -> Option<PathBuf> {
    // ancestors(): [exe, MacOS, Contents, Foo.app, …];包根在第 3 层
    let root = exe.ancestors().nth(3)?;
    if root.extension() == Some(std::ffi::OsStr::new("app")) {
        Some(root.to_path_buf())
    } else {
        None
    }
}

/// 包内 macOS 运行的稳定数据目录(与包位置无关):Application Support/AiCouncil。
fn application_support_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "AiCouncil").map(|p| p.data_dir().to_path_buf())
}

/// 默认数据目录(未设置 AICOUNCIL_DATA_DIR 时):包内 → Application Support;
/// 否则 exe 同级 data(dev/test 与真实数据隔离)。
fn default_data_dir(exe: &Path) -> std::io::Result<PathBuf> {
    if cfg!(target_os = "macos") {
        if let (Some(bundle), Some(dir)) = (bundled_app_root(exe), application_support_dir()) {
            let _ = bundle; // 仅判定“包内运行”;目录本身稳定,不随包路径变化
            return Ok(dir);
        }
    }
    portable_data_dir(exe)
}

fn portable_data_dir(exe: &Path) -> std::io::Result<PathBuf> {
    Ok(exe
        .parent()
        .ok_or_else(|| std::io::Error::other("无法确定程序目录"))?
        .join("data"))
}

/// `.env` 候选路径(与 desktop env_keys 的定位顺序一致,供凭据状态等使用):
/// `AICOUNCIL_ENV_PATH`(显式) > 稳定数据目录 `.env` > exe 同级 `.env`。
/// `AICOUNCIL_DATA_DIR` 设置时严格隔离:只有 [AICOUNCIL_ENV_PATH?, 数据目录/.env],
/// 绝不回退到 exe/CWD 等真实文件路径(测试不得触达私有文件)。
/// 选定的**第一个存在的文件**即唯一权威;键为空/缺失时就是“未配置”,
/// 不再尝试其他文件(与 desktop 语义一致)。
pub fn env_file_candidates() -> std::io::Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    if let Ok(explicit) = std::env::var("AICOUNCIL_ENV_PATH") {
        if !explicit.is_empty() {
            candidates.push(PathBuf::from(explicit));
        }
    }
    candidates.push(data_dir()?.join(".env"));
    if std::env::var_os("AICOUNCIL_DATA_DIR").is_none() {
        if let Ok(parent) = std::env::current_exe().map(|exe| {
            exe.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(PathBuf::new)
        }) {
            candidates.push(parent.join(".env"));
        }
    }
    candidates.dedup();
    Ok(candidates)
}

/// 首次启动旧数据导入(仅包内 macOS;隔离目录/AICOUNCIL_DATA_DIR 下不执行)。
/// 来源:包同级 `data/`(PC 0.1.10 便携目录,如 `dist/data`)。
/// 非覆盖、幂等;只导入 aicouncil.db 与 config.json;绝不触碰 credentials.dpapi。
#[doc(hidden)]
pub fn import_legacy_data(exe: &Path, dest_dir: &Path) -> std::io::Result<()> {
    // 隔离由 `data_dir()` 调用点保证(AICOUNCIL_DATA_DIR 命中时根本不会走到
    // 这里的导入分支);此处不再重复判定环境变量,避免同进程内并发测试互相
    // 干扰导致导入被静默跳过。
    if std::env::var_os("AICOUNCIL_DEMO_MODE").is_some() {
        return Ok(());
    }
    let Some(bundle) = bundled_app_root(exe) else {
        return Ok(());
    };
    let Some(src_dir) = bundle.parent().map(|p| p.join("data")) else {
        return Ok(());
    };
    let dest_db = dest_dir.join("aicouncil.db");
    let src_db = src_dir.join("aicouncil.db");
    if !dest_db.exists() && src_db.exists() {
        import_database_snapshot(&src_db, &dest_db)?;
    }
    copy_if_absent(&src_dir.join("config.json"), &dest_dir.join("config.json"))?;
    Ok(())
}

fn copy_if_absent(src: &Path, dest: &Path) -> std::io::Result<()> {
    if dest.exists() || !src.exists() {
        return Ok(());
    }
    let tmp = dest.with_extension("json.importing");
    std::fs::copy(src, &tmp)?;
    if dest.exists() {
        let _ = std::fs::remove_file(&tmp);
        return Ok(());
    }
    std::fs::rename(&tmp, dest)
}

/// WAL 安全的旧库导入:先旁路复制源(含 -wal/-shm/-journal),副本上做恢复与
/// backup 快照,quick_check 通过后原子改名落位。
/// 非覆盖(目标已存在直接跳过)、幂等;源库字节不被修改(恢复只发生在副本上)。
#[doc(hidden)]
pub fn import_database_snapshot(src: &Path, dest: &Path) -> std::io::Result<()> {
    if dest.exists() {
        return Ok(());
    }
    let staging = dest.with_extension("db.importing");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = (|| {
        let staged_db = staging.join("source.db");
        for suffix in ["", "-wal", "-shm", "-journal"] {
            let side = sidecar_path(src, suffix);
            if side.exists() {
                std::fs::copy(&side, staging.join(format!("source.db{suffix}")))?;
            }
        }
        // 打开副本(可写):WAL 恢复仅发生在副本上,源库保持字节不变。
        let from = rusqlite::Connection::open(&staged_db).map_err(std::io::Error::other)?;
        let check: String = from
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(std::io::Error::other)?;
        if check != "ok" {
            return Err(std::io::Error::other(format!("源库 quick_check: {check}")));
        }
        let tmp_dest = dest.with_extension("db.migrating");
        let _ = std::fs::remove_file(&tmp_dest);
        from.backup(rusqlite::DatabaseName::Main, &tmp_dest, None)
            .map_err(std::io::Error::other)?;
        let verify = rusqlite::Connection::open(&tmp_dest).map_err(std::io::Error::other)?;
        let verify_check: String = verify
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(std::io::Error::other)?;
        if verify_check != "ok" {
            return Err(std::io::Error::other(format!(
                "导入副本 quick_check: {verify_check}"
            )));
        }
        let fk_violation: Option<String> = verify
            .query_row("PRAGMA foreign_key_check", [], |r| r.get(0))
            .optional()
            .map_err(std::io::Error::other)?;
        if fk_violation.is_some() {
            return Err(std::io::Error::other("导入副本 foreign_key_check 非空"));
        }
        drop(verify);
        drop(from);
        if dest.exists() {
            // 并发启动已有目标:非覆盖,丢弃本次快照。
            let _ = std::fs::remove_file(&tmp_dest);
            return Ok(());
        }
        std::fs::rename(&tmp_dest, dest)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    if result.is_err() || dest.exists() {
        let _ = std::fs::remove_file(dest.with_extension("db.migrating"));
    }
    result
}

fn sidecar_path(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(suffix);
    db.with_file_name(name)
}

/// 测试专用:串行化进程级 `AICOUNCIL_DATA_DIR` 环境变量的用例,
/// 避免同二进制内并行测试互相干扰。
#[cfg(test)]
pub(crate) mod test_sync {
    use std::sync::Mutex;
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());
}

/// 测试专用入口(集成测试使用;不属于公共 API 面)。
#[doc(hidden)]
pub mod repo_test_hooks {
    pub use super::{import_database_snapshot, import_legacy_data};
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

    fn write_db(path: &Path, sql: &str) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(sql).unwrap();
        conn
    }

    #[test]
    fn import_uses_consistent_wal_snapshot_and_preserves_source() {
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("old.db");
        let dst_path = dir.path().join("new.db");
        let src = write_db(
            &src_path,
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE records(value TEXT); INSERT INTO records VALUES ('wal-only-row');",
        );
        assert!(dir.path().join("old.db-wal").exists());
        let before = std::fs::read(&src_path).unwrap();
        import_database_snapshot(&src_path, &dst_path).unwrap();
        let after = std::fs::read(&src_path).unwrap();
        assert_eq!(before, after, "源库字节不得被导入修改");
        let copied: String = rusqlite::Connection::open(&dst_path)
            .unwrap()
            .query_row("SELECT value FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(copied, "wal-only-row");
        let source_count: i64 = src
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(source_count, 1);
        assert!(
            !dir.path().join("new.db.importing").exists(),
            "staging 清理"
        );
    }

    #[test]
    fn import_never_overwrites_existing_local_database() {
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("old.db");
        let dst_path = dir.path().join("new.db");
        write_db(&src_path, "CREATE TABLE source_only(value TEXT);");
        let local = write_db(
            &dst_path,
            "CREATE TABLE local_only(value TEXT); INSERT INTO local_only VALUES ('keep');",
        );
        let before = std::fs::read(&dst_path).unwrap();
        import_database_snapshot(&src_path, &dst_path).unwrap();
        import_database_snapshot(&src_path, &dst_path).unwrap(); // 幂等
        let after = std::fs::read(&dst_path).unwrap();
        assert_eq!(before, after, "已有目标库不得被改动");
        let local_value: String = local
            .query_row("SELECT value FROM local_only", [], |r| r.get(0))
            .unwrap();
        assert_eq!(local_value, "keep");
    }

    #[test]
    fn import_rejects_corrupt_source_without_creating_target() {
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("old.db");
        std::fs::write(&src_path, b"not a sqlite database at all").unwrap();
        let dst_path = dir.path().join("new.db");
        let err = import_database_snapshot(&src_path, &dst_path).unwrap_err();
        assert!(err.to_string().contains("quick_check") || err.to_string().contains("file"));
        assert!(!dst_path.exists());
        assert!(!dst_path.with_extension("db.migrating").exists());
        assert!(!dst_path.with_extension("db.importing").exists());
    }

    #[test]
    fn bundled_detection_and_default_dirs() {
        let app_exe = Path::new("/opt/AiCouncil.app/Contents/MacOS/aicouncil");
        assert_eq!(
            bundled_app_root(app_exe).unwrap(),
            Path::new("/opt/AiCouncil.app")
        );
        let dist_exe = Path::new("/Users/x/proj/dist/AiCouncil.app/Contents/MacOS/aicouncil");
        assert_eq!(
            bundled_app_root(dist_exe).unwrap(),
            Path::new("/Users/x/proj/dist/AiCouncil.app")
        );
        let plain = if cfg!(windows) {
            Path::new("C:\\portable\\AiCouncil\\aicouncil.exe")
        } else {
            Path::new("/opt/AiCouncil/aicouncil")
        };
        assert!(bundled_app_root(plain).is_none());
        assert_eq!(
            portable_data_dir(plain).unwrap(),
            plain.parent().unwrap().join("data")
        );
    }

    #[test]
    fn legacy_import_copies_db_and_config_only_once_without_dpapi() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("AiCouncil.app/Contents/MacOS");
        std::fs::create_dir_all(&bundle).unwrap();
        let exe = bundle.join("aicouncil");
        let legacy = dir.path().join("data");
        std::fs::create_dir_all(&legacy).unwrap();
        write_db(
            &legacy.join("aicouncil.db"),
            "CREATE TABLE t(x); INSERT INTO t VALUES (1);",
        );
        std::fs::write(legacy.join("config.json"), "{\"version\":0}").unwrap();
        std::fs::write(legacy.join("credentials.dpapi"), b"cipher").unwrap();
        let dest = dir.path().join("support/AiCouncil");
        std::fs::create_dir_all(&dest).unwrap();

        import_legacy_data(&exe, &dest).unwrap();
        import_legacy_data(&exe, &dest).unwrap(); // 幂等
        assert!(dest.join("aicouncil.db").exists());
        let cfg: String = std::fs::read_to_string(dest.join("config.json")).unwrap();
        assert_eq!(cfg, "{\"version\":0}");
        // DPAPI 凭据文件是 Windows 密文,Mac 端不导入、不解密、不复制。
        assert!(!dest.join("credentials.dpapi").exists());
        assert_eq!(
            std::fs::read(legacy.join("credentials.dpapi")).unwrap(),
            b"cipher"
        );
        // 已有 config 不得被覆盖
        std::fs::write(dest.join("config.json"), "{\"version\":1}").unwrap();
        std::fs::write(legacy.join("config.json"), "{\"version\":0}").unwrap();
        import_legacy_data(&exe, &dest).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("config.json")).unwrap(),
            "{\"version\":1}"
        );
    }

    #[test]
    fn env_candidates_prefer_data_dir() {
        let _guard = test_sync::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("AICOUNCIL_DATA_DIR", dir.path());
        let candidates = env_file_candidates().unwrap();
        assert_eq!(candidates[0], dir.path().join(".env"));
        std::env::remove_var("AICOUNCIL_DATA_DIR");
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
