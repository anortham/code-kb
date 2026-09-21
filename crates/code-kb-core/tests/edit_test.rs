use code_kb_core::{
    MatchTier, Occurrence, Workspace, edit_file, find_julie_extract_binary, open_read_only,
    replace_symbol_body, safe_tempdir, scan_workspace, slicer,
};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn test_replace_symbol_body_atomic() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    // Create a sample rust file
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");

    let initial_code = r#"pub fn add_numbers(a: i32, b: i32) -> i32 {
    a + b
}
"#;
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let new_body = r#"{
    let sum = a + b;
    sum * 2
}"#;

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "add_numbers",
        "src/calc.rs",
        new_body,
        None,
    )
    .expect("replace_symbol_body failed");

    assert_eq!(res.symbol_name, "add_numbers");
    assert_eq!(res.file_path, "src/calc.rs");

    // Verify on disk
    let disk_content = fs::read_to_string(&file_path).unwrap();
    assert!(disk_content.contains("let sum = a + b;"));
    assert!(disk_content.contains("sum * 2"));

    // Verify in db
    let updated_symbol =
        code_kb_core::get_symbol_by_name(&conn, "add_numbers", Some("src/calc.rs"))
            .unwrap()
            .unwrap();

    let body = slicer::slice_symbol_body(&file_path, &updated_symbol).unwrap();
    assert_eq!(body.trim(), new_body.trim());
}

#[test]
fn test_replace_symbol_body_shrunk_file_does_not_panic() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");

    let initial_code = r#"pub fn long_function_name(a: i32, b: i32) -> i32 {
    let mut x = a * 2;
    let mut y = b * 3;
    x + y
}
"#;
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Now shrink the file to just 5 bytes without updating the database
    fs::write(&file_path, "short").unwrap();

    // Calling replace_symbol_body must NOT panic! It must return a clean Err
    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "long_function_name",
        "src/calc.rs",
        "{\n    42\n}",
        None,
    );

    assert!(
        res.is_err(),
        "Replacing in shrunk file must return an Err, not panic"
    );
}

#[test]
fn test_replace_symbol_body_rejects_syntax_error() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");

    let initial_code = r#"pub fn my_calc(a: i32) -> i32 {
    a * 2
}
"#;
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Attempt replacing body with invalid Rust syntax (e.g. missing operand, syntax error)
    let invalid_body = "{\n    let x = ;\n}";
    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "my_calc",
        "src/calc.rs",
        invalid_body,
        None,
    );

    assert!(
        res.is_err(),
        "Replacement with invalid syntax must be rejected"
    );

    // File on disk must remain uncorrupted and unchanged!
    let disk_content = fs::read_to_string(&file_path).unwrap();
    assert_eq!(
        disk_content, initial_code,
        "Disk content must not be modified when syntax error occurs"
    );
}

#[test]
fn test_replace_symbol_body_rejects_stale_indexed_hash_when_disk_differs() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");

    let initial_code = "pub fn my_calc(a: i32) -> i32 {\n    a * 2\n}\n";
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let sym = code_kb_core::get_symbol_by_name_exact(&conn, "my_calc", "src/calc.rs")
        .unwrap()
        .unwrap();
    let indexed_hash = sym
        .body_hash
        .clone()
        .expect("Indexed symbol should have body_hash");

    // Manually edit the file on disk so the body is different from indexed state,
    // and keep file length same or different
    let offline_code = "pub fn my_calc(a: i32) -> i32 {\n    a * 9\n}\n";
    fs::write(&file_path, offline_code).unwrap();

    // Calling replace_symbol_body with expected_hash matching the OLD indexed hash
    // MUST fail with HashMismatch because the disk content is no longer that hash!
    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "my_calc",
        "src/calc.rs",
        "{\n    a * 10\n}",
        Some(&indexed_hash),
    );

    assert!(
        matches!(res, Err(code_kb_core::EditError::HashMismatch(..))),
        "Must reject edit when expected hash does not match current disk body, got: {:?}",
        res
    );
}

