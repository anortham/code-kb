use code_kb_core::{
    Workspace, find_julie_extract_binary, get_context_slice_op, get_symbol_by_name, open_read_only,
    scan_workspace,
};
use std::fs;

#[test]
fn test_qualified_parent_disambiguation() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

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
    assert_eq!(
        alpha_create.start_line, 4,
        "Must match Alpha::create on line 4"
    );

    // Query Beta::create
    let beta_create = get_symbol_by_name(&conn, "Beta::create", Some("src/types.rs"))
        .expect("Query failed")
        .expect("Beta::create must be found");

    assert_eq!(beta_create.name, "create");
    assert_eq!(
        beta_create.start_line, 11,
        "Must match Beta::create on line 11"
    );
}

#[test]
fn test_path_filter_boundary_matching() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

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
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

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
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

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

#[test]
fn test_context_slice_qualified_method_uses_its_own_callees() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("types.rs"),
        "pub struct A;\n\
         impl A { pub fn new() -> A { wanted_dep(); A } }\n\
         pub struct B;\n\
         impl B { pub fn new() -> B { other_dep(); B } }\n\
         pub fn wanted_dep() {}\n\
         pub fn other_dep() {}\n",
    )
    .unwrap();

    let workspace = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&workspace, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let slice = get_context_slice_op(&workspace, &db_path, &conn, "A::new", Some("src/types.rs"))
        .expect("context slice failed");

    assert!(
        slice
            .callee_signatures
            .iter()
            .any(|signature| signature.contains("wanted_dep"))
    );
    assert!(
        !slice
            .callee_signatures
            .iter()
            .any(|signature| signature.contains("other_dep"))
    );
}

#[test]
fn test_get_symbol_by_name_not_crowded_out_by_imports() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // 1 definition
    fs::write(
        src_dir.join("target.rs"),
        "pub struct TargetConfig {\n    pub x: i32,\n}\n",
    )
    .unwrap();

    // 12 files importing TargetConfig
    for i in 1..=12 {
        fs::write(
            src_dir.join(format!("user_{}.rs", i)),
            format!(
                "use crate::target::TargetConfig;\npub fn user_{}() {{}}\n",
                i
            ),
        )
        .unwrap();
    }

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Query without path filter; imports must NOT crowd out the struct definition!
    let sym = get_symbol_by_name(&conn, "TargetConfig", None)
        .expect("Query failed")
        .expect("TargetConfig struct definition must be found");

    assert_eq!(sym.kind, "struct");
    assert_eq!(sym.path, "src/target.rs");
}

#[test]
fn test_context_slice_finds_related_tests() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        src_dir.join("calc.rs"),
        "pub fn calculate_price(base: i32) -> i32 {\n    base * 2\n}\n",
    )
    .unwrap();

    // Create 6 non-test functions matching "calculate_price" to saturate general search
    for i in 1..=6 {
        fs::write(
            src_dir.join(format!("other_{}.rs", i)),
            format!("pub fn calculate_price_helper_{}() -> i32 {{ {} }}\n", i, i),
        )
        .unwrap();
    }

    // Create test referencing calculate_price
    let tests_dir = root.join("tests");
    fs::create_dir_all(&tests_dir).unwrap();
    fs::write(
        tests_dir.join("calc_test.rs"),
        "#[test]\nfn test_calculate_price() {\n    let _ = calculate_price(5);\n}\n",
    )
    .unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let slice = get_context_slice_op(&ws, &db_path, &conn, "calculate_price", Some("src/calc.rs"))
        .expect("get_context_slice_op failed");

    assert!(
        slice
            .related_tests
            .iter()
            .any(|t| t.name.contains("calculate_price") || t.is_test),
        "Context slice must find related test, got: {:?}",
        slice
            .related_tests
            .iter()
            .map(|t| &t.name)
            .collect::<Vec<_>>()
    );
}
