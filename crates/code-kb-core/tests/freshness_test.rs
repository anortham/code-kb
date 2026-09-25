use code_kb_core::{
    OpError, SymbolSelector, Workspace, create_index, ensure_fresh_file,
    ensure_index_matches_extractor, find_julie_extract_binary, get_file, get_symbol_by_id,
    get_symbol_by_name, installed_extractor_version, open_read_only, open_read_write,
    reconcile_offline_edits, resolve_symbol_op, safe_tempdir, scan_workspace, update_file,
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
fn symbol_id_refreshes_or_requires_reselection_without_name_fallback() {
    let _extract_bin = find_julie_extract_binary().unwrap();
    let repo = safe_tempdir();
    let source = repo.path().join("src");
    fs::create_dir_all(&source).unwrap();
    let file = source.join("target.rs");
    fs::write(source.join("sibling.rs"), "pub fn same_name() { 9; }\n").unwrap();
    fs::write(&file, "pub fn same_name() { 1; }\n").unwrap();
    let workspace = Workspace::new(repo.path().to_path_buf());
    let db = repo.path().join("index.db");
    scan_workspace(&workspace, &db, true).unwrap();
    let conn = open_read_only(&db).unwrap();
    let id = get_symbol_by_name(&conn, "same_name", Some("src/target.rs"))
        .unwrap()
        .unwrap()
        .symbol_id;
    fs::write(&file, "pub fn same_name() { 2; }\n").unwrap();
    let refreshed = resolve_symbol_op(
        &workspace,
        &db,
        &conn,
        &SymbolSelector::Id(id.clone()),
        None,
    )
    .unwrap();
    assert_eq!(refreshed.symbol_id, id);
    fs::remove_file(&file).unwrap();
    let missing_after_refresh =
        resolve_symbol_op(&workspace, &db, &conn, &SymbolSelector::Id(id), None).unwrap_err();
    assert!(matches!(
        &missing_after_refresh,
        OpError::StaleSymbolId { .. }
    ));
    assert!(
        get_symbol_by_name(&conn, "same_name", Some("src/sibling.rs"))
            .unwrap()
            .is_some()
    );
    let refresh_message = missing_after_refresh.to_string();
    assert!(refresh_message.contains("is no longer indexed"));
    assert!(refresh_message.contains("run lookup_symbol or search_symbols"));
    assert!(refresh_message.contains(repo.path().file_name().unwrap().to_str().unwrap()));
    assert!(!refresh_message.contains("same_name"));
    assert!(!refresh_message.contains("Did you mean one of"));
    let already_missing = resolve_symbol_op(
        &workspace,
        &db,
        &conn,
        &SymbolSelector::Id("absent-id".to_string()),
        None,
    )
    .unwrap_err();
    let message = already_missing.to_string();
    assert!(message.contains("symbol_id 'absent-id' is no longer indexed"));
    assert!(message.contains("run lookup_symbol or search_symbols"));
    assert!(message.contains(repo.path().file_name().unwrap().to_str().unwrap()));
    assert!(!message.contains("Did you mean one of"));
}

#[test]
fn symbol_id_refresh_reloads_offsets_after_native_edit() {
    let _extract_bin = find_julie_extract_binary().unwrap();
    let repo = safe_tempdir();
    let source = repo.path().join("src");
    fs::create_dir_all(&source).unwrap();
    let file = source.join("target.rs");
    fs::write(&file, "pub fn selected() { 1; }\n").unwrap();
    let workspace = Workspace::new(repo.path().to_path_buf());
    let db = repo.path().join("index.db");
    scan_workspace(&workspace, &db, true).unwrap();
    let conn = open_read_only(&db).unwrap();
    let id = get_symbol_by_name(&conn, "selected", Some("src/target.rs"))
        .unwrap()
        .unwrap()
        .symbol_id;
    fs::write(&file, "// moved\npub fn selected() { 1; }\n").unwrap();

    match resolve_symbol_op(
        &workspace,
        &db,
        &conn,
        &SymbolSelector::Id(id.clone()),
        None,
    ) {
        Ok(refreshed) => {
            assert_eq!(refreshed.symbol_id, id);
            assert_eq!(refreshed.start_line, 2);
        }
        Err(error @ OpError::StaleSymbolId { .. }) => {
            assert!(
                error
                    .to_string()
                    .contains("run lookup_symbol or search_symbols")
            );
        }
        Err(error) => panic!("unexpected result: {error}"),
    }
}

#[test]
fn symbol_id_file_guard_uses_workspace_path_identity() {
    let _extract_bin = find_julie_extract_binary().unwrap();
    let repo = safe_tempdir();
    let source = repo.path().join("src");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("one.rs"), "pub fn first() {}\n").unwrap();
    fs::write(source.join("two.rs"), "pub fn second() {}\n").unwrap();
    let workspace = Workspace::new(repo.path().to_path_buf());
    let db = repo.path().join("index.db");
    scan_workspace(&workspace, &db, true).unwrap();
    let conn = open_read_only(&db).unwrap();
    let id = get_symbol_by_name(&conn, "first", None)
        .unwrap()
        .unwrap()
        .symbol_id;
    let absolute = source.join("one.rs");
    assert!(
        resolve_symbol_op(
            &workspace,
            &db,
            &conn,
            &SymbolSelector::Id(id.clone()),
            absolute.to_str()
        )
        .is_ok()
    );
    assert!(
        resolve_symbol_op(
            &workspace,
            &db,
            &conn,
            &SymbolSelector::Id(id.clone()),
            Some("src\\one.rs")
        )
        .is_ok()
    );
    assert!(matches!(
        resolve_symbol_op(
            &workspace,
            &db,
            &conn,
            &SymbolSelector::Id(id),
            Some("src/two.rs")
        ),
        Err(OpError::FileGuardMismatch { .. })
    ));
}

