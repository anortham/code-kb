use code_kb_core::db::{checkpoint_truncate, open_read_only, open_read_write};
use code_kb_core::edit::{EditError, replace_symbol_body};
use code_kb_core::sync::find_julie_extract_binary;
use code_kb_core::workspace::Workspace;
use code_kb_core::{safe_tempdir, scan_workspace, slicer};
use std::fs;
#[allow(unused_imports)]
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

fn setup_calc_workspace() -> (tempfile::TempDir, Workspace, std::path::PathBuf) {
    let _ = find_julie_extract_binary().expect("julie-extract binary must be available");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let initial_code = r#"pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}
"#;
    let file_path = src_dir.join("calc.rs");
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Initial scan must succeed");

    (temp_dir, ws, db_path)
}

// ----------------------------------------------------------------------------
// 1. Stress-test file locking and retry backoff in `edit.rs`
// ----------------------------------------------------------------------------

#[test]
#[cfg(windows)]
fn test_adversarial_persist_transient_sharing_violation_succeeds() {
    let (_temp_dir, ws, db_path) = setup_calc_workspace();
    let conn = open_read_only(&db_path).unwrap();
    let file_path = ws.canonical_root.join("src").join("calc.rs");

    // Spawn a background thread that holds FILE_SHARE_READ (share_mode 1: no FILE_SHARE_DELETE)
    // for 25ms. This simulates transient antivirus/indexer read locks that prevent file replacement.
    let target = file_path.clone();
    let lock_thread = std::thread::spawn(move || {
        // Sleep briefly so replace_symbol_body can perform its initial read before we lock
        std::thread::sleep(Duration::from_millis(1));
        let locked_file = fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ only: MoveFileEx / persist fails with ERROR_SHARING_VIOLATION
            .open(&target);
        if let Ok(file) = locked_file {
            std::thread::sleep(Duration::from_millis(25));
            drop(file);
        }
    });

    let new_body = r#"{
    let sum = a + b;
    sum * 10
}"#;

    // replace_symbol_body will encounter the transient lock, retry with exponential backoff (10ms, 20ms),
    // and succeed on attempt 2 once the background thread drops the lock handle.
    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "add_numbers",
        "src/calc.rs",
        new_body,
        None,
    );

    lock_thread.join().unwrap();

    assert!(
        res.is_ok(),
        "replace_symbol_body must survive transient sharing lock via retry backoff: {:?}",
        res.err()
    );

    let disk_content = fs::read_to_string(&file_path).unwrap();
    assert!(
        disk_content.contains("sum * 10"),
        "Disk content must reflect updated body"
    );

    // Verify no leftover .tmp files
    let src_dir = ws.canonical_root.join("src");
    let tmp_entries: Vec<_> = fs::read_dir(&src_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(
        tmp_entries.is_empty(),
        "No temporary files should leak in src dir: {:?}",
        tmp_entries
    );
}

#[test]
#[cfg(windows)]
fn test_adversarial_persist_permanent_sharing_violation_fails_and_cleans_up() {
    let (_temp_dir, ws, db_path) = setup_calc_workspace();
    let conn = open_read_only(&db_path).unwrap();
    let file_path = ws.canonical_root.join("src").join("calc.rs");
    let initial_content = fs::read_to_string(&file_path).unwrap();

    // Hold a permanent lock with share_mode(1) (FILE_SHARE_READ only, no delete)
    let locked_file = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&file_path)
        .expect("Open with share_mode 1 must succeed");

    let new_body = r#"{
    let sum = a + b;
    sum * 99
}"#;

    // replace_symbol_body will retry 5 times and then fail
    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "add_numbers",
        "src/calc.rs",
        new_body,
        None,
    );

    drop(locked_file);

    assert!(
        res.is_err(),
        "replace_symbol_body must fail when file is permanently locked"
    );
    match res.unwrap_err() {
        EditError::Io(path, err) => {
            assert!(path.contains("calc.rs"));
            let raw_code = err.raw_os_error().unwrap_or(0);
            assert!(
                raw_code == 32 || raw_code == 5,
                "Expected OS error 32 (Sharing Violation) or 5 (Access Denied), got {raw_code}"
            );
        }
        other => panic!("Expected EditError::Io, got: {other:?}"),
    }

    // Verify original content was NOT corrupted
    let current_content = fs::read_to_string(&file_path).unwrap();
    assert_eq!(
        current_content, initial_content,
        "Original file must remain intact after failed persist"
    );

    // Verify temp file was deleted on failure
    let src_dir = ws.canonical_root.join("src");
    let tmp_entries: Vec<_> = fs::read_dir(&src_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(
        tmp_entries.is_empty(),
        "Temporary file must be dropped and deleted on error: {:?}",
        tmp_entries
    );
}

