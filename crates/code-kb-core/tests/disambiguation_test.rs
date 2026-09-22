use code_kb_core::{
    Workspace, ensure_fts_index, find_callee_signatures, find_julie_extract_binary,
    find_literals_scoped, find_references_scoped, find_structural_facts_scoped,
    format_fact_categories, format_structural_facts, get_context_slice_op, get_symbol_by_name,
    list_structural_fact_categories_scoped, open_read_only, open_read_write, safe_tempdir,
    scan_workspace, search_symbols_scoped, suggest_file_paths, suggest_symbol_names,
};
use std::fs;

#[test]
fn test_qualified_parent_disambiguation() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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
fn qualified_method_lookup_prefers_definition_over_same_named_field() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("response.rs"),
        "pub struct Response { pub error: String }\nimpl Response { pub fn error() -> Self { Self { error: String::new() } } }\n",
    )
    .unwrap();

    let workspace = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&workspace, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let symbol = get_symbol_by_name(&conn, "Response::error", Some("src/response.rs"))
        .expect("qualified lookup must not be ambiguous")
        .expect("Response::error must be found");
    assert_eq!(symbol.kind, "method");
}

#[test]
fn test_path_filter_boundary_matching() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // Two files: domain.rs and main.rs, both with a function named run_it
    fs::write(src_dir.join("domain.rs"), "pub fn run_it() {}\n").unwrap();
    fs::write(src_dir.join("main.rs"), "pub fn run_it() {}\n").unwrap();

    let widget = "pub struct Widget;\nimpl Widget {\n    pub fn render() {}\n}\n";
    fs::create_dir_all(src_dir.join("core")).unwrap();
    fs::create_dir_all(src_dir.join("core2")).unwrap();
    fs::write(src_dir.join("core").join("widget.rs"), widget).unwrap();
    fs::write(src_dir.join("core2").join("widget.rs"), widget).unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");

    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Query with filter "main.rs" must NOT match "domain.rs"
    let sym = get_symbol_by_name(&conn, "run_it", Some("main.rs"))
        .expect("Query failed")
        .expect("run_it in main.rs must be found");

    assert_eq!(sym.path, "src/main.rs");

    let in_core = get_symbol_by_name(&conn, "Widget::render", Some("src/core"))
        .expect("Query failed")
        .expect("Widget::render in src/core must be found");
    assert_eq!(in_core.path, "src/core/widget.rs");

    let in_core2 = get_symbol_by_name(&conn, "Widget::render", Some("src/core2"))
        .expect("Query failed")
        .expect("Widget::render in src/core2 must be found");
    assert_eq!(in_core2.path, "src/core2/widget.rs");

    assert!(
        get_symbol_by_name(&conn, "Widget::render", Some("src/cor"))
            .expect("Query failed")
            .is_none()
    );
}