#[cfg(unix)]
#[test]
fn test_replace_symbol_body_preserves_file_permissions() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("script.rs");

    let initial_code = "pub fn run_script() -> i32 {\n    1\n}\n";
    fs::write(&file_path, initial_code).unwrap();

    // Set 0755 executable permissions
    fs::set_permissions(&file_path, fs::Permissions::from_mode(0o755)).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "run_script",
        "src/script.rs",
        "{\n    42\n}",
        None,
    );
    assert!(res.is_ok(), "replace_symbol_body should succeed: {:?}", res);

    // Verify permissions were preserved (not clobbered to 0600 by tempfile)
    let perms = fs::metadata(&file_path).unwrap().permissions();
    assert_eq!(
        perms.mode() & 0o777,
        0o755,
        "Permissions must remain 0755 after edit, got: {:o}",
        perms.mode() & 0o777
    );
}

#[cfg(unix)]
#[test]
fn test_replace_symbol_body_preserves_symlinks() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let real_file = src_dir.join("real.rs");
    let link_file = src_dir.join("link.rs");

    let initial_code = "pub fn linked_fn() -> i32 {\n    10\n}\n";
    fs::write(&real_file, initial_code).unwrap();
    std::os::unix::fs::symlink(&real_file, &link_file).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Edit through the symlink path
    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "linked_fn",
        "src/link.rs",
        "{\n    99\n}",
        None,
    );
    assert!(res.is_ok(), "replace_symbol_body should succeed: {:?}", res);

    // Verify link.rs is STILL a symlink
    let sym_meta = fs::symlink_metadata(&link_file).unwrap();
    assert!(
        sym_meta.is_symlink(),
        "Symlink must NOT be replaced by a regular file"
    );

    // Verify real file received the edit
    let real_content = fs::read_to_string(&real_file).unwrap();
    assert!(
        real_content.contains("99"),
        "Real file must contain edited content"
    );
}

#[test]
fn test_replace_symbol_body_normalizes_crlf() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("crlf.rs");

    // File with CRLF line endings
    let initial_code = "pub fn crlf_fn() -> i32 {\r\n    1\r\n}\r\n";
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Pass replacement body with LF only
    let new_body = "{\n    let a = 10;\n    a * 2\n}";
    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "crlf_fn",
        "src/crlf.rs",
        new_body,
        None,
    );
    assert!(res.is_ok(), "replace_symbol_body should succeed: {:?}", res);

    // Verify disk content preserves CRLF consistently throughout
    let disk_bytes = fs::read(&file_path).unwrap();
    let disk_str = String::from_utf8(disk_bytes.clone()).unwrap();
    assert!(
        disk_str.contains("\r\n"),
        "File must maintain CRLF line endings"
    );

    // Check there are no bare LF (\n without \r before it)
    let mut prev_char = ' ';
    for ch in disk_str.chars() {
        if ch == '\n' {
            assert_eq!(
                prev_char, '\r',
                "Every newline in CRLF file must be preceded by carriage return (CRLF)"
            );
        }
        prev_char = ch;
    }
}

#[test]
fn test_replace_symbol_body_chained_edits_with_expected_hash() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");

    let initial_code = "pub fn add_numbers(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // 1. First edit: no expected hash passed
    let res1 = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "add_numbers",
        "src/calc.rs",
        "{\n    let sum = a + b;\n    sum * 2\n}",
        None,
    )
    .expect("first edit should succeed");

    assert!(!res1.new_body_hash.is_empty());

    // 2. Second edit: passing valid expected_hash matching res1.new_body_hash
    let res2 = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "add_numbers",
        "src/calc.rs",
        "{\n    let sum = a + b;\n    sum * 3\n}",
        Some(&res1.new_body_hash),
    )
    .expect("second edit with correct expected_hash should succeed");

    assert_ne!(res1.new_body_hash, res2.new_body_hash);

    // 3. Third edit: passing stale expected_hash (res1.new_body_hash) must fail
    let res3 = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "add_numbers",
        "src/calc.rs",
        "{\n    let sum = a + b;\n    sum * 4\n}",
        Some(&res1.new_body_hash),
    );

    assert!(res3.is_err(), "Edit with stale expected_hash must fail");
    let err_msg = res3.unwrap_err().to_string();
    assert!(err_msg.contains("Optimistic lock failed") || err_msg.contains("expected body hash"));
}

