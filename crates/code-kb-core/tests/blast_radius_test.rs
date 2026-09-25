use rusqlite::Connection;

use code_kb_core::{
    Workspace, blast_radius_op, compute_blast_radius, compute_blast_radius_scoped, find_references,
    find_references_ext, find_related_tests, format_blast_radius, get_symbol_by_id,
    open_read_write, safe_tempdir,
};

fn setup_test_db(conn: &Connection) {
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
            start_line INTEGER, start_column INTEGER
        );",
    )
    .unwrap();
}

#[test]
fn test_language_agnostic_callee_filtering() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO files VALUES ('f1', 'src/auth.py', 'python', 'h1', 100, 10, 'now');
        INSERT INTO symbols VALUES
            ('s_login', 'f1', 'src/auth.py', 'python', 'login', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_verify', 'f1', 'src/auth.py', 'python', 'verify_token', 'function', NULL, NULL, NULL, NULL, 6, 0, 10, 0, 51, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO pending_relationships VALUES
            ('s_login', 'verify_token', 'calls', 'src/auth.py', 2, 4),
            ('s_login', 'print', 'calls', 'src/auth.py', 3, 4),
            ('s_login', 'len', 'calls', 'src/auth.py', 4, 4);",
    )
    .unwrap();

    // Default find_references: filters out unresolved stdlib/external calls like 'print' and 'len'
    let internal_callees = find_references(&conn, "login", "callees", 10).unwrap();
    assert_eq!(internal_callees.len(), 1);
    assert_eq!(internal_callees[0].to_symbol_name, "verify_token");

    // find_references_ext with include_external=true includes 'print' and 'len'
    let all_callees = find_references_ext(&conn, "login", "callees", 10, true).unwrap();
    assert_eq!(all_callees.len(), 3);
    let names: Vec<_> = all_callees
        .iter()
        .map(|c| c.to_symbol_name.as_str())
        .collect();
    assert!(names.contains(&"verify_token"));
    assert!(names.contains(&"print"));
    assert!(names.contains(&"len"));
}

#[test]
fn test_blast_radius_multi_hop_and_likely_tests() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO files VALUES
            ('f1', 'src/core.rs', 'rust', 'h1', 100, 10, 'now'),
            ('f2', 'src/service.rs', 'rust', 'h2', 100, 10, 'now'),
            ('f3', 'src/api.rs', 'rust', 'h3', 100, 10, 'now'),
            ('f4', 'tests/core_test.rs', 'rust', 'h4', 100, 10, 'now');
        INSERT INTO symbols VALUES
            ('s_base', 'f1', 'src/core.rs', 'rust', 'base_calc', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_service', 'f2', 'src/service.rs', 'rust', 'service_calc', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_api', 'f3', 'src/api.rs', 'rust', 'handle_request', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_test', 'f4', 'tests/core_test.rs', 'rust', 'test_base_calc', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0);
        INSERT INTO relationships VALUES
            ('s_service', 's_base', 'calls', 'src/service.rs', 2, 0),
            ('s_api', 's_service', 'calls', 'src/api.rs', 2, 0),
            ('s_test', 's_base', 'calls', 'tests/core_test.rs', 2, 0);",
    )
    .unwrap();

    let result = compute_blast_radius(&conn, &["base_calc"], &[], 3, 20).unwrap();
    assert_eq!(result.seed_type, "symbol");
    assert_eq!(result.seeds, vec!["base_calc"]);

    // Test caller must be partitioned into likely_tests
    assert_eq!(result.likely_tests.len(), 1);
    assert_eq!(result.likely_tests[0].name, "test_base_calc");
    assert_eq!(result.likely_tests[0].path, "tests/core_test.rs");

    // Impacted non-test symbols: depth 1 (service_calc), depth 2 (handle_request)
    assert_eq!(result.impacted_symbols.len(), 2);
    assert_eq!(result.impacted_symbols[0].name, "service_calc");
    assert_eq!(result.impacted_symbols[0].depth, 1);
    assert_eq!(result.impacted_symbols[1].name, "handle_request");
    assert_eq!(result.impacted_symbols[1].depth, 2);

    // Formatted output verification
    let formatted = format_blast_radius(&result);
    assert!(formatted.contains("## Blast Radius & Test Impact (Symbol: base_calc)"));
    assert!(formatted.contains("### Likely Tests to Run (1 returned)"));
    assert!(formatted.contains(
        "tests/core_test.rs:\n  - `test_base_calc` [line 1] (transitive caller [depth 1])"
    ));
    assert!(formatted.contains("src/service.rs:\n  - [depth 1] function `service_calc` [line 1]"));
    assert!(formatted.contains("src/api.rs:\n  - [depth 2] function `handle_request` [line 1]"));
}