#[test]
fn test_ambiguous_symbol_detection() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
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

    let temp_dir = safe_tempdir();
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

    let temp_dir = safe_tempdir();
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

    let slice = get_context_slice_op(
        &workspace,
        &db_path,
        &conn,
        "A::new",
        Some("src/types.rs"),
        false,
    )
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

    let temp_dir = safe_tempdir();
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

    let temp_dir = safe_tempdir();
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

    let slice = get_context_slice_op(
        &ws,
        &db_path,
        &conn,
        "calculate_price",
        Some("src/calc.rs"),
        false,
    )
    .expect("get_context_slice_op failed");

    assert!(
        slice
            .related_tests
            .iter()
            .any(|t| t.name.contains("calculate_price")),
        "Context slice must find related test, got: {:?}",
        slice
            .related_tests
            .iter()
            .map(|t| &t.name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_context_slice_include_external() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    fs::write(
        src_dir.join("service.py"),
        "from json import dumps\n\ndef helper():\n    return 42\n\ndef execute():\n    helper()\n    print('done')\n    dumps({'a': 1})\n",
    )
    .unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    // Default: include_external = false
    let slice_default = get_context_slice_op(
        &ws,
        &db_path,
        &conn,
        "execute",
        Some("src/service.py"),
        false,
    )
    .expect("get_context_slice_op failed");

    assert!(
        slice_default
            .callee_signatures
            .iter()
            .any(|s| s.contains("helper")),
        "Default slice should include workspace callee helper, got: {:?}",
        slice_default.callee_signatures
    );
    assert!(
        !slice_default
            .callee_signatures
            .iter()
            .any(|s| s.contains("print")),
        "Default slice should filter external call print, got: {:?}",
        slice_default.callee_signatures
    );
    assert!(
        !slice_default
            .callee_signatures
            .iter()
            .any(|s| s.contains("dumps")),
        "Default slice should filter imported external call dumps, got: {:?}",
        slice_default.callee_signatures
    );

    // With include_external = true
    let slice_ext = get_context_slice_op(
        &ws,
        &db_path,
        &conn,
        "execute",
        Some("src/service.py"),
        true,
    )
    .expect("get_context_slice_op failed");

    assert!(
        slice_ext
            .callee_signatures
            .iter()
            .any(|s| s.contains("helper")),
        "Extended slice should include helper, got: {:?}",
        slice_ext.callee_signatures
    );
    assert!(
        slice_ext
            .callee_signatures
            .iter()
            .any(|s| s.contains("print")),
        "Extended slice should include external print, got: {:?}",
        slice_ext.callee_signatures
    );
    assert!(
        slice_ext
            .callee_signatures
            .iter()
            .any(|s| s.contains("dumps")),
        "Extended slice should include imported external dumps, got: {:?}",
        slice_ext.callee_signatures
    );
}

fn setup_test_db(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "CREATE TABLE files (
            file_id TEXT PRIMARY KEY, path TEXT, language TEXT, content_hash TEXT,
            content_bytes INTEGER, line_count INTEGER, indexed_at TEXT
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
            signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
            start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
            start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
            body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
            body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
            semantic_group TEXT, is_test INTEGER, test_container INTEGER
        );
        CREATE TABLE relationships (
            from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT, path TEXT,
            start_line INTEGER, start_column INTEGER
        );
        CREATE TABLE pending_relationships (
            from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
            start_line INTEGER, start_column INTEGER,
            target_receiver TEXT, target_namespace_json TEXT, target_display_name TEXT
        );
        CREATE TABLE type_facts (
            type_fact_id TEXT, symbol_id TEXT, language TEXT, resolved_type TEXT, generic_params_json TEXT
        );
        CREATE TABLE structural_facts (
            structural_fact_id TEXT PRIMARY KEY, file_id TEXT, path TEXT NOT NULL, language TEXT,
            pattern_id TEXT, capture_name TEXT, node_kind TEXT, containing_symbol_id TEXT,
            start_line INTEGER, end_line INTEGER, confidence REAL, metadata_json TEXT
        );
        CREATE TABLE literals (
            literal_id TEXT PRIMARY KEY, file_id TEXT, path TEXT NOT NULL, language TEXT,
            kind TEXT, literal_text TEXT, carrier TEXT, containing_symbol_id TEXT,
            start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
            start_byte INTEGER, end_byte INTEGER
        );",
    )
    .unwrap();
}

#[test]
fn exact_name_lookup_returns_a_test_row_that_substring_lookup_hides() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s1', 'f1', 'tests/alpha_test.rs', 'rust', 'test_alpha', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0),
            ('s2', 'f2', 'src/alpha.rs', 'rust', 'alpha', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);",
    )
    .unwrap();

    let paths = |query: &str| {
        search_symbols_scoped(&conn, query, None, None, false, 10)
            .unwrap()
            .into_iter()
            .map(|s| s.path)
            .collect::<Vec<_>>()
    };

    assert_eq!(paths("test_alpha"), vec!["tests/alpha_test.rs"]);
    assert_eq!(paths("alpha"), vec!["src/alpha.rs"]);
    assert_eq!(paths("alph"), vec!["src/alpha.rs"]);
}

