//! macOS 数据迁移集成测试(全部使用临时目录与合成数据,绝不触碰真实
//! `dist/data` 或用户数据)。
//!
//! 覆盖:WAL 安全导入、源库字节不变、非覆盖、幂等、损坏源拒绝、
//! 完整 schema 迁移链(0001–0006)行数/内容等价、foreign_key_check。
//!
//! 隔离方式:所有用例直接调用 `repo::import_database_snapshot` /
//! `import_legacy_data` 的测试专用包装;不依赖真实 exe 路径与 HOME。

use std::path::Path;
use std::sync::Mutex;

use aicouncil::storage::repo::repo_test_hooks as hooks;
use rusqlite::OptionalExtension;

/// 进程级串行:SQLite 文件操作较多,避免并发 FS 抖动;
/// 锁中毒时继续(单测试失败不应级联整个套件)。
static IO_LOCK: Mutex<()> = Mutex::new(());

fn io_lock() -> std::sync::MutexGuard<'static, ()> {
    IO_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn open_db(path: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(path).unwrap()
}

/// 构造"0.1.10 全迁移链"合成旧库:应用 0001–0006 并插入基线规模的假数据
/// (3 会话/18 消息/2 分组;内容为无敏感意义的合成文本)。
fn build_legacy_db(path: &Path, conversations: usize, messages_per_conv: usize) {
    let conn = open_db(path);
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            name TEXT PRIMARY KEY,
            applied_at INTEGER NOT NULL
        );",
    )
    .unwrap();
    for sql in [
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_attachments.sql"),
        include_str!("../migrations/0003_title_metadata.sql"),
        include_str!("../migrations/0004_request_recovery.sql"),
        include_str!("../migrations/0005_message_removal.sql"),
        include_str!("../migrations/0006_composer_drafts.sql"),
    ] {
        conn.execute_batch(sql).unwrap();
    }
    conn.execute_batch(
        "INSERT INTO schema_migrations(name, applied_at) VALUES
            ('0001_init',0),('0002_attachments',0),('0003_title_metadata',0),
            ('0004_request_recovery',0),('0005_message_removal',0),('0006_composer_drafts',0);",
    )
    .unwrap();
    for c in 0..conversations {
        let cid = format!("00000000-0000-4000-8000-{c:012x}");
        conn.execute(
            "INSERT INTO conversations(id, title, system_prompt, draft, created_at, updated_at)
             VALUES (?1, ?2, '', '', 1, 1)",
            rusqlite::params![cid, format!("合成会话 {c}")],
        )
        .unwrap();
        for m in 0..messages_per_conv {
            let mid = format!("10000000-0000-4000-8000-{c:08x}{m:04x}");
            let role = if m % 2 == 0 { "user" } else { "assistant" };
            conn.execute(
                "INSERT INTO messages(id, conversation_id, role, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                rusqlite::params![mid, cid, role, format!("合成消息 {c}-{m}")],
            )
            .unwrap();
        }
    }
}

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// 内容聚合指纹(不打印内容本身):行数与拼接内容的哈希等价性。
fn content_fingerprint(path: &Path) -> (i64, u64) {
    let conn = open_db(path);
    let rows: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT content FROM messages ORDER BY id")
            .unwrap();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        rows.map(|r| r.unwrap()).collect()
    };
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for row in &rows {
        row.hash(&mut hasher);
    }
    (rows.len() as i64, hasher.finish())
}

#[test]
fn wal_import_preserves_rows_and_source_bytes() {
    let _g = io_lock();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("legacy.db");
    let dst = dir.path().join("support").join("aicouncil.db");
    std::fs::create_dir_all(dst.parent().unwrap()).unwrap();

    // WAL 场景:未 checkpoint 的行只存在于 -wal 中
    {
        let conn = open_db(&src);
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        build_legacy_db(&src, 2, 2);
        conn.execute_batch(
            "INSERT INTO messages(id, conversation_id, role, content, created_at)
             VALUES ('20000000-0000-4000-8000-000000000001',
                     '00000000-0000-4000-8000-000000000000',
                     'user', '仅存在于 WAL 的行', 1)",
        )
        .unwrap();
        assert!(dir.path().join("legacy.db-wal").exists());
    }
    let src_bytes_before = std::fs::read(&src).unwrap();

    hooks::import_database_snapshot(&src, &dst).unwrap();

    assert_eq!(
        std::fs::read(&src).unwrap(),
        src_bytes_before,
        "源库字节必须保持不变"
    );
    let imported = open_db(&dst);
    assert_eq!(
        imported
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    let fk: Option<String> = imported
        .query_row("PRAGMA foreign_key_check", [], |r| r.get(0))
        .optional()
        .unwrap();
    assert!(fk.is_none(), "foreign_key_check 必须为空");
    assert_eq!(count(&imported, "SELECT COUNT(*) FROM conversations"), 2);
    assert_eq!(count(&imported, "SELECT COUNT(*) FROM messages"), 5);
    let wal_row: i64 = imported
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE content = '仅存在于 WAL 的行'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(wal_row, 1, "未 checkpoint 的 WAL 行必须保留");
}

#[test]
fn import_is_idempotent_and_never_overwrites() {
    let _g = io_lock();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("legacy.db");
    build_legacy_db(&src, 1, 4);
    let dst = dir.path().join("aicouncil.db");

    hooks::import_database_snapshot(&src, &dst).unwrap();
    let first = content_fingerprint(&dst);
    let dst_bytes = std::fs::read(&dst).unwrap();

    // 第二次启动/导入:不重复、不覆盖
    hooks::import_database_snapshot(&src, &dst).unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), dst_bytes);
    assert_eq!(content_fingerprint(&dst), first);

    // 已有本地新库(含本地专属行)绝不被旧库覆盖
    let dir2 = tempfile::tempdir().unwrap();
    let src2 = dir2.path().join("legacy.db");
    build_legacy_db(&src2, 1, 2);
    let dst2 = dir2.path().join("aicouncil.db");
    {
        let conn = open_db(&dst2);
        conn.execute_batch(
            "CREATE TABLE local_only(x TEXT); INSERT INTO local_only VALUES ('keep');",
        )
        .unwrap();
    }
    hooks::import_database_snapshot(&src2, &dst2).unwrap();
    let local = open_db(&dst2);
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM local_only"),
        1,
        "本地新库必须原样保留"
    );
    // 旧库不得并入已存在的本地库:conversations 表根本不应被创建
    let conv_table: i64 = local
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='conversations'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(conv_table, 0, "已存在的本地库不得被旧库并入");
}

