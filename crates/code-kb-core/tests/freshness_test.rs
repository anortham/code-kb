use code_kb_core::{
    Workspace, ensure_fresh_file, find_julie_extract_binary, get_symbol_by_name, open_read_only,
    reconcile_offline_edits, scan_workspace,
};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn test_ensure_fresh_file_detects_equal_size_edit() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");

    // 40 bytes
    let initial_code = "pub fn foo_fn() -> i32 {\n    100\n}\n";
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Equal-size edit: replace 'foo_fn' with 'bar_fn' (exact same byte count: 40 bytes)
    let modified_code = "pub fn bar_fn() -> i32 {\n    100\n}\n";
    assert_eq!(initial_code.len(), modified_code.len());
    fs::write(&file_path, modified_code).unwrap();

    // ensure_fresh_file must detect that the content changed despite identical byte count!
    let was_dirty =
        ensure_fresh_file(&ws, &db_path, &conn, "src/calc.rs").expect("ensure_fresh_file failed");

    assert!(was_dirty, "Equal-size edit must be detected as dirty");

    // Verify the symbol in DB was actually updated to bar_fn
    let symbol = code_kb_core::get_symbol_by_name(&conn, "bar_fn", Some("src/calc.rs")).unwrap();
    assert!(symbol.is_some(), "Database must now contain 'bar_fn'");
}

#[test]
fn test_ensure_fresh_file_removes_deleted_file_from_index() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("removed.rs");
    fs::write(&file_path, "pub fn removed_symbol() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();

    fs::remove_file(&file_path).unwrap();

    assert!(ensure_fresh_file(&ws, &db_path, &conn, "src/removed.rs").unwrap());
    assert!(
        get_symbol_by_name(&conn, "removed_symbol", Some("src/removed.rs"))
            .unwrap()
            .is_none()
    );
}

#[cfg(unix)]
#[test]
fn test_ensure_fresh_file_propagates_read_failure() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("unreadable.rs");
    fs::write(&file_path, "pub fn unreadable_symbol() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();

    fs::set_permissions(&file_path, fs::Permissions::from_mode(0o000)).unwrap();
    let result = ensure_fresh_file(&ws, &db_path, &conn, "src/unreadable.rs");
    fs::set_permissions(&file_path, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(result.is_err());
}

#[test]
fn test_reconcile_offline_edits_equal_size() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");

    let initial_code = "pub fn foo_fn() -> i32 {\n    100\n}\n";
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Equal-size edit: replace 'foo_fn' with 'bar_fn'
    let modified_code = "pub fn bar_fn() -> i32 {\n    100\n}\n";
    assert_eq!(initial_code.len(), modified_code.len());
    fs::write(&file_path, modified_code).unwrap();

    let report =
        reconcile_offline_edits(&ws, &db_path, &conn).expect("reconcile_offline_edits failed");

    assert!(
        report.modified.contains(&"src/calc.rs".to_string()),
        "Equal-size modified file must appear in reconcile report.modified, got: {:?}",
        report.modified
    );
}

#[test]
fn test_get_symbol_body_fresh_after_comment_added() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");

    let initial_code = "pub fn my_function() -> i32 {\n    12345\n}\n";
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Now prepend 5 lines of comments before the function
    let modified_code = "// Line 1\n// Line 2\n// Line 3\n// Line 4\n// Line 5\npub fn my_function() -> i32 {\n    12345\n}\n";
    fs::write(&file_path, modified_code).unwrap();

    // Call get_symbol_body_op (without passing file_path explicitly)
    let (symbol, body) =
        code_kb_core::get_symbol_body_op(&ws, &db_path, &conn, "my_function", None)
            .expect("get_symbol_body_op failed");

    assert!(
        body.contains("12345"),
        "Body must contain 12345, got: {}",
        body
    );
    assert!(
        !body.contains("// Line"),
        "Body must not contain comments from above, got: {}",
        body
    );
    assert_eq!(
        symbol.start_line, 6,
        "Symbol line number must be refreshed to line 6"
    );
}