#[test]
fn find_references_scoped_disambiguates_multi_file_symbols() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s1', 'f1', 'src/alpha.rs', 'rust', 'run', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s2', 'f2', 'src/beta.rs', 'rust', 'run', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('c1', 'f3', 'src/caller.rs', 'rust', 'caller_alpha', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO relationships VALUES
            ('c1', 's1', 'calls', 'src/caller.rs', 1, 0);",
    )
    .unwrap();

    let refs =
        find_references_scoped(&conn, "run", "callers", 10, false, Some("src/alpha.rs")).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].from_symbol_name, "caller_alpha");
}

#[test]
fn test_pending_references_preserve_target_identity_with_shared_parent_name() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('w_alpha', 'f1', 'src/alpha.rs', 'rust', 'Worker', 'struct', 'pub struct Worker', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
            ('r_alpha', 'f1', 'src/alpha.rs', 'rust', 'run', 'method', 'pub fn run(&self)', NULL, 'pub', 'w_alpha', 2, 4, 4, 5, 20, 50, 2, 4, 4, 5, 20, 50, 'h1', 'method', 0, 0),
            ('w_beta', 'f2', 'src/beta.rs', 'rust', 'Worker', 'struct', 'pub struct Worker', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
            ('r_beta', 'f2', 'src/beta.rs', 'rust', 'run', 'method', 'pub fn run(&self)', NULL, 'pub', 'w_beta', 2, 4, 4, 5, 20, 50, 2, 4, 4, 5, 20, 50, 'h2', 'method', 0, 0),
            ('c_gamma', 'f3', 'src/gamma.rs', 'rust', 'caller', 'function', 'pub fn caller()', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'function', 0, 0);

        -- Caller calls beta::Worker::run() explicitly
        INSERT INTO pending_relationships (from_symbol_id, target_terminal_name, kind, path, start_line, start_column, target_receiver, target_namespace_json, target_display_name) VALUES
            ('c_gamma', 'run', 'calls', 'src/gamma.rs', 5, 8, NULL, '[\"beta\", \"Worker\"]', 'beta::Worker::run');",
    )
    .unwrap();

    // Query callers for Worker::run scoped to alpha.rs: must be empty because call was to beta::Worker::run
    let refs_alpha =
        find_references_scoped(&conn, "run", "callers", 10, false, Some("src/alpha.rs")).unwrap();
    assert!(
        refs_alpha.is_empty(),
        "Expected 0 callers for alpha.rs, got: {:?}",
        refs_alpha
    );

    // Query callers for Worker::run scoped to beta.rs: must find caller
    let refs_beta =
        find_references_scoped(&conn, "run", "callers", 10, false, Some("src/beta.rs")).unwrap();
    assert_eq!(refs_beta.len(), 1);
    assert_eq!(refs_beta[0].from_symbol_name, "caller");
}

#[test]
fn test_structural_facts_scoped_boundary_matching() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO structural_facts (structural_fact_id, file_id, path, language, pattern_id, capture_name, node_kind, containing_symbol_id, start_line, end_line, confidence) VALUES
            ('sf1', 'f1', 'Cargo.toml', 'toml', 'toml.key_value.v1', 'name', 'table', NULL, 1, 1, 1.0),
            ('sf2', 'f2', 'crates/a/Cargo.toml', 'toml', 'toml.key_value.v1', 'name', 'table', NULL, 1, 1, 1.0),
            ('sf3', 'f3', 'src/api/users.rs', 'rust', 'axum.route.v1', 'get_users', 'function_item', NULL, 1, 1, 1.0),
            ('sf4', 'f4', 'src/api_backup/users.rs', 'rust', 'axum.route.v1', 'get_users', 'function_item', NULL, 1, 1, 1.0);",
    )
    .unwrap();

    // 'Cargo.toml' must not match 'crates/a/Cargo.toml'
    let root_cargo = find_structural_facts_scoped(&conn, "config", Some("Cargo.toml"), 10).unwrap();
    assert_eq!(root_cargo.len(), 1);
    assert_eq!(root_cargo[0].path, "Cargo.toml");

    // 'src/api' must match 'src/api/users.rs' but NOT 'src/api_backup/users.rs'
    let api_routes = find_structural_facts_scoped(&conn, "route", Some("src/api"), 10).unwrap();
    assert_eq!(api_routes.len(), 1);
    assert_eq!(api_routes[0].path, "src/api/users.rs");
}

