use code_kb_core::{
    Workspace, ensure_fresh_file, ensure_index_matches_extractor, find_julie_extract_binary,
    get_file, get_symbol_by_name, installed_extractor_version, open_read_only, open_read_write,
    reconcile_offline_edits, safe_tempdir, scan_workspace, update_file,
};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn test_ensure_fresh_file_detects_equal_size_edit() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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

    // Modify file with exactly equal length (40 bytes): replace 'foo_fn' with 'bar_fn'
    let modified_code = "pub fn bar_fn() -> i32 {\n    100\n}\n";
    assert_eq!(initial_code.len(), modified_code.len());
    fs::write(&file_path, modified_code).unwrap();

    // ensure_fresh_file should detect content change via content_hash check
    let changed = ensure_fresh_file(&ws, &db_path, &conn, "src/calc.rs").unwrap();
    assert!(changed, "ensure_fresh_file should report file changed");

    // Symbol in database should now be bar_fn
    let symbol = get_symbol_by_name(&conn, "bar_fn", Some("src/calc.rs"))
        .unwrap()
        .expect("bar_fn should exist in db");
    assert_eq!(symbol.name, "bar_fn");
}

#[test]
fn test_update_file_scans_headers_with_cpp_detection() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let repo = safe_tempdir();
    let root = repo.path().to_path_buf();
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    let header = src.join("widget.h");
    fs::write(
        &header,
        "class Widget {\n    Q_OBJECT\n    Q_PROPERTY(int value READ value)\npublic:\n    int value() const { return m_value; }\nprivate:\n    int m_value = 0;\n};\n",
    )
    .unwrap();
    fs::write(src.join("unchanged.rs"), "pub fn unchanged() {}\n").unwrap();

    let workspace = Workspace::new(root);
    let db = workspace.canonical_root.join("index.db");
    scan_workspace(&workspace, &db, true).unwrap();
    let (header_hash, unchanged_hash) = {
        let conn = open_read_only(&db).unwrap();
        let header = get_file(&conn, "src/widget.h").unwrap().unwrap();
        assert_eq!(header.language, "cpp");
        let unchanged_hash = get_file(&conn, "src/unchanged.rs")
            .unwrap()
            .unwrap()
            .content_hash;
        (header.content_hash, unchanged_hash)
    };

    fs::write(
        &header,
        "class Widget {\n    Q_OBJECT\n    Q_PROPERTY(int value READ value)\npublic:\n    int value() const { return m_value; }\nprivate:\n    int m_value = 1;\n};\n",
    )
    .unwrap();
    update_file(&workspace, &db, "src/widget.h").unwrap();

    let conn = open_read_only(&db).unwrap();
    let header = get_file(&conn, "src/widget.h").unwrap().unwrap();
    assert_eq!(header.language, "cpp");
    assert_ne!(header.content_hash, header_hash);
    assert_eq!(
        get_file(&conn, "src/unchanged.rs")
            .unwrap()
            .unwrap()
            .content_hash,
        unchanged_hash
    );
    drop(conn);

    assert!(matches!(
        update_file(&workspace, &db, "src/missing.h"),
        Err(code_kb_core::SyncError::TargetNotIndexed(path)) if path == "src/missing.h"
    ));
}

#[test]
fn test_ensure_fresh_file_removes_deleted_file_from_index() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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
fn test_reconcile_offline_edits_refreshes_two_headers_together() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    for (name, value) in [("first", 0), ("second", 0)] {
        fs::write(
            src_dir.join(format!("{name}.h")),
            format!("class {name} {{ public: int value = {value}; }};\n"),
        )
        .unwrap();
    }

    let ws = Workspace::new(root);
    let db_path = ws.canonical_root.join("test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();
    let initial_hashes: Vec<(String, String)> = ["src/first.h", "src/second.h"]
        .into_iter()
        .map(|path| {
            (
                path.to_string(),
                get_file(&conn, path).unwrap().unwrap().content_hash,
            )
        })
        .collect();
    let revisions_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM extraction_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    for (name, value) in [("first", 1), ("second", 1)] {
        fs::write(
            ws.canonical_root.join(format!("src/{name}.h")),
            format!("class {name} {{ public: int value = {value}; }};\n"),
        )
        .unwrap();
    }

    let report = reconcile_offline_edits(&ws, &db_path, &conn).unwrap();

    assert_eq!(report.modified.len(), 2, "{report:?}");
    for (path, initial_hash) in initial_hashes {
        let stored = get_file(&conn, &path).unwrap().unwrap();
        assert_ne!(
            stored.content_hash, initial_hash,
            "{path} was not refreshed"
        );
    }
    let revisions_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM extraction_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(revisions_after, revisions_before + 1);
}