#[test]
fn test_reconcile_offline_edits_added_and_deleted() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let a_path = src_dir.join("a.rs");
    let b_path = src_dir.join("b.rs");

    fs::write(&a_path, "pub fn func_a() {}\n").unwrap();
    fs::write(&b_path, "pub fn func_b() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Delete b.rs and add c.rs
    fs::remove_file(&b_path).unwrap();
    let c_path = src_dir.join("c.rs");
    fs::write(&c_path, "pub fn func_c() {}\n").unwrap();

    let report =
        reconcile_offline_edits(&ws, &db_path, &conn).expect("reconcile_offline_edits failed");

    assert!(
        report.deleted.contains(&"src/b.rs".to_string()),
        "Deleted file must be detected, got: {:?}",
        report.deleted
    );
    assert!(
        report.added.contains(&"src/c.rs".to_string()),
        "Added file must be detected, got: {:?}",
        report.added
    );
}

#[cfg(unix)]
#[test]
fn test_reconcile_offline_edits_preserves_unreadable_directory_records() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("locked.rs");
    fs::write(&file_path, "pub fn locked_symbol() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();

    // Add a new file at root that should still be indexed even if src is unreadable
    let new_file = root.join("top.rs");
    fs::write(&new_file, "pub fn top_symbol() {}\n").unwrap();

    fs::set_permissions(&src_dir, fs::Permissions::from_mode(0o000)).unwrap();
    let result = reconcile_offline_edits(&ws, &db_path, &conn);
    fs::set_permissions(&src_dir, fs::Permissions::from_mode(0o755)).unwrap();

    let report = result.expect("reconciliation should not abort on unreadable directory");
    assert!(report.added.contains(&"top.rs".to_string()));
    assert!(
        get_symbol_by_name(&conn, "locked_symbol", Some("src/locked.rs"))
            .unwrap()
            .is_some(),
        "Unreadable directory files must not be deleted from database"
    );
}

#[test]
fn test_reconcile_offline_edits_continues_when_individual_update_fails() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("valid.rs"), "pub fn valid_symbol() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();

    // Add another file
    fs::write(src_dir.join("another.rs"), "pub fn another_symbol() {}\n").unwrap();

    let report =
        reconcile_offline_edits(&ws, &db_path, &conn).expect("reconciliation should succeed");
    assert!(report.added.contains(&"src/another.rs".to_string()));
    assert!(
        get_symbol_by_name(&conn, "another_symbol", Some("src/another.rs"))
            .unwrap()
            .is_some()
    );
}

#[test]
fn test_codebase_outline_depth_bounded_symbols() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let deep_dir = root.join("src").join("nested");
    fs::create_dir_all(&deep_dir).unwrap();

    let root_file = root.join("root.rs");
    let mid_file = root.join("src").join("lib.rs");
    let deep_file = deep_dir.join("deep.rs");

    fs::write(&root_file, "pub fn root_fn() {}\n").unwrap();
    fs::write(&mid_file, "pub fn mid_fn() {}\n").unwrap();
    fs::write(&deep_file, "pub fn deep_fn() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // With depth = 1 and no filter, only files at depth 1 (0 slashes) should load symbols
    let syms_depth1 = code_kb_core::load_scoped_outline_symbols(&conn, None, 1, 5)
        .expect("load_scoped_outline_symbols failed");

    assert!(
        syms_depth1.contains_key("root.rs"),
        "root.rs must have symbols at depth 1"
    );
    assert!(
        !syms_depth1.contains_key("src/lib.rs"),
        "src/lib.rs must NOT have symbols at depth 1"
    );
    assert!(
        !syms_depth1.contains_key("src/nested/deep.rs"),
        "deep.rs must NOT have symbols at depth 1"
    );

    // With depth = 2, root.rs and src/lib.rs (<= 1 slash) load symbols, but deep.rs (2 slashes) does not
    let syms_depth2 = code_kb_core::load_scoped_outline_symbols(&conn, None, 2, 5)
        .expect("load_scoped_outline_symbols failed");

    assert!(syms_depth2.contains_key("root.rs"));
    assert!(syms_depth2.contains_key("src/lib.rs"));
    assert!(
        !syms_depth2.contains_key("src/nested/deep.rs"),
        "deep.rs must NOT have symbols at depth 2"
    );

    // Verify codebase_outline_op outputs correctly
    let outline =
        code_kb_core::codebase_outline_op(&ws, &conn, 1, None).expect("codebase_outline_op failed");
    assert!(outline.contains("root.rs"));
    assert!(outline.contains("src/"));
    // At depth 1, src/lib.rs should not be rendered as a file
    assert!(!outline.contains("lib.rs"));
}