#[test]
fn test_find_callee_signatures_deduplication_before_cap() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    let mut sql = String::from(
        "INSERT INTO symbols VALUES
            ('s_caller', 'f1', 'src/lib.rs', 'rust', 'caller', 'function', 'pub fn caller()', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'function', 0, 0),
            ('s_h1', 'f1', 'src/lib.rs', 'rust', 'helper_one', 'function', 'pub fn helper_one()', NULL, 'pub', NULL, 11, 0, 20, 0, 101, 200, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'function', 0, 0),
            ('s_h2', 'f1', 'src/lib.rs', 'rust', 'helper_two', 'function', 'pub fn helper_two()', NULL, 'pub', NULL, 21, 0, 30, 0, 201, 300, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'function', 0, 0);\n",
    );
    // 25 calls to helper_one
    for i in 1..=25 {
        sql.push_str(&format!(
            "INSERT INTO relationships VALUES ('s_caller', 's_h1', 'calls', 'src/lib.rs', {i}, 0);\n"
        ));
    }
    // 1 call to helper_two
    sql.push_str(
        "INSERT INTO relationships VALUES ('s_caller', 's_h2', 'calls', 'src/lib.rs', 26, 0);\n",
    );
    conn.execute_batch(&sql).unwrap();

    let sigs = find_callee_signatures(&conn, "caller", "s_caller", 10, false).unwrap();
    // Both helper_one and helper_two must be returned, not crowded out by the 25 calls to helper_one
    assert_eq!(
        sigs.len(),
        2,
        "Expected 2 distinct callee signatures, got: {:?}",
        sigs
    );
    assert!(sigs.iter().any(|s| s.contains("helper_one")));
    assert!(sigs.iter().any(|s| s.contains("helper_two")));
}

#[test]
fn context_slice_finds_a_test_in_another_file_with_an_unrelated_name() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("ledger.rs"),
        "pub fn compute_total(base: i32) -> i32 {\n    base * 2\n}\n",
    )
    .unwrap();

    let tests_dir = root.join("tests");
    fs::create_dir_all(&tests_dir).unwrap();
    fs::write(
        tests_dir.join("ledger_test.rs"),
        "#[test]\nfn doubling_holds_for_positive_input() {\n    let doubled = compute_total(5);\n    assert_eq!(doubled, 10);\n}\n",
    )
    .unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let slice = get_context_slice_op(
        &ws,
        &db_path,
        &conn,
        "compute_total",
        Some("src/ledger.rs"),
        false,
    )
    .expect("get_context_slice_op failed");

    assert!(
        slice
            .related_tests
            .iter()
            .any(|t| t.name == "doubling_holds_for_positive_input"),
        "cross-file caller test must appear, got: {:?}",
        slice
            .related_tests
            .iter()
            .map(|t| &t.name)
            .collect::<Vec<_>>()
    );
}

#[test]
fn context_slice_omits_markdown_code_blocks_from_related_tests() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(
        src_dir.join("ledger.rs"),
        "pub fn compute_total(base: i32) -> i32 {\n    base * 2\n}\n",
    )
    .unwrap();

    let docs_dir = root.join("docs");
    fs::create_dir_all(&docs_dir).unwrap();
    fs::write(
        docs_dir.join("guide.md"),
        "# Guide\n\n```rust\nlet total = compute_total(5);\nassert_eq!(total, 10);\n```\n",
    )
    .unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("test.db");
    scan_workspace(&ws, &db_path, true).expect("Scan failed");
    let conn = open_read_only(&db_path).unwrap();

    let slice = get_context_slice_op(
        &ws,
        &db_path,
        &conn,
        "compute_total",
        Some("src/ledger.rs"),
        false,
    )
    .expect("get_context_slice_op failed");

    assert!(
        slice.related_tests.iter().all(|t| !t.path.ends_with(".md")),
        "markdown code blocks must not be related tests, got: {:?}",
        slice
            .related_tests
            .iter()
            .map(|t| format!("{} [{}]", t.name, t.path))
            .collect::<Vec<_>>()
    );
}

