use code_kb_core::{
    Workspace, find_julie_extract_binary, open_read_only, replace_symbol_body, scan_workspace,
    slicer,
};
use std::fs;

#[test]
fn test_replace_symbol_body_atomic() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

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
