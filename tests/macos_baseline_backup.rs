//! 真实 PC 备份的受控迁移自检(基线计数与完整性;不读取/不打印任何内容)。
//!
//! 由 `scripts/macos_migrate_selfcheck.sh` 调用:该脚本先把 `dist/pc-data-backup`
//! 复制到临时目录并校验哈希,再设 `AICOUNCIL_BASELINE_DIR` 指向**副本**。
//! 测试本身从不触碰真实 `dist/data`、备份原件或凭据文件;未设环境变量时跳过。
//!
//! 基线(来自 docs/TAURI_MACOS_PLAN.md,非敏感计数):
//! 3 conversations / 18 messages / 12 generations / 12 usage_records / 2 groups / 6 migrations。

use std::path::PathBuf;

fn baseline_dir() -> PathBuf {
    let Some(dir) = std::env::var_os("AICOUNCIL_BASELINE_DIR").map(PathBuf::from) else {
        eprintln!("skip: set AICOUNCIL_BASELINE_DIR (via scripts/macos_migrate_selfcheck.sh) to verify the real backup copy");
        std::process::exit(0);
    };
    if !dir.join("aicouncil.db").exists() {
        panic!("基线副本缺少 aicouncil.db: {}", dir.display());
    }
    dir
}

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn baseline_backup_counts_and_integrity() {
    let dir = baseline_dir();
    let conn = rusqlite::Connection::open_with_flags(
        dir.join("aicouncil.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM conversations"), 3);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM messages"), 18);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM generations"), 12);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM usage_records"), 12);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM groups"), 2);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM schema_migrations"), 6);
}

#[test]
fn baseline_backup_imports_with_identical_counts() {
    let dir = baseline_dir();
    let src = dir.join("aicouncil.db");
    let dest_dir = tempfile::tempdir().unwrap();
    let dest = dest_dir.path().join("aicouncil.db");

    aicouncil::storage::repo::repo_test_hooks::import_database_snapshot(&src, &dest).unwrap();

    let imported = rusqlite::Connection::open(&dest).unwrap();
    assert_eq!(
        imported
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(count(&imported, "SELECT COUNT(*) FROM conversations"), 3);
    assert_eq!(count(&imported, "SELECT COUNT(*) FROM messages"), 18);
    assert_eq!(count(&imported, "SELECT COUNT(*) FROM generations"), 12);
    assert_eq!(count(&imported, "SELECT COUNT(*) FROM usage_records"), 12);
    assert_eq!(count(&imported, "SELECT COUNT(*) FROM groups"), 2);
    assert_eq!(
        count(&imported, "SELECT COUNT(*) FROM schema_migrations"),
        6
    );

    // 幂等:二次导入不重复(行数不变)
    aicouncil::storage::repo::repo_test_hooks::import_database_snapshot(&src, &dest).unwrap();
    assert_eq!(count(&imported, "SELECT COUNT(*) FROM messages"), 18);
}