#[test]
fn symbol_id_file_guard_refreshes_owner_before_rejecting_mismatch() {
    let _extract_bin = find_julie_extract_binary().unwrap();
    let repo = safe_tempdir();
    let source = repo.path().join("src");
    fs::create_dir_all(&source).unwrap();
    let file = source.join("target.rs");
    let guard = source.join("guard.rs");
    fs::write(&file, "pub fn selected() { 1; }\n").unwrap();
    fs::write(&guard, "pub fn guard() {}\n").unwrap();
    let workspace = Workspace::new(repo.path().to_path_buf());
    let db = repo.path().join("index.db");
    scan_workspace(&workspace, &db, true).unwrap();
    let conn = open_read_only(&db).unwrap();
    let selected = get_symbol_by_name(&conn, "selected", Some("src/target.rs"))
        .unwrap()
        .unwrap();
    let old_body_hash = selected.body_hash.unwrap();
    fs::write(&file, "pub fn selected() { 2; }\n").unwrap();

    let result = resolve_symbol_op(
        &workspace,
        &db,
        &conn,
        &SymbolSelector::Id(selected.symbol_id.clone()),
        guard.to_str(),
    );

    assert!(matches!(result, Err(OpError::FileGuardMismatch { .. })));
    let refreshed = get_symbol_by_id(&conn, &selected.symbol_id)
        .unwrap()
        .unwrap();
    assert_ne!(refreshed.body_hash.as_deref(), Some(old_body_hash.as_str()));
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
    let (header_hash, unchanged_hash, unchanged_revision, revisions_before) = {
        let conn = open_read_only(&db).unwrap();
        let header = get_file(&conn, "src/widget.h").unwrap().unwrap();
        assert_eq!(header.language, "cpp");
        let unchanged_hash = get_file(&conn, "src/unchanged.rs")
            .unwrap()
            .unwrap()
            .content_hash;
        let unchanged_revision: i64 = conn
            .query_row(
                "SELECT last_revision_id FROM files WHERE path = ?1",
                ["src/unchanged.rs"],
                |row| row.get(0),
            )
            .unwrap();
        let revisions_before: i64 = conn
            .query_row("SELECT COUNT(*) FROM extraction_revisions", [], |row| {
                row.get(0)
            })
            .unwrap();
        (
            header.content_hash,
            unchanged_hash,
            unchanged_revision,
            revisions_before,
        )
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
    assert_eq!(
        conn.query_row(
            "SELECT last_revision_id FROM files WHERE path = ?1",
            ["src/unchanged.rs"],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        unchanged_revision
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM extraction_revisions", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        revisions_before + 1
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
    fs::write(src_dir.join("unchanged.rs"), "pub fn unchanged() {}\n").unwrap();

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
    let unchanged = get_file(&conn, "src/unchanged.rs").unwrap().unwrap();
    let unchanged_revision: i64 = conn
        .query_row(
            "SELECT last_revision_id FROM files WHERE path = ?1",
            ["src/unchanged.rs"],
            |row| row.get(0),
        )
        .unwrap();
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
    assert_eq!(
        get_file(&conn, "src/unchanged.rs")
            .unwrap()
            .unwrap()
            .content_hash,
        unchanged.content_hash
    );
    assert_eq!(
        conn.query_row(
            "SELECT last_revision_id FROM files WHERE path = ?1",
            ["src/unchanged.rs"],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        unchanged_revision
    );
    let revisions_after: i64 = conn
        .query_row("SELECT COUNT(*) FROM extraction_revisions", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(revisions_after, revisions_before + 1);
}

#[test]
fn test_reconcile_offline_edits_refreshes_a_large_batch_with_a_header() {
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
    fs::write(src_dir.join("unchanged.rs"), "pub fn unchanged() {}\n").unwrap();

    let ws = Workspace::new(root);
    let db_path = ws.canonical_root.join("test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();
    let header_hash = get_file(&conn, "src/widget.h")
        .unwrap()
        .unwrap()
        .content_hash;
    let unchanged = get_file(&conn, "src/unchanged.rs").unwrap().unwrap();
    let unchanged_revision: i64 = conn
        .query_row(
            "SELECT last_revision_id FROM files WHERE path = ?1",
            ["src/unchanged.rs"],
            |row| row.get(0),
        )
        .unwrap();
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
    assert_eq!(
        get_file(&conn, "src/unchanged.rs")
            .unwrap()
            .unwrap()
            .content_hash,
        unchanged.content_hash
    );
    assert_eq!(
        conn.query_row(
            "SELECT last_revision_id FROM files WHERE path = ?1",
            ["src/unchanged.rs"],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        unchanged_revision
    );
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
fn a_worktree_index_copied_from_an_older_extractor_is_rebuilt() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let main_root = temp_dir.path().join("main");
    let wt_root = temp_dir.path().join("feature");
    for root in [&main_root, &wt_root] {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src").join("calc.rs"),
            "pub fn foo_fn() -> i32 {\n    100\n}\n",
        )
        .unwrap();
    }
    fs::create_dir_all(main_root.join(".git").join("worktrees").join("feature")).unwrap();
    fs::write(
        wt_root.join(".git"),
        format!(
            "gitdir: {}\n",
            main_root
                .join(".git")
                .join("worktrees")
                .join("feature")
                .display()
        ),
    )
    .unwrap();
    let main_db = main_root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(main_root.clone()), &main_db, true).expect("Scan failed");
    {
        let conn = rusqlite::Connection::open(&main_db).unwrap();
        conn.execute_batch(
            "UPDATE artifact_metadata SET value = '0.0.1' WHERE key = 'binary_version';
             UPDATE extraction_revisions SET binary_version = '0.0.1';",
        )
        .unwrap();
    }

    let wt_db = wt_root.join(".code-kb").join("artifact.db");
    create_index(&Workspace::new(wt_root.clone()), &wt_db).unwrap();

    let conn = open_read_only(&wt_db).unwrap();
    let stale: i64 = conn
        .query_row(
            "SELECT count(*) FROM files f
             JOIN extraction_revisions r ON r.revision_id = f.last_revision_id
             WHERE r.binary_version != ?1",
            [installed_extractor_version()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
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

fn scanned_calc_index(root: &std::path::Path, db_path: &std::path::Path) -> Workspace {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src").join("calc.rs"),
        "pub fn foo_fn() -> i32 {\n    100\n}\n",
    )
    .unwrap();
    let ws = Workspace::new(root.to_path_buf());
    scan_workspace(&ws, db_path, true).expect("Scan failed");
    ws
}

fn recorded_binary_version(db_path: &std::path::Path) -> String {
    open_read_only(db_path)
        .unwrap()
        .query_row(
            "SELECT value FROM artifact_metadata WHERE key = 'binary_version'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn test_index_written_by_a_newer_extractor_is_not_rebuilt_by_an_older_one() {
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let db_path = root.join(".code-kb").join("artifact.db");
    let ws = scanned_calc_index(&root, &db_path);
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE artifact_metadata SET value = '99.0.0' WHERE key = 'binary_version'",
            [],
        )
        .unwrap();
    }

    assert!(
        !ensure_index_matches_extractor(&ws, &db_path, &installed_extractor_version()).unwrap()
    );

    assert_eq!(recorded_binary_version(&db_path), "99.0.0");
}

#[test]
fn test_file_rows_from_a_newer_extractor_block_a_rebuild_by_an_older_one() {
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let db_path = root.join(".code-kb").join("artifact.db");
    let ws = scanned_calc_index(&root, &db_path);
    let installed = installed_extractor_version();
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE extraction_revisions SET binary_version = '99.0.0'",
            [],
        )
        .unwrap();
    }

    assert!(!ensure_index_matches_extractor(&ws, &db_path, &installed).unwrap());

    let newer_rows: i64 = open_read_only(&db_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM extraction_revisions WHERE binary_version = '99.0.0'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(newer_rows > 0);
}

#[test]
fn test_failed_rebuild_keeps_the_previous_index() {
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("repo");
    let db_path = temp_dir.path().join("index").join("artifact.db");
    let ws = scanned_calc_index(&root, &db_path);
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE artifact_metadata SET value = '0.0.1' WHERE key = 'binary_version'",
            [],
        )
        .unwrap();
    }
    fs::remove_dir_all(&root).unwrap();

    assert!(ensure_index_matches_extractor(&ws, &db_path, &installed_extractor_version()).is_err());

    assert_eq!(recorded_binary_version(&db_path), "0.0.1");
    let conn = open_read_only(&db_path).unwrap();
    assert!(get_symbol_by_name(&conn, "foo_fn", None).unwrap().is_some());
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