#[test]
fn test_reconcile_offline_edits_forces_a_large_batch_with_a_header() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("widget.h"),
        "class Widget { int old_value; };\n",
    )
    .unwrap();
    for index in 0..50 {
        fs::write(
            src_dir.join(format!("file_{index}.rs")),
            format!("pub fn old_{index}() {{}}\n"),
        )
        .unwrap();
    }

    let ws = Workspace::new(root);
    let db_path = ws.canonical_root.join("test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();
    let header_hash = get_file(&conn, "src/widget.h")
        .unwrap()
        .unwrap()
        .content_hash;
    let revisions_before: i64 = conn
        .query_row("SELECT COUNT(*) FROM extraction_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    fs::write(
        ws.canonical_root.join("src/widget.h"),
        "class Widget { int new_value; };\n",
    )
    .unwrap();
    for index in 0..50 {
        fs::write(
            ws.canonical_root.join(format!("src/file_{index}.rs")),
            format!("pub fn new_{index}() {{}}\n"),
        )
        .unwrap();
    }

    let report = reconcile_offline_edits(&ws, &db_path, &conn).unwrap();

    assert_eq!(report.modified.len(), 51, "{report:?}");
    let stored = get_file(&conn, "src/widget.h").unwrap().unwrap();
    assert_ne!(stored.content_hash, header_hash);
    let revisions_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM extraction_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(revisions_after, revisions_before + 1);
}

#[test]
fn test_get_symbol_body_fresh_after_comment_added() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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

#[cfg(unix)]
#[test]
fn test_reconcile_offline_edits_continues_when_individual_update_fails() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("valid.rs"), "pub fn valid_symbol() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();

    // Add another valid file and an unreadable file that will fail during update_file
    fs::write(src_dir.join("another.rs"), "pub fn another_symbol() {}\n").unwrap();
    let unreadable_path = src_dir.join("unreadable.rs");
    fs::write(&unreadable_path, "pub fn unreadable_symbol() {}\n").unwrap();
    fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o000)).unwrap();

    let result = reconcile_offline_edits(&ws, &db_path, &conn);
    // Restore permissions so cleanup succeeds
    fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o644)).unwrap();

    let report =
        result.expect("reconciliation should succeed even when an individual update fails");
    assert!(report.added.contains(&"src/another.rs".to_string()));
    assert!(report.added.contains(&"src/unreadable.rs".to_string()));
    assert!(
        get_symbol_by_name(&conn, "another_symbol", Some("src/another.rs"))
            .unwrap()
            .is_some(),
        "Valid file must be successfully indexed into database"
    );
    assert!(
        get_symbol_by_name(&conn, "unreadable_symbol", Some("src/unreadable.rs"))
            .unwrap()
            .is_none(),
        "Failed file must not be indexed into database"
    );

    let retry = reconcile_offline_edits(&ws, &db_path, &conn).unwrap();
    assert!(
        retry.added.contains(&"src/unreadable.rs".to_string()),
        "got: {:?}",
        retry.added
    );
    assert!(
        get_symbol_by_name(&conn, "unreadable_symbol", Some("src/unreadable.rs"))
            .unwrap()
            .is_some(),
        "A file whose update failed once must be retried and indexed"
    );
}

#[test]
fn test_codebase_outline_depth_bounded_symbols() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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

#[test]
fn test_reconcile_offline_edits_preserves_hidden_files() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let hidden_dir = root.join(".config");
    fs::create_dir_all(&hidden_dir).unwrap();
    let file_path = hidden_dir.join("helper.rs");
    fs::write(&file_path, "pub fn hidden_helper() -> i32 { 42 }\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Verify it was indexed
    let sym = code_kb_core::get_symbol_by_name(&conn, "hidden_helper", Some(".config/helper.rs"))
        .unwrap();
    assert!(sym.is_some(), "Symbol in hidden dir should be indexed");

    // Run reconciliation without changing the file
    let report =
        reconcile_offline_edits(&ws, &db_path, &conn).expect("reconcile_offline_edits failed");

    // Hidden file must NOT be reported as deleted
    assert!(
        !report.deleted.contains(&".config/helper.rs".to_string()),
        "Hidden file should not be reported as deleted: {:?}",
        report.deleted
    );

    // Verify symbol still exists in DB
    let sym_after =
        code_kb_core::get_symbol_by_name(&conn, "hidden_helper", Some(".config/helper.rs"))
            .unwrap();
    assert!(
        sym_after.is_some(),
        "Symbol in hidden dir should still exist after reconciliation"
    );
}

#[test]
fn test_index_from_other_extractor_version_is_rebuilt() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("calc.rs"),
        "pub fn foo_fn() -> i32 {\n    100\n}\n",
    )
    .unwrap();
    let ws = Workspace::new(root.clone());
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let installed = installed_extractor_version();
    assert!(!ensure_index_matches_extractor(&ws, &db_path, &installed).unwrap());

    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE artifact_metadata SET value = '0.0.1' WHERE key = 'binary_version'",
            [],
        )
        .unwrap();
    }
    assert!(ensure_index_matches_extractor(&ws, &db_path, &installed).unwrap());
    assert!(!ensure_index_matches_extractor(&ws, &db_path, &installed).unwrap());

    let conn = open_read_only(&db_path).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM artifact_metadata WHERE key = 'binary_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(version, "0.0.1");
    assert!(get_symbol_by_name(&conn, "foo_fn", None).unwrap().is_some());
}