#[test]
fn test_blast_radius_op_file_seed_and_stem_matching() {
    let temp = safe_tempdir();
    let workspace = Workspace::new(temp.path().to_path_buf());
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO files VALUES
            ('f1', 'src/auth.rs', 'rust', 'h1', 100, 10, 'now'),
            ('f2', 'tests/test_auth.rs', 'rust', 'h2', 100, 10, 'now');
        INSERT INTO symbols VALUES
            ('s_auth', 'f1', 'src/auth.rs', 'rust', 'authenticate', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);",
    )
    .unwrap();

    let result = blast_radius_op(&workspace, &conn, None, Some("src/auth.rs"), 2, 20).unwrap();
    assert_eq!(result.seed_type, "file");
    assert_eq!(result.seeds, vec!["src/auth.rs"]);

    // Even without an explicit call edge, stem-matched test file is found!
    assert_eq!(result.likely_tests.len(), 1);
    assert_eq!(result.likely_tests[0].path, "tests/test_auth.rs");
    assert_eq!(result.likely_tests[0].reason, "stem-matched test file");
}

#[test]
fn related_tests_list_flagged_tests_before_helpers_in_the_same_test_file() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s_target', 'f1', 'command.go', 'go', 'ExecuteC', 'method', NULL, NULL, NULL, NULL, 10, 0, 20, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_helper', 'f2', 'command_test.go', 'go', 'executeCommandC', 'function', NULL, NULL, NULL, NULL, 48, 0, 55, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_test', 'f2', 'command_test.go', 'go', 'TestExecuteC', 'function', NULL, NULL, NULL, NULL, 159, 0, 170, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0);
        INSERT INTO relationships VALUES
            ('s_helper', 's_target', 'calls', 'command_test.go', 50, 0),
            ('s_test', 's_target', 'calls', 'command_test.go', 160, 0);",
    )
    .unwrap();
    let target = get_symbol_by_id(&conn, "s_target").unwrap().unwrap();

    let tests = find_related_tests(&conn, &target, 1).unwrap();

    assert_eq!(tests[0].name, "TestExecuteC");
}

#[test]
fn a_stem_matches_test_file_names_not_the_folders_above_them() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO files VALUES
            ('f1', 'src/flask/app.py', 'python', 'h1', 100, 10, 'now'),
            ('f2', 'tests/test_app.py', 'python', 'h2', 100, 10, 'now'),
            ('f3', 'tests/test_apps/.flaskenv', 'dotenv', 'h3', 100, 10, 'now'),
            ('f4', 'tests/test_apps/blueprintapp/__init__.py', 'python', 'h4', 100, 10, 'now'),
            ('f5', 'tests/test_apps/blueprintapp/static/css/test.css', 'css', 'h5', 100, 10, 'now');",
    )
    .unwrap();

    let result = compute_blast_radius(&conn, &[], &["src/flask/app.py"], 1, 20).unwrap();

    let paths: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|t| t.path.as_str())
        .collect();
    assert_eq!(paths, ["tests/test_app.py"]);
}

#[test]
fn a_matched_file_counts_as_a_test_only_when_its_name_or_symbols_say_so() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO files VALUES
            ('f1', 'src/flask/app.py', 'python', 'h1', 100, 10, 'now'),
            ('f2', 'tests/test_app.py', 'python', 'h2', 100, 10, 'now'),
            ('f3', 'tests/test_apps/cliapp/app.py', 'python', 'h3', 100, 10, 'now'),
            ('f4', 'tests/test_apps/cliapp/multiapp.py', 'python', 'h4', 100, 10, 'now'),
            ('f5', 'test/fixtures/app.tmpl', 'text', 'h5', 100, 10, 'now'),
            ('f6', 'test/app.render.js', 'javascript', 'h6', 100, 10, 'now'),
            ('f7', 'tests/test_apps/cliapp/inner1/inner2/flask.py', 'python', 'h7', 100, 10, 'now'),
            ('f8', 'tests/app_inspect.py', 'python', 'h8', 100, 10, 'now'),
            ('f9', 'src/test/java/AppTest.java', 'java', 'h9', 100, 10, 'now'),
            ('f10', 'spec/app_spec.rb', 'ruby', 'h10', 100, 10, 'now');
        INSERT INTO symbols VALUES
            ('s_app', 'f3', 'tests/test_apps/cliapp/app.py', 'python', 'testapp', 'variable', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_render', 'f6', 'test/app.render.js', 'javascript', 'should render', 'function', NULL, NULL, NULL, NULL, 5, 0, 9, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0);",
    )
    .unwrap();

    let result = compute_blast_radius(&conn, &[], &["src/flask/app.py"], 1, 20).unwrap();

    let paths: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|t| t.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "spec/app_spec.rb",
            "src/test/java/AppTest.java",
            "test/app.render.js",
            "tests/test_app.py"
        ]
    );
}