#[test]
fn qualified_lookup_requires_every_ancestor_segment() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('outer', 'f1', 'src/tree.rs', 'rust', 'Outer', 'struct', NULL, NULL, 'pub', NULL, 1, 0, 30, 0, 0, 300, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
            ('inner', 'f1', 'src/tree.rs', 'rust', 'Inner', 'struct', NULL, NULL, 'pub', 'outer', 2, 4, 20, 4, 10, 200, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
            ('run', 'f1', 'src/tree.rs', 'rust', 'run', 'method', 'pub fn run(&self)', NULL, 'pub', 'inner', 3, 8, 6, 8, 20, 60, 3, 8, 6, 8, 20, 60, 'h1', 'method', 0, 0);",
    )
    .unwrap();

    let lookup = |name: &str| {
        get_symbol_by_name(&conn, name, None)
            .unwrap()
            .map(|s| s.symbol_id)
    };

    assert_eq!(lookup("Outer::Inner::run").as_deref(), Some("run"));
    assert_eq!(lookup("Inner::run").as_deref(), Some("run"));
    assert_eq!(lookup("Bogus::Inner::run"), None);
}

#[test]
fn qualified_lookup_with_a_full_chain_still_honors_the_path_filter() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('outer_a', 'f1', 'src/alpha.rs', 'rust', 'Outer', 'struct', NULL, NULL, 'pub', NULL, 1, 0, 30, 0, 0, 300, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
            ('inner_a', 'f1', 'src/alpha.rs', 'rust', 'Inner', 'struct', NULL, NULL, 'pub', 'outer_a', 2, 4, 20, 4, 10, 200, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
            ('run_a', 'f1', 'src/alpha.rs', 'rust', 'run', 'method', 'pub fn run(&self)', NULL, 'pub', 'inner_a', 3, 8, 6, 8, 20, 60, 3, 8, 6, 8, 20, 60, 'h1', 'method', 0, 0),
            ('outer_b', 'f2', 'src/beta.rs', 'rust', 'Outer', 'struct', NULL, NULL, 'pub', NULL, 1, 0, 30, 0, 0, 300, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
            ('inner_b', 'f2', 'src/beta.rs', 'rust', 'Inner', 'struct', NULL, NULL, 'pub', 'outer_b', 2, 4, 20, 4, 10, 200, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
            ('run_b', 'f2', 'src/beta.rs', 'rust', 'run', 'method', 'pub fn run(&self)', NULL, 'pub', 'inner_b', 3, 8, 6, 8, 20, 60, 3, 8, 6, 8, 20, 60, 'h2', 'method', 0, 0);",
    )
    .unwrap();

    let found = get_symbol_by_name(&conn, "Outer::Inner::run", Some("src/beta.rs"))
        .unwrap()
        .expect("the path filter must pick one of the two chains");
    assert_eq!(found.symbol_id, "run_b");
}

fn insert_inner_run_chains(conn: &rusqlite::Connection, decoys: usize, wanted: &[&str]) {
    let mut rows = Vec::new();
    for i in 0..decoys {
        rows.push(format!(
            "('outer{i}', 'f{i}', 'src/decoy{i}.rs', 'rust', 'Other{i}', 'struct', NULL, NULL, 'pub', NULL, 1, 0, 30, 0, 0, 300, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
             ('inner{i}', 'f{i}', 'src/decoy{i}.rs', 'rust', 'Inner', 'struct', NULL, NULL, 'pub', 'outer{i}', 2, 4, 20, 4, 10, 200, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
             ('run{i}', 'f{i}', 'src/decoy{i}.rs', 'rust', 'run', 'method', 'pub fn run(&self)', NULL, 'pub', 'inner{i}', 3, 8, 6, 8, 20, 60, 3, 8, 6, 8, 20, 60, 'h{i}', 'method', 0, 0)"
        ));
    }
    for id in wanted {
        rows.push(format!(
            "('outer_{id}', 'fw_{id}', 'src/{id}.rs', 'rust', 'Wanted', 'struct', NULL, NULL, 'pub', NULL, 1, 0, 30, 0, 0, 300, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
             ('inner_{id}', 'fw_{id}', 'src/{id}.rs', 'rust', 'Inner', 'struct', NULL, NULL, 'pub', 'outer_{id}', 2, 4, 20, 4, 10, 200, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
             ('{id}', 'fw_{id}', 'src/{id}.rs', 'rust', 'run', 'method', 'pub fn run(&self)', NULL, 'pub', 'inner_{id}', 3, 8, 6, 8, 20, 60, 3, 8, 6, 8, 20, 60, 'hw_{id}', 'method', 1, 0)"
        ));
    }
    conn.execute_batch(&format!("INSERT INTO symbols VALUES {};", rows.join(",\n")))
        .unwrap();
}

#[test]
fn qualified_lookup_finds_a_chain_sorted_past_the_row_cap() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    insert_inner_run_chains(&conn, 40, &["wanted_run"]);

    let found = get_symbol_by_name(&conn, "Wanted::Inner::run", None)
        .unwrap()
        .expect("the wanted chain sorts after 40 decoys and must still be found");
    assert_eq!(found.symbol_id, "wanted_run");
}

#[test]
fn qualified_lookup_reports_ambiguity_past_the_row_cap() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    insert_inner_run_chains(&conn, 40, &["wanted_a", "wanted_b"]);

    let err = get_symbol_by_name(&conn, "Wanted::Inner::run", None).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("Ambiguous"), "{text}");
    assert!(text.contains("src/wanted_a.rs"), "{text}");
    assert!(text.contains("src/wanted_b.rs"), "{text}");
}

#[test]
fn a_symbol_typo_suggests_the_closest_indexed_name() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s1', 'f1', 'src/formatters.rs', 'rust', 'format_symbol_body', 'function', NULL, NULL, 'pub', NULL, 10, 0, 20, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s2', 'f2', 'src/server.rs', 'rust', 'handle_call_tool', 'function', NULL, NULL, 'pub', NULL, 5, 0, 9, 0, 0, 80, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);",
    )
    .unwrap();
    ensure_fts_index(&conn).unwrap();

    let names: Vec<String> = suggest_symbol_names(&conn, "format_sybmol_body", None)
        .into_iter()
        .map(|s| s.name)
        .collect();

    assert_eq!(names, ["format_symbol_body"]);
}

