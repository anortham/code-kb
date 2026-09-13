use std::fs;
use code_kb_core::{
    find_julie_extract_binary, get_symbol_by_name, open_read_only, scan_workspace, Workspace,
};

#[test]
fn test_qualified_parent_disambiguation() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("types.rs");

    let code = r#"
pub struct Alpha;
impl Alpha {
    pub fn create() -> Alpha {
        Alpha
    }
}

pub struct Beta;
impl Beta {
    pub fn create() -> Beta {
        Beta
    }
}
"#;
    fs::write(&file_path, code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Query Alpha::create
    let alpha_create = get_symbol_by_name(&conn, "Alpha::create", Some("src/types.rs"))
        .expect("Query failed")
        .expect("Alpha::create must be found");

    assert_eq!(alpha_create.name, "create");
    // Line 4 is Alpha's create, not Beta's create (line 11)
    assert_eq!(alpha_create.start_line, 4, "Must match Alpha::create on line 4");

    // Query Beta::create
    let beta_create = get_symbol_by_name(&conn, "Beta::create", Some("src/types.rs"))
        .expect("Query failed")
        .expect("Beta::create must be found");

    assert_eq!(beta_create.name, "create");
    assert_eq!(beta_create.start_line, 11, "Must match Beta::create on line 11");
}

#[test]
fn test_path_filter_boundary_matching() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    
    // Two files: domain.rs and main.rs, both with a function named run_it
    fs::write(src_dir.join("domain.rs"), "pub fn run_it() {}\n").unwrap();
    fs::write(src_dir.join("main.rs"), "pub fn run_it() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Query with filter "main.rs" must NOT match "domain.rs"
    let sym = get_symbol_by_name(&conn, "run_it", Some("main.rs"))
        .expect("Query failed")
        .expect("run_it in main.rs must be found");

    assert_eq!(sym.path, "src/main.rs");
}

#[test]
fn test_ambiguous_symbol_detection() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let code = r#"
pub struct Alpha;
impl Alpha {
    pub fn do_work() {}
}

pub struct Beta;
impl Beta {
    pub fn do_work() {}
}
"#;
    fs::write(src_dir.join("work.rs"), code).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Querying unqualified "do_work" without parent when two exist in the file must return an AmbiguousSymbol error
    let res = get_symbol_by_name(&conn, "do_work", Some("src/work.rs"));
    match res {
        Err(code_kb_core::QueryError::AmbiguousSymbol(name, count, _)) => {
            assert_eq!(name, "do_work");
            assert_eq!(count, 2);
        }
        other => panic!("Expected AmbiguousSymbol error, got: {:?}", other),
    }
}

#[test]
fn test_file_skeleton_exact_path_isolation() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping integration test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src1 = root.join("src");
    let src2 = root.join("nested").join("src");
    fs::create_dir_all(&src1).unwrap();
    fs::create_dir_all(&src2).unwrap();

    fs::write(src1.join("lib.rs"), "pub fn root_lib_func() {}\n").unwrap();
    fs::write(src2.join("lib.rs"), "pub fn nested_lib_func() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let skeleton = code_kb_core::file_skeleton_op(&ws, &db_path, &conn, "src/lib.rs")
        .expect("file_skeleton_op failed");

    assert!(
        skeleton.contains("root_lib_func"),
        "Skeleton for src/lib.rs must contain root_lib_func, got:\n{skeleton}"
    );
    assert!(
        !skeleton.contains("nested_lib_func"),
        "Skeleton for src/lib.rs must NOT contain nested_lib_func from nested/src/lib.rs, got:\n{skeleton}"
    );
}