#[test]
fn test_adversarial_rollback_on_sync_failure() {
    let (_temp_dir, ws, db_path) = setup_calc_workspace();
    let conn = open_read_only(&db_path).unwrap();
    let file_path = ws.canonical_root.join("src").join("calc.rs");
    let initial_content = fs::read_to_string(&file_path).unwrap();

    // Corrupt the database file permissions to make `sync::update_file` fail during re-indexing
    drop(conn);
    let mut perms = fs::metadata(&db_path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&db_path, perms).unwrap();

    let conn_ro = open_read_only(&db_path).unwrap();

    let new_body = r#"{
    let sum = a + b;
    sum * 7
}"#;

    // replace_symbol_body will write new_file_bytes, attempt sync::update_file which fails
    // because db is read-only, then execute rollback atomically and return SyncWithRollback.
    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn_ro,
        "add_numbers",
        "src/calc.rs",
        new_body,
        None,
    );

    // Restore permissions for cleanup
    let mut perms = fs::metadata(&db_path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    let _ = fs::set_permissions(&db_path, perms);

    assert!(res.is_err(), "Must fail due to read-only database");
    match res.unwrap_err() {
        EditError::SyncWithRollback(msg) => {
            assert!(
                !msg.is_empty(),
                "SyncWithRollback message should describe sync error"
            );
        }
        other => panic!("Expected EditError::SyncWithRollback, got: {other:?}"),
    }

    // Crucial check: Verify the file on disk was ROLLED BACK to its exact initial content!
    let content_after_rollback = fs::read_to_string(&file_path).unwrap();
    assert_eq!(
        content_after_rollback, initial_content,
        "Disk file must be restored to initial bytes after sync failure"
    );
}

#[test]
#[cfg(windows)]
fn test_adversarial_rollback_fails_cleanly_when_dest_locked() {
    let (_temp_dir, ws, db_path) = setup_calc_workspace();
    let conn = open_read_only(&db_path).unwrap();
    let file_path = ws.canonical_root.join("src").join("calc.rs");

    // Make database read-only so sync fails
    drop(conn);
    let mut perms = fs::metadata(&db_path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&db_path, perms).unwrap();

    let conn_ro = open_read_only(&db_path).unwrap();

    let target = file_path.clone();
    let initial_bytes = fs::read(&file_path).unwrap();
    let lock_thread = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        while fs::read(&target).ok().as_deref() == Some(initial_bytes.as_slice())
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(1));
        }
        let locked_file = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&target);
        if let Ok(file) = locked_file {
            std::thread::sleep(Duration::from_millis(500));
            drop(file);
        }
    });

    let new_body = r#"{
    let sum = a + b;
    sum * 88
}"#;

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn_ro,
        "add_numbers",
        "src/calc.rs",
        new_body,
        None,
    );

    lock_thread.join().unwrap();

    // Restore permissions for cleanup
    let mut perms = fs::metadata(&db_path).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    let _ = fs::set_permissions(&db_path, perms);

    assert!(res.is_err(), "Must fail when sync fails");
    match res.unwrap_err() {
        EditError::SyncRollbackFailed {
            sync_error,
            rollback_error,
        } => {
            assert!(
                !sync_error.is_empty(),
                "Sync error must be reported in SyncRollbackFailed"
            );
            assert!(
                !rollback_error.is_empty(),
                "Rollback error must be reported in SyncRollbackFailed"
            );
        }
        EditError::SyncWithRollback(_) => {
            // In case timing completed rollback after lock dropped
        }
        other => panic!("Expected SyncRollbackFailed or SyncWithRollback, got: {other:?}"),
    }
}

