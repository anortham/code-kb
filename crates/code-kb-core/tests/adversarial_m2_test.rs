use code_kb_core::db::checkpoint_truncate;
use code_kb_core::{open_read_only, open_read_write, safe_tempdir};
use std::fs;

#[test]
fn test_adversarial_mmap_size_zero_on_windows_allows_concurrent_writes() {
    let temp_dir = safe_tempdir();
    let db_path = temp_dir.path().join("mmap_stress.db");
    let conn_rw = open_read_write(&db_path).unwrap();
    conn_rw
        .execute(
            "CREATE TABLE stress (id INTEGER PRIMARY KEY, msg TEXT);",
            [],
        )
        .unwrap();
    conn_rw
        .execute("INSERT INTO stress (msg) VALUES ('init');", [])
        .unwrap();
    checkpoint_truncate(&conn_rw).unwrap();
    let conn_ro = open_read_only(&db_path).unwrap();
    let mmap_size: i64 = conn_ro
        .query_row("PRAGMA mmap_size;", [], |r| r.get(0))
        .unwrap();
    #[cfg(windows)]
    assert_eq!(mmap_size, 0);
    #[cfg(not(windows))]
    assert_eq!(mmap_size, 268435456);
    for i in 0..500 {
        conn_rw
            .execute(
                "INSERT INTO stress (msg) VALUES (?1);",
                [format!("row_{i}")],
            )
            .unwrap();
    }
    checkpoint_truncate(&conn_rw).unwrap();
    let count: i64 = conn_ro
        .query_row("SELECT count(*) FROM stress;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 501);
}

#[test]
fn test_adversarial_checkpoint_truncate_truncates_wal_file_to_zero_bytes() {
    let temp_dir = safe_tempdir();
    let db_path = temp_dir.path().join("wal_truncate_test.db");
    let wal_path = temp_dir.path().join("wal_truncate_test.db-wal");
    let conn_rw = open_read_write(&db_path).unwrap();
    conn_rw
        .execute(
            "CREATE TABLE audit_log (id INTEGER PRIMARY KEY, entry TEXT);",
            [],
        )
        .unwrap();
    for i in 0..500 {
        conn_rw
            .execute(
                "INSERT INTO audit_log (entry) VALUES (?1);",
                [format!("audit_entry_{i}")],
            )
            .unwrap();
    }
    assert!(wal_path.exists());
    assert!(fs::metadata(&wal_path).unwrap().len() > 0);
    checkpoint_truncate(&conn_rw).unwrap();
    assert_eq!(fs::metadata(&wal_path).unwrap().len(), 0);
    drop(conn_rw);
    let conn_ro = open_read_only(&db_path).unwrap();
    let count: i64 = conn_ro
        .query_row("SELECT count(*) FROM audit_log;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 500);
}

#[test]
fn test_adversarial_worktree_db_copy_contains_all_flushed_transactions() {
    let temp_dir = safe_tempdir();
    let parent_dir = temp_dir.path().join("parent/.code-kb");
    fs::create_dir_all(&parent_dir).unwrap();
    let parent_db = parent_dir.join("artifact.db");
    let conn_rw = open_read_write(&parent_db).unwrap();
    conn_rw
        .execute("CREATE TABLE symbols (name TEXT, file_path TEXT);", [])
        .unwrap();
    for i in 0..100 {
        conn_rw
            .execute(
                "INSERT INTO symbols VALUES (?1, ?2);",
                [format!("sym_{i}"), format!("src/file_{i}.rs")],
            )
            .unwrap();
    }
    checkpoint_truncate(&conn_rw).unwrap();
    drop(conn_rw);
    let worktree_dir = temp_dir.path().join("worktree/.code-kb");
    fs::create_dir_all(&worktree_dir).unwrap();
    let worktree_db = worktree_dir.join("artifact.db");
    fs::copy(&parent_db, &worktree_db).unwrap();
    let conn_worktree = open_read_only(&worktree_db).unwrap();
    let count: i64 = conn_worktree
        .query_row("SELECT count(*) FROM symbols;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 100);
}
