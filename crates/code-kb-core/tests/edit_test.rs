use std::fs;
use code_kb_core::{
    find_julie_extract_binary, open_read_only, replace_symbol_body, scan_workspace,
    slicer, Workspace,
};

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
    let updated_symbol = code_kb_core::get_symbol_by_name(&conn, "add_numbers", Some("src/calc.rs"))
        .unwrap()
        .unwrap();

    let body = slicer::slice_symbol_body(&file_path, &updated_symbol).unwrap();
    assert_eq!(body.trim(), new_body.trim());
}