#[test]
fn a_wrong_file_path_suggests_the_indexed_file_with_the_same_stem() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO files VALUES
            ('f1', 'crates/code-kb-core/src/formatters.rs', 'rust', 'h1', 100, 10, 't'),
            ('f2', 'crates/code-kb-cli/src/mcp/server.rs', 'rust', 'h2', 100, 10, 't');",
    )
    .unwrap();

    let found = suggest_file_paths(&conn, "crates/code-kb-cli/src/mcp/format.rs");

    assert_eq!(found, ["crates/code-kb-core/src/formatters.rs"]);
}

#[test]
fn a_missing_symbol_with_no_neighbour_names_the_workspace() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s1', 'f1', 'src/formatters.rs', 'rust', 'format_symbol_body', 'function', NULL, NULL, 'pub', NULL, 10, 0, 20, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);",
    )
    .unwrap();
    ensure_fts_index(&conn).unwrap();
    conn.execute(
        "INSERT INTO artifact_metadata (key, value) VALUES ('root_path', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        ["/home/dev/widget-shop"],
    )
    .unwrap();

    let err = find_references_scoped(&conn, "zzzqqq", "callers", 10, false, None).unwrap_err();

    assert_eq!(
        err.to_string(),
        "Symbol 'zzzqqq' not found in widget-shop. No similar name is indexed; check the workspace and spelling."
    );
}