#[test]
fn file_seed_finds_test_named_after_its_module() {
    let temp = safe_tempdir();
    let workspace = Workspace::new(temp.path().to_path_buf());
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO files VALUES
            ('f1', 'src/mcp/server.rs', 'rust', 'h1', 100, 10, 'now'),
            ('f2', 'tests/mcp_test.rs', 'rust', 'h2', 100, 10, 'now'),
            ('f3', 'tests/unrelated_test.rs', 'rust', 'h3', 100, 10, 'now');",
    )
    .unwrap();

    let result =
        blast_radius_op(&workspace, &conn, None, Some("src/mcp/server.rs"), 2, 20).unwrap();
    assert_eq!(result.likely_tests.len(), 1);
    assert_eq!(result.likely_tests[0].path, "tests/mcp_test.rs");
    assert_eq!(result.likely_tests[0].reason, "module-matched test file");
}

#[test]
fn file_seed_keeps_specific_tests_ahead_of_module_matches() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO files VALUES
            ('f1', 'crates/code-kb-cli/tests/unrelated_test.rs', 'rust', 'h1', 100, 10, 'now'),
            ('f2', 'tests/cli_test.rs', 'rust', 'h2', 100, 10, 'now'),
            ('f3', 'tests/target_test.rs', 'rust', 'h3', 100, 10, 'now'),
            ('f4', 'tests/cli/unrelated_test.rs', 'rust', 'h4', 100, 10, 'now');",
    )
    .unwrap();

    let seeds = [
        "src/cli/first.rs",
        "src/other/target.rs",
        "src/test/unused.rs",
    ];
    let capped = compute_blast_radius(&conn, &[], &seeds, 2, 1).unwrap();
    assert_eq!(capped.likely_tests[0].path, "tests/target_test.rs");

    let full = compute_blast_radius(&conn, &[], &seeds, 2, 20).unwrap();
    let paths: Vec<&str> = full
        .likely_tests
        .iter()
        .map(|test| test.path.as_str())
        .collect();
    assert_eq!(paths, vec!["tests/target_test.rs", "tests/cli_test.rs"]);
}