#[test]
fn test_replace_symbol_body_rejects_syntax_error_in_a_language_without_a_bundled_grammar() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    fs::create_dir_all(root.join("lib")).unwrap();
    let file_path = root.join("lib/calc.rb");
    let initial_code = "def my_calc(a)\n  a * 2\nend\n";
    fs::write(&file_path, initial_code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let res = replace_symbol_body(
        &ws,
        &db_path,
        &conn,
        "my_calc",
        "lib/calc.rb",
        "def my_calc(a)\n  a * (2\nend\n",
        None,
    );

    let message = res.expect_err("broken Ruby must be rejected").to_string();
    assert!(message.contains("syntax"), "{message}");
    assert!(message.contains("at line "), "{message}");
    assert_eq!(fs::read_to_string(&file_path).unwrap(), initial_code);
}

#[test]
fn validate_syntax_skips_paths_the_extractor_has_no_grammar_for() {
    assert_eq!(
        code_kb_core::validate_syntax("notes.unknown", "anything (\n"),
        Ok(false)
    );
    assert_eq!(
        code_kb_core::validate_syntax("src/lib.rs", "pub fn f() {}\n"),
        Ok(true)
    );
}

fn scan_into(root: &std::path::Path, files: &[(&str, &str)]) -> (Workspace, std::path::PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    for (rel, content) in files {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
    }
    let ws = Workspace::new(root.to_path_buf());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    (ws, db_path)
}

#[test]
fn test_edit_file_replaces_one_exact_match() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[(
            "src/calc.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )],
    );
    let conn = open_read_only(&db_path).unwrap();

    let res = edit_file(
        &ws,
        &db_path,
        &conn,
        "src/calc.rs",
        "a + b",
        "a - b",
        Occurrence::Only,
    )
    .expect("edit_file must succeed");

    assert_eq!(res.file_path, "src/calc.rs");
    assert_eq!(res.replacements, 1);
    assert_eq!(res.first_line, 2);
    assert_eq!(res.match_tier, MatchTier::Exact);
    assert!(res.syntax_checked);
    assert_eq!(
        fs::read_to_string(dir.path().join("src/calc.rs")).unwrap(),
        "pub fn add(a: i32, b: i32) -> i32 {\n    a - b\n}\n"
    );
}

#[test]
fn test_edit_file_matches_ignoring_indentation() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[(
            "src/calc.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    let sum = a + b;\n    sum\n}\n",
        )],
    );
    let conn = open_read_only(&db_path).unwrap();

    let res = edit_file(
        &ws,
        &db_path,
        &conn,
        "src/calc.rs",
        "let sum = a + b;\nsum",
        "let sum = a - b;\nsum",
        Occurrence::Only,
    )
    .expect("edit_file must succeed");

    assert_eq!(res.match_tier, MatchTier::Whitespace);
    assert_eq!(res.replacements, 1);
    assert_eq!(res.first_line, 2);
    assert_eq!(
        fs::read_to_string(dir.path().join("src/calc.rs")).unwrap(),
        "pub fn add(a: i32, b: i32) -> i32 {\n    let sum = a - b;\n    sum\n}\n"
    );
}

#[test]
fn test_edit_file_refuses_ambiguous_match_and_lists_lines() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[(
            "src/calc.rs",
            "pub fn a() -> i32 {\n    1 + 1\n}\n\npub fn b() -> i32 {\n    1 + 1\n}\n",
        )],
    );
    let conn = open_read_only(&db_path).unwrap();

    let err = edit_file(
        &ws,
        &db_path,
        &conn,
        "src/calc.rs",
        "1 + 1",
        "2 + 2",
        Occurrence::Only,
    )
    .expect_err("two matches must be refused");

    match &err {
        code_kb_core::EditError::AmbiguousMatch(path, lines) => {
            assert_eq!(path, "src/calc.rs");
            assert_eq!(lines, &vec![2, 6]);
        }
        other => panic!("expected AmbiguousMatch, got {other:?}"),
    }
    let message = err.to_string();
    assert!(message.contains("2"), "{message}");
    assert!(message.contains("6"), "{message}");
    assert!(message.contains("occurrence"), "{message}");
    assert!(
        fs::read_to_string(dir.path().join("src/calc.rs"))
            .unwrap()
            .contains("1 + 1")
    );
}