fn facts_alias_fixture(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "INSERT INTO structural_facts (structural_fact_id, file_id, path, language, pattern_id, capture_name, node_kind, containing_symbol_id, start_line, end_line, confidence) VALUES
            ('sf_css', 'f1', 'web/site.css', 'css', 'css.media_query.v1', 'media', 'media_statement', NULL, 1, 1, 1.0),
            ('sf_sql', 'f2', 'db/reports.sql', 'sql', 'sql.select_query.v1', 'select', 'select_statement', NULL, 1, 1, 1.0);",
    )
    .unwrap();
}

#[test]
fn the_sql_alias_matches_only_the_sql_family() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    facts_alias_fixture(&conn);

    let facts = find_structural_facts_scoped(&conn, "sql", None, 10).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].pattern_id, "sql.select_query.v1");
}

#[test]
fn the_category_listing_names_the_aliases_that_match_this_index() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    facts_alias_fixture(&conn);

    let categories = list_structural_fact_categories_scoped(&conn, None).unwrap();
    let listing = format_fact_categories(&categories);
    let alias_line = listing.lines().next().unwrap();
    assert!(alias_line.starts_with("Aliases:"), "{listing}");
    assert!(alias_line.contains("sql (1 pattern, 1 fact)"), "{listing}");
    assert!(!alias_line.contains("css"), "{listing}");
}

#[test]
fn facts_and_literals_each_get_the_whole_limit() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO structural_facts (structural_fact_id, file_id, path, language, pattern_id, capture_name, node_kind, containing_symbol_id, start_line, end_line, confidence) VALUES
            ('sf_a', 'f1', 'db/a.sql', 'sql', 'sql.select_query.v1', 'select', 'select_statement', NULL, 1, 1, 1.0),
            ('sf_b', 'f1', 'db/a.sql', 'sql', 'sql.insert_statement.v1', 'insert', 'insert_statement', NULL, 2, 2, 1.0);
         INSERT INTO literals (literal_id, file_id, path, language, kind, literal_text, carrier, containing_symbol_id, start_line, start_column, end_line, end_column, start_byte, end_byte) VALUES
            ('lit_a', 'f1', 'db/a.sql', 'sql', 'sql_query', 'SELECT 1', 'string', NULL, 1, 0, 1, 8, 0, 8),
            ('lit_b', 'f1', 'db/a.sql', 'sql', 'sql_query', 'SELECT 2', 'string', NULL, 2, 0, 2, 8, 9, 17);",
    )
    .unwrap();

    let facts = find_structural_facts_scoped(&conn, "sql", None, 1).unwrap();
    let literals = find_literals_scoped(&conn, "sql", None, 1).unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(literals.len(), 1);

    let out = format_structural_facts(&facts, &literals, "sql", 1);
    assert_eq!(out.matches("limit reached").count(), 2, "{out}");
}

