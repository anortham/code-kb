use code_kb_core::{
    Workspace, find_julie_extract_binary, open_read_only, replace_symbol_body, scan_workspace,
    slicer,
};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn test_replace_symbol_body_atomic() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = tempfile::tempdir().unwrap();
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

    // Scan the workspace
    scan_workspace(&ws, &db_path, true).expect("Scan failed");

    // Open connection
    let conn = open_read_only(&db_path).unwrap();

    let new_body = "{\n    let sum = a + b;\n    sum * 2\n}";
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

    // Verify disk content
    let disk_content = fs::read_to_string(&file_path).unwrap();
    assert!(disk_content.contains("sum * 2"));
    assert!(disk_content.contains("pub fn add_numbers(a: i32, b: i32) -> i32"));

    // Verify database was updated
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

    let temp_dir = tempfile::tempdir().unwrap();
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

    let temp_dir = tempfile::tempdir().unwrap();
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

    let temp_dir = tempfile::tempdir().unwrap();
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

    let temp_dir = tempfile::tempdir().unwrap();
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

    let temp_dir = tempfile::tempdir().unwrap();
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

    let temp_dir = tempfile::tempdir().unwrap();
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

    let temp_dir = tempfile::tempdir().unwrap();
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