#[test]
fn test_edit_file_occurrence_all_replaces_every_match() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[(
            "src/calc.rs",
            "pub fn a() -> i32 {\n    1 + 1\n}\n\npub fn b() -> i32 {\n    1 + 1\n}\n",
        )],
    );
    let conn = open_read_only(&db_path).unwrap();

    let res = edit_file(
        &ws,
        &db_path,
        &conn,
        "src/calc.rs",
        "1 + 1",
        "2 + 2",
        Occurrence::All,
    )
    .expect("edit_file must succeed");

    assert_eq!(res.replacements, 2);
    assert_eq!(res.first_line, 2);
    assert_eq!(
        fs::read_to_string(dir.path().join("src/calc.rs")).unwrap(),
        "pub fn a() -> i32 {\n    2 + 2\n}\n\npub fn b() -> i32 {\n    2 + 2\n}\n"
    );
    assert_eq!(res.touched_symbols, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn test_edit_file_rejects_syntax_error_and_leaves_file_unchanged() {
    let dir = safe_tempdir();
    let initial = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    let (ws, db_path) = scan_into(dir.path(), &[("src/calc.rs", initial)]);
    let conn = open_read_only(&db_path).unwrap();

    let err = edit_file(
        &ws,
        &db_path,
        &conn,
        "src/calc.rs",
        "a + b",
        "a + ",
        Occurrence::Only,
    )
    .expect_err("broken Rust must be rejected");

    assert!(err.to_string().contains("syntax"), "{err}");
    assert_eq!(
        fs::read_to_string(dir.path().join("src/calc.rs")).unwrap(),
        initial
    );
}

#[test]
fn test_edit_file_preserves_crlf() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[("src/crlf.rs", "pub fn crlf_fn() -> i32 {\r\n    1\r\n}\r\n")],
    );
    let conn = open_read_only(&db_path).unwrap();

    edit_file(
        &ws,
        &db_path,
        &conn,
        "src/crlf.rs",
        "    1\n",
        "    let a = 2;\n    a\n",
        Occurrence::Only,
    )
    .expect("edit_file must succeed");

    let disk = fs::read_to_string(dir.path().join("src/crlf.rs")).unwrap();
    assert_eq!(
        disk, "pub fn crlf_fn() -> i32 {\r\n    let a = 2;\r\n    a\r\n}\r\n",
        "CRLF endings must survive the edit"
    );
}

#[test]
fn test_edit_file_reindexes_shifted_symbol_offsets() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[(
            "src/calc.rs",
            "pub fn head() -> i32 {\n    1\n}\n\npub fn tail() -> i32 {\n    2\n}\n",
        )],
    );
    let conn = open_read_only(&db_path).unwrap();

    edit_file(
        &ws,
        &db_path,
        &conn,
        "src/calc.rs",
        "pub fn head() -> i32 {\n    1\n}",
        "pub fn head() -> i32 {\n    let x = 1;\n    x\n}",
        Occurrence::Only,
    )
    .expect("edit_file must succeed");

    let tail = code_kb_core::get_symbol_by_name(&conn, "tail", Some("src/calc.rs"))
        .unwrap()
        .unwrap();
    assert_eq!(tail.start_line, 6);
}