#[test]
fn qualified_blast_radius_seed_selects_only_its_parent_method() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('alpha', 'f1', 'src/types.rs', 'rust', 'Alpha', 'struct', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('beta', 'f1', 'src/types.rs', 'rust', 'Beta', 'struct', NULL, NULL, NULL, NULL, 2, 0, 2, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('alpha_process', 'f1', 'src/types.rs', 'rust', 'process', 'method', NULL, NULL, NULL, 'alpha', 3, 0, 3, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('beta_process', 'f1', 'src/types.rs', 'rust', 'process', 'method', NULL, NULL, NULL, 'beta', 4, 0, 4, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('alpha_caller', 'f1', 'src/types.rs', 'rust', 'calls_alpha', 'function', NULL, NULL, NULL, NULL, 5, 0, 5, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('beta_caller', 'f1', 'src/types.rs', 'rust', 'calls_beta', 'function', NULL, NULL, NULL, NULL, 6, 0, 6, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO relationships VALUES
            ('alpha_caller', 'alpha_process', 'calls', 'src/types.rs', 5, 0),
            ('beta_caller', 'beta_process', 'calls', 'src/types.rs', 6, 0);",
    )
    .unwrap();

    let result = compute_blast_radius(&conn, &["Alpha::process"], &[], 1, 20).unwrap();
    assert_eq!(
        result
            .impacted_symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>(),
        vec!["calls_alpha"]
    );
}

#[test]
fn blast_radius_rejects_unknown_or_ambiguous_symbol_seeds() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('alpha', 'f1', 'src/types.rs', 'rust', 'Alpha', 'struct', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('beta', 'f1', 'src/types.rs', 'rust', 'Beta', 'struct', NULL, NULL, NULL, NULL, 2, 0, 2, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('alpha_process', 'f1', 'src/types.rs', 'rust', 'process', 'method', NULL, NULL, NULL, 'alpha', 3, 0, 3, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('beta_process', 'f1', 'src/types.rs', 'rust', 'process', 'method', NULL, NULL, NULL, 'beta', 4, 0, 4, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);",
    )
    .unwrap();

    assert!(matches!(
        compute_blast_radius(&conn, &["process"], &[], 1, 20),
        Err(code_kb_core::QueryError::AmbiguousSymbol(_, 2, _))
    ));
    assert!(matches!(
        compute_blast_radius(&conn, &["missing"], &[], 1, 20),
        Err(code_kb_core::QueryError::SymbolNotFound { ref name, .. }) if name == "missing"
    ));
}

#[test]
fn unknown_symbol_id_error_uses_reselection_guidance_without_name_candidates() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    let error = code_kb_core::compute_blast_radius_scoped_with_ids(
        &conn,
        &[],
        &["missing-id"],
        None,
        &[],
        1,
        20,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("symbol id missing-id"));
    assert!(error.contains("Run lookup_symbol or search_symbols again"));
    assert!(!error.contains("Did you mean one of"));
}

#[test]
fn blast_radius_symbol_id_requires_database_path_instead_of_git_discovery() {
    let temp = safe_tempdir();
    let workspace = code_kb_core::Workspace::new(temp.path().to_path_buf());
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute_batch(
        "INSERT INTO symbols VALUES ('existing-id', 'f1', 'src/target.rs', 'rust', 'target', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);",
    )
    .unwrap();

    let result = code_kb_core::blast_radius_selected_op(
        &workspace,
        None,
        &conn,
        Some(code_kb_core::SymbolSelector::Id("existing-id".to_string())),
        None,
        2,
        20,
    );

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("database path is required")
    );
}

#[test]
fn blast_radius_disambiguates_symbol_seed_using_seed_path() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s1', 'f1', 'src/alpha.rs', 'rust', 'handle', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s2', 'f2', 'src/beta.rs', 'rust', 'handle', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('c1', 'f3', 'src/caller_alpha.rs', 'rust', 'call_alpha', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('u1', 'f1', 'src/alpha.rs', 'rust', 'unrelated', 'function', NULL, NULL, NULL, NULL, 5, 0, 5, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('c_u', 'f4', 'src/caller_u.rs', 'rust', 'call_u', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO relationships VALUES
            ('c1', 's1', 'calls', 'src/caller_alpha.rs', 1, 0),
            ('c_u', 'u1', 'calls', 'src/caller_u.rs', 1, 0);",
    )
    .unwrap();

    // 1. compute_blast_radius_scoped resolves 'handle' using path filter without seeding unrelated functions in src/alpha.rs
    let res =
        compute_blast_radius_scoped(&conn, &["handle"], Some("src/alpha.rs"), &[], 1, 20).unwrap();
    assert_eq!(res.impacted_symbols.len(), 1);
    assert_eq!(res.impacted_symbols[0].name, "call_alpha");

    // 2. blast_radius_op similarly disambiguates symbol using file without pulling in other symbols in src/alpha.rs
    let ws = Workspace::new(temp.path().to_path_buf());
    let res_op = blast_radius_op(&ws, &conn, Some("handle"), Some("src/alpha.rs"), 1, 20).unwrap();
    assert_eq!(res_op.impacted_symbols.len(), 1);
    assert_eq!(res_op.impacted_symbols[0].name, "call_alpha");

    // 3. Mixed request: symbol in alpha and file in beta resolves without forcing symbol into beta
    let res_mixed =
        compute_blast_radius_scoped(&conn, &["call_alpha"], None, &["src/beta.rs"], 1, 20).unwrap();
    assert_eq!(res_mixed.seed_type, "mixed");
}

#[test]
fn test_blast_radius_seed_path_with_underscores_and_case() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s_test', 'f1', 'tests/cli_test.rs', 'rust', 'run_cli_test', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_caller', 'f2', 'src/runner.rs', 'rust', 'execute_tests', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO relationships VALUES
            ('s_caller', 's_test', 'calls', 'src/runner.rs', 2, 0);",
    )
    .unwrap();

    // File seed with underscores must match exact path (CRIT-01)
    let res = compute_blast_radius_scoped(&conn, &[], None, &["tests/cli_test.rs"], 1, 20).unwrap();
    assert_eq!(res.impacted_symbols.len(), 1);
    assert_eq!(res.impacted_symbols[0].name, "execute_tests");

    // Case variation must also match via COLLATE NOCASE (MED-05)
    let res_case =
        compute_blast_radius_scoped(&conn, &[], None, &["TESTS/CLI_TEST.RS"], 1, 20).unwrap();
    assert_eq!(res_case.impacted_symbols.len(), 1);
    assert_eq!(res_case.impacted_symbols[0].name, "execute_tests");
}

#[test]
fn test_blast_radius_cycle_excludes_seed_symbol() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    // Call graph: a -> b -> a (cycle)
    // When a changes, b calls a (impacted at depth 1).
    // a calls b (depth 2), but a is the seed itself and must NOT be returned as an impacted caller!
    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('sym_a', 'f1', 'src/lib.rs', 'rust', 'func_a', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('sym_b', 'f1', 'src/lib.rs', 'rust', 'func_b', 'function', NULL, NULL, NULL, NULL, 5, 0, 5, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO relationships VALUES
            ('sym_b', 'sym_a', 'calls', 'src/lib.rs', 6, 0),
            ('sym_a', 'sym_b', 'calls', 'src/lib.rs', 2, 0);",
    )
    .unwrap();

    let res = compute_blast_radius_scoped(&conn, &["func_a"], None, &[], 2, 20).unwrap();
    // Only func_b should be reported as impacted; func_a must not be reported as impacted by itself
    assert_eq!(res.impacted_symbols.len(), 1);
    assert_eq!(res.impacted_symbols[0].name, "func_b");
    assert_eq!(res.impacted_symbols[0].depth, 1);
}

#[test]
fn test_blast_radius_max_depth_clamped() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('sym_a', 'f1', 'src/lib.rs', 'rust', 'func_a', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);",
    )
    .unwrap();

    // Large max_depth is clamped to 5 without error
    let res = compute_blast_radius_scoped(&conn, &["func_a"], None, &[], 100, 20).unwrap();
    assert_eq!(res.impacted_symbols.len(), 0);
}