#[test]
fn test_index_written_by_the_installed_extractor_is_kept_even_when_it_is_not_the_pinned_build() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("calc.rs"),
        "pub fn foo_fn() -> i32 {\n    100\n}\n",
    )
    .unwrap();
    let ws = Workspace::new(root.clone());
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE artifact_metadata SET value = '0.0.1' WHERE key = 'binary_version'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE extraction_revisions SET binary_version = '0.0.1'",
            [],
        )
        .unwrap();
    }

    assert!(!ensure_index_matches_extractor(&ws, &db_path, "0.0.1").unwrap());

    let conn = open_read_only(&db_path).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM artifact_metadata WHERE key = 'binary_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, "0.0.1");
}

#[test]
fn test_scan_replaces_an_empty_artifact_file() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("calc.rs"),
        "pub fn foo_fn() -> i32 {\n    100\n}\n",
    )
    .unwrap();
    let db_path = root.join(".code-kb").join("artifact.db");
    fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    fs::write(&db_path, b"").unwrap();
    let ws = Workspace::new(root.clone());

    scan_workspace(&ws, &db_path, false).expect("scan must replace an empty artifact");

    let conn = open_read_only(&db_path).unwrap();
    assert!(get_symbol_by_name(&conn, "foo_fn", None).unwrap().is_some());
}

#[test]
fn test_index_at_another_extraction_level_is_rebuilt() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("calc.rs"),
        "pub fn foo_fn() -> i32 {\n    100\n}\n",
    )
    .unwrap();
    let ws = Workspace::new(root.clone());
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let installed = installed_extractor_version();
    assert!(!ensure_index_matches_extractor(&ws, &db_path, &installed).unwrap());
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE artifact_metadata SET value = 'full' WHERE key = 'index_level'",
            [],
        )
        .unwrap();
    }

    assert!(ensure_index_matches_extractor(&ws, &db_path, &installed).unwrap());

    let conn = open_read_only(&db_path).unwrap();
    let level: String = conn
        .query_row(
            "SELECT value FROM artifact_metadata WHERE key = 'index_level'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(level, "facts");
}

#[test]
fn test_ensure_index_matches_extractor_rebuilds_when_a_file_revision_differs() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn stable() {}\n").unwrap();
    let ws = Workspace::new(root.clone());
    let db_path = root.join(".code-kb/artifact.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let installed = installed_extractor_version();

    assert!(!ensure_index_matches_extractor(&ws, &db_path, &installed).unwrap());

    {
        let conn = open_read_write(&db_path).unwrap();
        conn.execute(
            "UPDATE extraction_revisions SET binary_version = '0.0.1'",
            [],
        )
        .unwrap();
    }

    assert!(ensure_index_matches_extractor(&ws, &db_path, &installed).unwrap());

    let conn = open_read_only(&db_path).unwrap();
    let stale: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files f JOIN extraction_revisions r ON r.revision_id = f.last_revision_id WHERE r.binary_version != ?1",
            [&installed],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
}

#[test]
fn test_reconcile_offline_edits_remembers_files_the_extractor_skips() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.rs"), "pub fn func_a() {}\n").unwrap();
    let lock_path = root.join("Cargo.lock");
    fs::write(&lock_path, "[[package]]\nname = \"a\"\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    fs::write(&lock_path, "[[package]]\nname = \"a\"\nversion = \"1\"\n").unwrap();
    let lock = "Cargo.lock".to_string();
    let edited = reconcile_offline_edits(&ws, &db_path, &conn).unwrap();
    assert!(
        edited.modified.contains(&lock),
        "got: {:?}",
        edited.modified
    );

    let settled = reconcile_offline_edits(&ws, &db_path, &conn).unwrap();
    assert!(!settled.added.contains(&lock), "got: {:?}", settled.added);
    assert!(!settled.modified.contains(&lock));

    fs::write(&lock_path, "[[package]]\nname = \"a\"\nversion = \"1.0\"\n").unwrap();
    let edited_again = reconcile_offline_edits(&ws, &db_path, &conn).unwrap();
    assert!(
        edited_again.added.contains(&lock),
        "got: {:?}",
        edited_again.added
    );

    let settled_again = reconcile_offline_edits(&ws, &db_path, &conn).unwrap();
    assert!(
        !settled_again.added.contains(&lock),
        "got: {:?}",
        settled_again.added
    );
}

#[cfg(unix)]
#[test]
fn test_scan_workspace_keeps_the_index_when_one_file_cannot_be_read() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/ok.rs"), "pub fn readable_symbol() {}\n").unwrap();
    let locked = root.join("src/locked.rs");
    fs::write(&locked, "pub fn locked_symbol() {}\n").unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    let result = scan_workspace(&ws, &db_path, true);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();

    result.expect("a scan that skips an unreadable file must keep the index");
    let conn = open_read_only(&db_path).unwrap();
    assert!(
        get_symbol_by_name(&conn, "readable_symbol", Some("src/ok.rs"))
            .unwrap()
            .is_some()
    );
    assert!(
        get_symbol_by_name(&conn, "locked_symbol", Some("src/locked.rs"))
            .unwrap()
            .is_none()
    );
}