fn qt_facts_fixture(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "INSERT INTO structural_facts (structural_fact_id, file_id, path, language, pattern_id, capture_name, node_kind, containing_symbol_id, start_line, end_line, confidence) VALUES
            ('qf_sig', 'f1', 'ui/Button.qml', 'qml', 'qml.signal_declaration.v1', 'signal', 'ui_signal', NULL, 1, 1, 1.0),
            ('qf_imp', 'f1', 'ui/Button.qml', 'qml', 'qml.import_statement.v1', 'import', 'ui_import', NULL, 2, 2, 1.0),
            ('qf_dirimp', 'f2', 'ui/qmldir', 'qmldir', 'qmldir.import.v1', 'import', 'import', NULL, 1, 1, 1.0),
            ('qf_bind', 'f1', 'ui/Button.qml', 'qml', 'qml.binding.v1', 'binding', 'ui_binding', NULL, 3, 3, 1.0),
            ('qf_obj', 'f1', 'ui/Button.qml', 'qml', 'qml.object_instantiation.v1', 'object', 'ui_object_definition', NULL, 4, 4, 1.0),
            ('qf_type', 'f2', 'ui/qmldir', 'qmldir', 'qmldir.object_type.v1', 'type', 'object_type', NULL, 2, 2, 1.0),
            ('qf_mod', 'f2', 'ui/qmldir', 'qmldir', 'qmldir.module.v1', 'module', 'module', NULL, 3, 3, 1.0),
            ('qf_prag', 'f1', 'ui/Button.qml', 'qml', 'qml.pragma.v1', 'pragma', 'ui_pragma', NULL, 5, 5, 1.0),
            ('qf_qprop', 'f1', 'ui/Button.qml', 'qml', 'qml.property_declaration.v1', 'property', 'ui_property', NULL, 6, 6, 1.0),
            ('qf_cprop', 'f3', 'src/layouts/button.h', 'cpp', 'cpp.qt_property.v1', 'property', 'field_declaration', NULL, 7, 7, 1.0);",
    )
    .unwrap();
}

#[test]
fn qt_fact_aliases_reach_their_pattern_families() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    qt_facts_fixture(&conn);

    let expected: &[(&str, &[&str])] = &[
        ("signal", &["qml.signal_declaration.v1"]),
        ("signals", &["qml.signal_declaration.v1"]),
        ("import", &["qml.import_statement.v1", "qmldir.import.v1"]),
        ("imports", &["qml.import_statement.v1", "qmldir.import.v1"]),
        ("binding", &["qml.binding.v1"]),
        ("bindings", &["qml.binding.v1"]),
        (
            "component",
            &["qml.object_instantiation.v1", "qmldir.object_type.v1"],
        ),
        (
            "components",
            &["qml.object_instantiation.v1", "qmldir.object_type.v1"],
        ),
        ("module", &["qmldir.module.v1"]),
        ("modules", &["qmldir.module.v1"]),
        ("pragma", &["qml.pragma.v1"]),
        (
            "property",
            &["cpp.qt_property.v1", "qml.property_declaration.v1"],
        ),
        (
            "properties",
            &["cpp.qt_property.v1", "qml.property_declaration.v1"],
        ),
    ];

    for (alias, patterns) in expected {
        let facts = find_structural_facts_scoped(&conn, alias, None, 10).unwrap();
        let mut ids: Vec<String> = facts.iter().map(|f| f.pattern_id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids, *patterns, "{alias}");
    }
}

#[test]
fn a_scoped_qt_fact_alias_keeps_its_path_filter() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    qt_facts_fixture(&conn);

    let facts = find_structural_facts_scoped(&conn, "import", Some("ui/qmldir"), 10).unwrap();

    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].pattern_id, "qmldir.import.v1");

    let properties =
        find_structural_facts_scoped(&conn, "property", Some("src/layouts"), 10).unwrap();

    assert_eq!(properties.len(), 1);
    assert_eq!(properties[0].pattern_id, "cpp.qt_property.v1");
}

#[test]
fn the_category_listing_names_the_qt_aliases() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    qt_facts_fixture(&conn);

    let categories = list_structural_fact_categories_scoped(&conn, None).unwrap();
    let alias_line = format_fact_categories(&categories)
        .lines()
        .next()
        .unwrap()
        .to_string();

    for alias in [
        "signal (1 pattern, 1 fact)",
        "import (2 patterns, 2 facts)",
        "binding (1 pattern, 1 fact)",
        "component (2 patterns, 2 facts)",
        "module (1 pattern, 1 fact)",
        "pragma (1 pattern, 1 fact)",
        "property (2 patterns, 2 facts)",
    ] {
        assert!(alias_line.contains(alias), "{alias_line}");
    }
    assert!(!alias_line.contains("signals ("), "{alias_line}");
}