#[test]
fn stem_matched_test_files_skip_documentation() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
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
            semantic_group TEXT, is_test INTEGER, test_container INTEGER, content_type TEXT
        );
        CREATE TABLE relationships (
            from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT, path TEXT,
            start_line INTEGER, start_column INTEGER
        );
        CREATE TABLE pending_relationships (
            from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
            start_line INTEGER, start_column INTEGER
        );
        INSERT INTO files VALUES
            ('f1', 'src/ledger.rs', 'rust', 'h1', 100, 10, 'now'),
            ('f2', 'tests/ledger_test.rs', 'rust', 'h2', 100, 10, 'now'),
            ('f3', 'docs/tests/ledger.md', 'markdown', 'h3', 100, 10, 'now');
        INSERT INTO symbols VALUES
            ('s_ledger', 'f1', 'src/ledger.rs', 'rust', 'post_entry', 'function', 'pub fn post_entry()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0, NULL),
            ('s_doc', 'f3', 'docs/tests/ledger.md', 'markdown', 'Ledger tests', 'module', 'Ledger tests', NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0, 'documentation');",
    )
    .unwrap();

    let result = compute_blast_radius(&conn, &["post_entry"], &[], 1, 20).unwrap();

    let paths: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|t| t.path.as_str())
        .collect();
    assert_eq!(paths, vec!["tests/ledger_test.rs"]);
}