// ----------------------------------------------------------------------------
// 2. Stress-test mixed line endings in `edit.rs`
// ----------------------------------------------------------------------------

#[test]
fn test_adversarial_mixed_crlf_lf_in_crlf_file() {
    let _extract_bin = find_julie_extract_binary().expect("julie-extract binary must be available");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // A file with CRLF line endings
    let initial_code = "pub fn calculate(val: i32) -> i32 {\r\n    val * 2\r\n}\r\n";
    let file_path = src_dir.join("calc.rs");
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Adversarial replacement: mixed CRLF and LF lines within the incoming new body!
    let mixed_body = "{\r\n    let step1 = val * 2;\n    let step2 = step1 + 10;\r\n    step2\n}";

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "calculate",
        "src/calc.rs",
        mixed_body,
        None,
    )
    .expect("replace_symbol_body must succeed");

    assert_eq!(res.symbol_name, "calculate");

    // Verify disk content has STRICTLY CRLF line endings (every \n must be preceded by \r)
    let disk_bytes = fs::read(&file_path).unwrap();
    let disk_str = String::from_utf8(disk_bytes).unwrap();
    assert!(disk_str.contains("\r\n"), "File must contain CRLF");

    let mut prev_char = ' ';
    for ch in disk_str.chars() {
        if ch == '\n' {
            assert_eq!(
                prev_char, '\r',
                "Every newline in CRLF file must be CRLF (found bare LF after {prev_char:?})"
            );
        }
        prev_char = ch;
    }

    // Verify slicer retrieves normalized body cleanly
    let updated_symbol = code_kb_core::get_symbol_by_name(&conn, "calculate", Some("src/calc.rs"))
        .unwrap()
        .unwrap();
    let sliced_body = slicer::slice_symbol_body(&file_path, &updated_symbol).unwrap();
    assert!(
        sliced_body.contains("step2"),
        "Sliced body must contain updated code"
    );
}

#[test]
fn test_adversarial_mixed_crlf_lf_in_lf_file() {
    let _extract_bin = find_julie_extract_binary().expect("julie-extract binary must be available");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // A file with pure LF line endings
    let initial_code = "pub fn compute_sum(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    let file_path = src_dir.join("sum.rs");
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Adversarial replacement: mixed CRLF and LF lines in new body for an LF file
    let mixed_body = "{\r\n    let sum = a + b;\n    sum * 3\r\n}";

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "compute_sum",
        "src/sum.rs",
        mixed_body,
        None,
    )
    .expect("replace_symbol_body must succeed");

    assert_eq!(res.symbol_name, "compute_sum");

    // Verify disk content has ZERO carriage returns (\r)
    let disk_bytes = fs::read(&file_path).unwrap();
    let disk_str = String::from_utf8(disk_bytes).unwrap();
    assert!(
        !disk_str.contains('\r'),
        "LF file must contain zero carriage returns (\\r)"
    );
    assert!(disk_str.contains('\n'), "LF file must contain \\n");
}

#[test]
fn test_adversarial_file_with_no_newlines_at_all() {
    let _extract_bin = find_julie_extract_binary().expect("julie-extract binary must be available");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // A single line file with absolutely NO newlines
    let initial_code = "pub fn one_liner(x: i32) -> i32 { x + 1 }";
    let file_path = src_dir.join("oneline.rs");
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Replace single line function with a multi-line body
    let multi_line_body = "{\r\n    let y = x + 1;\n    y * 2\r\n}";

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "one_liner",
        "src/oneline.rs",
        multi_line_body,
        None,
    )
    .expect("replace_symbol_body must succeed on file with no newlines");

    assert_eq!(res.symbol_name, "one_liner");

    // Since the original file had no \r\n, is_crlf is false, so it uniformly normalizes to LF
    let disk_bytes = fs::read(&file_path).unwrap();
    let disk_str = String::from_utf8(disk_bytes).unwrap();
    assert!(
        !disk_str.contains('\r'),
        "Original file without newlines should normalize to LF"
    );
    assert!(disk_str.contains("let y = x + 1;\n    y * 2"));
}