#[test]
fn corrupt_source_rejected_without_target() {
    let _g = io_lock();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("broken.db");
    std::fs::write(&src, b"definitely not sqlite").unwrap();
    let dst = dir.path().join("aicouncil.db");
    let err = hooks::import_database_snapshot(&src, &dst).unwrap_err();
    assert!(!dst.exists(), "失败时不得产生目标库");
    assert!(!dst.with_extension("db.migrating").exists());
    assert!(!dst.with_extension("db.importing").exists());
    let _ = err; // 错误信息已在单元测试断言;此处仅要求返回 Err
}

#[test]
fn bundled_app_imports_sibling_data_idempotently() {
    let _g = io_lock();
    let dir = tempfile::tempdir().unwrap();
    // 模拟 dist 布局:dist/AiCouncil.app/Contents/MacOS/aicouncil + dist/data
    let macos = dir.path().join("dist/AiCouncil.app/Contents/MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    let exe = macos.join("aicouncil");
    let legacy = dir.path().join("dist/data");
    std::fs::create_dir_all(&legacy).unwrap();
    build_legacy_db(&legacy.join("aicouncil.db"), 3, 6);
    std::fs::write(
        legacy.join("config.json"),
        r#"{"version":0,"general":{"locale":"en"}}"#,
    )
    .unwrap();
    std::fs::write(legacy.join("credentials.dpapi"), b"windows cipher").unwrap();

    let dest = dir.path().join("support/AiCouncil");
    std::fs::create_dir_all(&dest).unwrap();

    hooks::import_legacy_data(&exe, &dest).unwrap();
    hooks::import_legacy_data(&exe, &dest).unwrap(); // 第二次启动:幂等

    let db = open_db(&dest.join("aicouncil.db"));
    assert_eq!(count(&db, "SELECT COUNT(*) FROM conversations"), 3);
    assert_eq!(count(&db, "SELECT COUNT(*) FROM messages"), 18);
    let (rows, _) = content_fingerprint(&dest.join("aicouncil.db"));
    assert_eq!(rows, 18);
    let cfg = std::fs::read_to_string(dest.join("config.json")).unwrap();
    assert!(cfg.contains("\"locale\""));
    // Windows DPAPI 密文:不导入、不解密、原文件不动
    assert!(!dest.join("credentials.dpapi").exists());
    assert_eq!(
        std::fs::read(legacy.join("credentials.dpapi")).unwrap(),
        b"windows cipher"
    );
}

#[test]
fn full_migration_chain_baseline_equivalence() {
    let _g = io_lock();
    // 基线规模(与 PC 备份记录的规模一致,内容为合成数据):
    // 3 会话 / 18 消息,经完整 0001–0006 迁移链 + 导入后完全等价。
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("legacy.db");
    build_legacy_db(&src, 3, 6);
    let fingerprint_before = content_fingerprint(&src);

    let dst = dir.path().join("aicouncil.db");
    hooks::import_database_snapshot(&src, &dst).unwrap();
    assert_eq!(content_fingerprint(&dst), fingerprint_before);

    // 导入产物可直接被 storage::open 打开(补齐缺省迁移不改动已应用项)
    let db = aicouncil::storage::open(&dst).unwrap();
    let convs = db.conversations().unwrap();
    assert_eq!(convs.len(), 3);
    for conv in &convs {
        assert!(conv.title.starts_with("合成会话"));
    }
    assert_eq!(count(&open_db(&dst), "SELECT COUNT(*) FROM messages"), 18);
    assert_eq!(
        count(&open_db(&dst), "SELECT COUNT(*) FROM schema_migrations"),
        6
    );
}