#[test]
fn blast_radius_reports_requested_limit_truncation_before_output_truncation() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO files VALUES
            ('f1', 'src/core.rs', 'rust', 'h1', 100, 10, 'now'),
            ('f2', 'src/service.rs', 'rust', 'h2', 100, 10, 'now'),
            ('f3', 'src/api.rs', 'rust', 'h3', 100, 10, 'now'),
            ('f4', 'tests/core_test.rs', 'rust', 'h4', 100, 10, 'now'),
            ('f5', 'tests/api_test.rs', 'rust', 'h5', 100, 10, 'now');
        INSERT INTO symbols VALUES
            ('s_base', 'f1', 'src/core.rs', 'rust', 'base_calc', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_service', 'f2', 'src/service.rs', 'rust', 'service_calc', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_api', 'f3', 'src/api.rs', 'rust', 'handle_request', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s_test_one', 'f4', 'tests/core_test.rs', 'rust', 'test_base_calc', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0),
            ('s_test_two', 'f5', 'tests/api_test.rs', 'rust', 'test_handle_request', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0);
        INSERT INTO relationships VALUES
            ('s_service', 's_base', 'calls', 'src/service.rs', 2, 0),
            ('s_api', 's_service', 'calls', 'src/api.rs', 2, 0),
            ('s_test_one', 's_base', 'calls', 'tests/core_test.rs', 2, 0),
            ('s_test_two', 's_api', 'calls', 'tests/api_test.rs', 2, 0);",
    )
    .unwrap();

    let limited = compute_blast_radius(&conn, &["base_calc"], &[], 3, 1).unwrap();
    let limited_json = serde_json::to_value(&limited).unwrap();
    assert_eq!(limited.likely_tests.len(), 1);
    assert_eq!(limited.impacted_symbols.len(), 1);
    assert_eq!(limited_json["likely_tests_truncated"], true);
    assert_eq!(limited_json["impacted_symbols_truncated"], true);
    let limited_text = format_blast_radius(&limited);
    assert!(limited_text.contains("Likely Tests to Run (1 returned)"));
    assert!(limited_text.contains("Requested limit"));

    let exact_one = compute_blast_radius(&conn, &["base_calc"], &[], 1, 1).unwrap();
    assert_eq!(exact_one.likely_tests.len(), 1);
    assert_eq!(exact_one.impacted_symbols.len(), 1);
    assert!(!exact_one.likely_tests_truncated);
    assert!(!exact_one.impacted_symbols_truncated);

    let exact = compute_blast_radius(&conn, &["base_calc"], &[], 3, 2).unwrap();
    let exact_json = serde_json::to_value(&exact).unwrap();
    assert_eq!(exact_json["likely_tests_truncated"], false);
    assert_eq!(exact_json["impacted_symbols_truncated"], false);

    let zero = compute_blast_radius(&conn, &["base_calc"], &[], 3, 0).unwrap();
    let zero_text = format_blast_radius(&zero);
    assert!(zero.likely_tests.is_empty());
    assert!(zero.impacted_symbols.is_empty());
    assert!(zero_text.contains("Likely Tests to Run (0 returned)"));
    assert!(!zero_text.contains("No direct or stem-matched tests found"));
}

#[test]
fn blast_radius_probes_traversal_and_test_file_ceilings() {
    for (count, expected_ceiling) in [(200, false), (201, true)] {
        let temp = safe_tempdir();
        let conn = open_read_write(&temp.path().join("index.db")).unwrap();
        setup_test_db(&conn);
        conn.execute(
            "INSERT INTO files VALUES ('f1', 'src/core.rs', 'rust', 'h1', 100, 10, 'now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols VALUES ('root', 'f1', 'src/core.rs', 'rust', 'base_calc', 'function', NULL, NULL, NULL, NULL, 1, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0)",
            [],
        )
        .unwrap();
        for index in 0..count {
            let id = format!("s{index}");
            conn.execute(
                "INSERT INTO symbols VALUES (?1, 'f1', 'src/core.rs', 'rust', ?2, 'function', NULL, NULL, NULL, NULL, ?3, 0, 5, 0, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0)",
                rusqlite::params![id, format!("caller_{index}"), index + 2],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO relationships VALUES (?1, 'root', 'calls', 'src/core.rs', 2, 0)",
                rusqlite::params![format!("s{index}")],
            )
            .unwrap();
        }
        let result = compute_blast_radius(&conn, &["base_calc"], &[], 1, 200).unwrap();
        assert_eq!(result.traversal_ceiling_reached, expected_ceiling);
        assert_eq!(result.likely_tests.len(), 200);
        if expected_ceiling {
            assert!(
                format_blast_radius(&result)
                    .contains("Traversal stopped at the 200-row discovery ceiling")
            );
        }
    }

    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    conn.execute(
        "INSERT INTO files VALUES ('seed', 'src/widget.rs', 'rust', 'h1', 100, 10, 'now')",
        [],
    )
    .unwrap();
    for index in 0..11 {
        conn.execute(
            "INSERT INTO files VALUES (?1, ?2, 'rust', 'h', 100, 10, 'now')",
            rusqlite::params![format!("t{index}"), format!("tests/widget_test_{index}.rs")],
        )
        .unwrap();
    }
    let result = compute_blast_radius(&conn, &[], &["src/widget.rs"], 1, 20).unwrap();
    assert_eq!(result.likely_tests.len(), 10);
    assert!(result.test_file_ceiling_reached);
}