#[test]
fn test_adversarial_multi_line_replacement_with_blank_lines_and_comments() {
    let _extract_bin = find_julie_extract_binary().expect("julie-extract binary must be available");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let initial_code = "pub fn complex_process(val: i32) -> i32 {\r\n    val\r\n}\r\n";
    let file_path = src_dir.join("complex.rs");
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Multi-line replacement with blank lines, comments, and mixed line endings
    let complex_body = "{\n    // Step 1: Pre-process\n    let a = val + 1;\n\n    // Step 2: Intermediate\r\n    let b = a * 2;\r\n\r\n    // Step 3: Final\n    b + 3\n}";

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "complex_process",
        "src/complex.rs",
        complex_body,
        None,
    )
    .expect("Multi-line replacement with blank lines must succeed");

    assert_eq!(res.symbol_name, "complex_process");

    let disk_bytes = fs::read(&file_path).unwrap();
    let disk_str = String::from_utf8(disk_bytes).unwrap();
    // Verify blank lines are preserved as \r\n\r\n
    assert!(
        disk_str.contains("\r\n\r\n"),
        "Blank lines must be preserved as CRLF pairs"
    );

    // Verify all newlines are CRLF
    let mut prev = ' ';
    for ch in disk_str.chars() {
        if ch == '\n' {
            assert_eq!(prev, '\r', "Every newline must be CRLF");
        }
        prev = ch;
    }
}

#[test]
fn test_adversarial_chained_edits_with_crlf_hash_concurrency() {
    let _extract_bin = find_julie_extract_binary().expect("julie-extract binary must be available");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let initial_code = "pub fn chained_calc(x: i32) -> i32 {\r\n    x + 1\r\n}\r\n";
    let file_path = src_dir.join("chained.rs");
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // First edit: mixed CRLF/LF input on CRLF file
    let edit1_body = "{\n    let a = x + 1;\r\n    a * 2\n}";
    let res1 = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "chained_calc",
        "src/chained.rs",
        edit1_body,
        None,
    )
    .expect("First edit must succeed");

    // Second edit: supply res1.new_body_hash as expected_body_hash
    let edit2_body = "{\n    let a = x + 1;\n    let b = a * 2;\r\n    b * 3\n}";
    let res2 = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "chained_calc",
        "src/chained.rs",
        edit2_body,
        Some(&res1.new_body_hash),
    )
    .expect("Second edit with expected_body_hash must succeed");

    assert_ne!(res1.new_body_hash, res2.new_body_hash);

    // Stale hash edit must fail
    let edit3_body = "{\n    42\n}";
    let res3 = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "chained_calc",
        "src/chained.rs",
        edit3_body,
        Some(&res1.new_body_hash),
    );
    assert!(res3.is_err(), "Edit with stale hash must fail");
}

#[test]
fn test_adversarial_multi_byte_utf8_symbol_replacement() {
    let _extract_bin = find_julie_extract_binary().expect("julie-extract binary must be available");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let initial_code = "pub fn greeting() -> &'static str {\n    \"こんにちは世界 🦀\"\n}\n";
    let file_path = src_dir.join("utf8.rs");
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let new_body = "{\n    \"Здравствуйте, мир! 🚀 🦀\"\n}";

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "greeting",
        "src/utf8.rs",
        new_body,
        None,
    )
    .expect("UTF-8 replacement must succeed");

    assert_eq!(res.symbol_name, "greeting");

    let disk_str = fs::read_to_string(&file_path).unwrap();
    assert!(disk_str.contains("Здравствуйте, мир! 🚀 🦀"));
}

// ----------------------------------------------------------------------------
// 3. SQLite memory mapping (`mmap_size = 0` on Windows) and `checkpoint_truncate`
// ----------------------------------------------------------------------------