#[test]
fn test_edit_file_edits_an_unsupported_text_file() {
    let readme = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("README.md");
    let readme_text = fs::read_to_string(&readme).expect("repository README.md must be readable");
    let heading = "## Why code-kb?";
    assert_eq!(readme_text.matches(heading).count(), 1);

    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[
            ("notes.txt", "hello\nworld\n"),
            ("README.md", readme_text.as_str()),
        ],
    );
    let conn = open_read_only(&db_path).unwrap();

    let text = edit_file(
        &ws,
        &db_path,
        &conn,
        "notes.txt",
        "world",
        "there",
        Occurrence::Only,
    )
    .expect("editing a text file must succeed");
    assert!(!text.syntax_checked);
    assert!(text.touched_symbols.is_empty());
    assert_eq!(
        fs::read_to_string(dir.path().join("notes.txt")).unwrap(),
        "hello\nthere\n"
    );

    edit_file(
        &ws,
        &db_path,
        &conn,
        "README.md",
        heading,
        "## Why use code-kb?",
        Occurrence::Only,
    )
    .expect("editing a Markdown file must succeed");
    assert!(
        fs::read_to_string(dir.path().join("README.md"))
            .unwrap()
            .contains("## Why use code-kb?")
    );
}

#[test]
fn test_edit_file_preserves_file_permissions() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[("src/script.rs", "pub fn run_script() -> i32 {\n    1\n}\n")],
    );
    let conn = open_read_only(&db_path).unwrap();
    let target = dir.path().join("src/script.rs");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o755)).unwrap();

    edit_file(
        &ws,
        &db_path,
        &conn,
        "src/script.rs",
        "    1\n",
        "    42\n",
        Occurrence::Only,
    )
    .expect("edit_file must succeed");

    let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "permissions must stay 0755, got {mode:o}");
}

#[test]
fn test_edit_file_reports_touched_symbols() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[(
            "src/item.rs",
            "pub struct Item;\n\nimpl Item {\n    pub fn get(&self) -> i32 {\n        1\n    }\n}\n",
        )],
    );
    let conn = open_read_only(&db_path).unwrap();

    let res = edit_file(
        &ws,
        &db_path,
        &conn,
        "src/item.rs",
        "        1\n",
        "        2\n",
        Occurrence::Only,
    )
    .expect("edit_file must succeed");

    assert_eq!(res.touched_symbols, vec!["get".to_string()]);
}

#[test]
fn test_edit_file_rejects_empty_old_text_and_a_no_op_edit() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[("src/calc.rs", "pub fn add() -> i32 {\n    1\n}\n")],
    );
    let conn = open_read_only(&db_path).unwrap();

    assert!(matches!(
        edit_file(
            &ws,
            &db_path,
            &conn,
            "src/calc.rs",
            "",
            "x",
            Occurrence::Only
        ),
        Err(code_kb_core::EditError::EmptyOldText)
    ));
    assert!(matches!(
        edit_file(
            &ws,
            &db_path,
            &conn,
            "src/calc.rs",
            "    1\n",
            "    1\n",
            Occurrence::Only
        ),
        Err(code_kb_core::EditError::NoChange)
    ));
    assert!(matches!(
        edit_file(&ws, &db_path, &conn, "src", "a", "b", Occurrence::Only),
        Err(code_kb_core::EditError::NotAFile(_))
    ));
}

#[test]
fn test_edit_file_reports_the_nearest_lines_when_nothing_matches() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[(
            "src/calc.rs",
            "pub fn add(a: i32, b: i32) -> i32 {\n    let sum = a + b;\n    sum\n}\n",
        )],
    );
    let conn = open_read_only(&db_path).unwrap();

    let err = edit_file(
        &ws,
        &db_path,
        &conn,
        "src/calc.rs",
        "let sum = a * b;",
        "let sum = a / b;",
        Occurrence::Only,
    )
    .expect_err("a missing text must be refused");

    let message = err.to_string();
    assert!(message.contains("src/calc.rs"), "{message}");
    assert!(message.contains("let sum = a + b;"), "{message}");
    assert!(message.contains("2"), "{message}");
}

#[test]
fn test_edit_file_touched_symbols_prefer_the_enclosing_definition_over_a_local() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(
        dir.path(),
        &[(
            "src/run.rs",
            "pub fn run() -> i32 {\n    let total = 1 + 1;\n    total\n}\n",
        )],
    );
    let conn = open_read_only(&db_path).unwrap();

    let res = edit_file(
        &ws,
        &db_path,
        &conn,
        "src/run.rs",
        "1 + 1",
        "2 + 2",
        Occurrence::Only,
    )
    .expect("edit_file must succeed");

    assert_eq!(res.touched_symbols, vec!["run".to_string()]);
}