#[test]
fn test_adversarial_mmap_size_zero_on_windows_allows_concurrent_writes() {
    let temp_dir = safe_tempdir();
    let db_path = temp_dir.path().join("mmap_stress.db");

    // Initialize DB with WAL mode
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

    // Open read-only connection
    let conn_ro = open_read_only(&db_path).unwrap();

    // Verify mmap_size pragma
    let mmap_size: i64 = conn_ro
        .query_row("PRAGMA mmap_size;", [], |r| r.get(0))
        .unwrap();

    #[cfg(windows)]
    assert_eq!(
        mmap_size, 0,
        "PRAGMA mmap_size must be 0 on Windows to prevent section locks"
    );

    #[cfg(not(windows))]
    assert_eq!(
        mmap_size, 268435456,
        "PRAGMA mmap_size should be 256MB on non-Windows"
    );

    // While conn_ro remains active and open, write 500 records with conn_rw
    for i in 0..500 {
        conn_rw
            .execute(
                "INSERT INTO stress (msg) VALUES (?1);",
                [format!("row_{i}")],
            )
            .unwrap();
    }

    // Checkpoint with TRUNCATE while conn_ro is still open
    // On Windows, if mmap_size were non-zero, this would fail with ERROR_USER_MAPPED_FILE (1224)
    let cp_res = checkpoint_truncate(&conn_rw);
    assert!(
        cp_res.is_ok(),
        "checkpoint_truncate must succeed while read-only connection is active: {:?}",
        cp_res.err()
    );

    // Verify read-only connection reads all rows cleanly
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

    // Insert 500 rows to ensure WAL file has significant content
    for i in 0..500 {
        conn_rw
            .execute(
                "INSERT INTO audit_log (entry) VALUES (?1);",
                [format!("audit_entry_{i}")],
            )
            .unwrap();
    }

    // Ensure WAL exists and has non-zero length
    assert!(wal_path.exists(), "WAL file must exist after transactions");
    let wal_len_before = fs::metadata(&wal_path).unwrap().len();
    assert!(
        wal_len_before > 0,
        "WAL file size must be > 0 bytes before checkpoint: got {wal_len_before}"
    );

    // Call checkpoint_truncate
    checkpoint_truncate(&conn_rw).expect("checkpoint_truncate must succeed");

    // On TRUNCATE, SQLite truncates the WAL file to 0 bytes
    let wal_len_after = fs::metadata(&wal_path).unwrap().len();
    assert_eq!(
        wal_len_after, 0,
        "WAL file size must be truncated to 0 bytes after PRAGMA wal_checkpoint(TRUNCATE)"
    );

    // Even if we delete or don't copy the WAL file, main DB has all data
    drop(conn_rw);
    let conn_ro = open_read_only(&db_path).unwrap();
    let count: i64 = conn_ro
        .query_row("SELECT count(*) FROM audit_log;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 500, "All rows must be present in main DB file");
}

#[test]
fn test_adversarial_worktree_db_copy_contains_all_flushed_transactions() {
    let temp_dir = safe_tempdir();
    let parent_dir = temp_dir.path().join("parent").join(".code-kb");
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

    // Flush WAL to main DB using checkpoint_truncate
    checkpoint_truncate(&conn_rw).expect("checkpoint_truncate must succeed");
    drop(conn_rw); // Close handle before copy

    // Simulate worktree copy: copy parent_db to worktree_db WITHOUT copying parent_db-wal
    let worktree_dir = temp_dir.path().join("worktree").join(".code-kb");
    fs::create_dir_all(&worktree_dir).unwrap();
    let worktree_db = worktree_dir.join("artifact.db");

    fs::copy(&parent_db, &worktree_db).expect("Copy must succeed");

    // Open worktree DB in read-only mode and verify all 100 symbols exist
    let conn_worktree = open_read_only(&worktree_db).expect("Open worktree DB must succeed");
    let count: i64 = conn_worktree
        .query_row("SELECT count(*) FROM symbols;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 100,
        "Worktree DB snapshot must contain all 100 transactions without WAL dependency"
    );
}