#[test]
fn test_edit_file_overlapping_matches_are_ambiguous_and_last_picks_the_last_start() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(dir.path(), &[("notes/word.txt", "banana\n")]);
    let conn = open_read_only(&db_path).unwrap();

    let err = edit_file(
        &ws,
        &db_path,
        &conn,
        "notes/word.txt",
        "ana",
        "X",
        Occurrence::Only,
    )
    .expect_err("overlapping matches must be refused");
    match &err {
        code_kb_core::EditError::AmbiguousMatch(_, lines) => assert_eq!(lines, &vec![1, 1]),
        other => panic!("expected AmbiguousMatch, got {other:?}"),
    }

    let res = edit_file(
        &ws,
        &db_path,
        &conn,
        "notes/word.txt",
        "ana",
        "X",
        Occurrence::Last,
    )
    .expect("last must pick the last start");
    assert_eq!(res.replacements, 1);
    assert_eq!(
        fs::read_to_string(dir.path().join("notes/word.txt")).unwrap(),
        "banX\n"
    );
}

#[test]
fn test_edit_file_occurrence_all_replaces_non_overlapping_matches_only() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(dir.path(), &[("notes/word.txt", "aaaa\n")]);
    let conn = open_read_only(&db_path).unwrap();

    let res = edit_file(
        &ws,
        &db_path,
        &conn,
        "notes/word.txt",
        "aa",
        "b",
        Occurrence::All,
    )
    .expect("all must replace the non-overlapping matches");
    assert_eq!(res.replacements, 2);
    assert_eq!(
        fs::read_to_string(dir.path().join("notes/word.txt")).unwrap(),
        "bb\n"
    );
}

#[test]
fn test_edit_file_overlapping_line_windows_are_ambiguous() {
    let dir = safe_tempdir();
    let (ws, db_path) = scan_into(dir.path(), &[("notes/list.txt", "  x\n  x\n  x\n")]);
    let conn = open_read_only(&db_path).unwrap();

    let err = edit_file(
        &ws,
        &db_path,
        &conn,
        "notes/list.txt",
        "x\nx",
        "y",
        Occurrence::Only,
    )
    .expect_err("two line windows must be refused");
    match &err {
        code_kb_core::EditError::AmbiguousMatch(_, lines) => assert_eq!(lines, &vec![1, 2]),
        other => panic!("expected AmbiguousMatch, got {other:?}"),
    }
}

#[test]
fn test_edit_file_many_matches_stay_cheap_and_a_too_large_result_is_refused() {
    let dir = safe_tempdir();
    let big = "a".repeat(1024 * 1024);
    let (ws, db_path) = scan_into(dir.path(), &[("notes/big.txt", &big)]);
    let conn = open_read_only(&db_path).unwrap();
    let replacement = "b".repeat(4096);
    let started = std::time::Instant::now();

    let err = edit_file(
        &ws,
        &db_path,
        &conn,
        "notes/big.txt",
        "a",
        &replacement,
        Occurrence::Only,
    )
    .expect_err("a million matches must be refused as ambiguous");
    let message = err.to_string();
    assert!(message.contains("and 1048566 more"), "{message}");
    assert!(message.len() < 400, "{message}");

    let err = edit_file(
        &ws,
        &db_path,
        &conn,
        "notes/big.txt",
        "a",
        &replacement,
        Occurrence::All,
    )
    .expect_err("a result above the size cap must be refused");
    assert!(
        matches!(err, code_kb_core::EditError::EditTooLarge),
        "{err:?}"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("notes/big.txt")).unwrap(),
        big
    );

    let res = edit_file(
        &ws,
        &db_path,
        &conn,
        "notes/big.txt",
        "a",
        &replacement,
        Occurrence::First,
    )
    .expect("first must succeed");
    assert_eq!(res.bytes_written, big.len() - 1 + replacement.len());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}
