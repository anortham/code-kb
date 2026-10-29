use rusqlite::Connection;

use code_kb_core::{
    Workspace, blast_radius_op, compute_blast_radius, compute_blast_radius_scoped,
    find_julie_extract_binary, find_references, find_references_ext, find_related_tests,
    format_blast_radius, get_symbol_by_id, open_read_only, open_read_write, safe_tempdir,
    scan_workspace,
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
    assert!(
        formatted.contains("tests/core_test.rs:\n  - `test_base_calc` [line 1] (direct caller)")
    );
    assert!(formatted.contains("src/service.rs:\n  - [depth 1] function `service_calc` [line 1]"));
    assert!(formatted.contains("src/api.rs:\n  - [depth 2] function `handle_request` [line 1]"));
}

#[test]
fn a_file_seed_lists_no_test_file_that_only_shares_its_stem() {
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
    assert!(result.likely_tests.is_empty(), "{:?}", result.likely_tests);
    assert!(
        format_blast_radius(&result)
            .contains("No test reaches it through the index. Run the full test suite.\n")
    );
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
fn a_file_seed_lists_no_test_file_that_only_shares_its_module_name() {
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

    assert!(result.likely_tests.is_empty(), "{:?}", result.likely_tests);
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
    assert!(error.contains("Run lookup_symbol again"));
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

fn call_chain(conn: &Connection, names: &[&str]) {
    for (line, name) in names.iter().enumerate() {
        conn.execute(
            "INSERT INTO symbols (symbol_id, file_id, path, language, name, kind, start_line,
                start_column, end_line, end_column, start_byte, end_byte, is_test, test_container)
             VALUES (?1, 'f1', 'src/lib.rs', 'rust', ?1, 'function', ?2, 0, ?2, 0, 0, 1, 0, 0)",
            rusqlite::params![name, line as i64 + 1],
        )
        .unwrap();
    }
    for pair in names.windows(2) {
        conn.execute(
            "INSERT INTO relationships VALUES (?1, ?2, 'calls', 'src/lib.rs', 1, 0)",
            rusqlite::params![pair[1], pair[0]],
        )
        .unwrap();
    }
}

#[test]
fn a_walk_that_ends_at_the_depth_limit_has_no_callers_beyond_it() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    call_chain(&conn, &["a", "b", "c"]);

    let res = compute_blast_radius_scoped(&conn, &["a"], None, &[], 2, 20).unwrap();

    assert_eq!(res.impacted_symbols.len(), 2);
    assert_eq!(res.stopped_at_depth, None);
    assert!(format_blast_radius(&res).contains("No test reaches it through the index."));
}

#[test]
fn a_walk_with_a_caller_beyond_the_depth_limit_says_a_larger_depth_may_find_tests() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    call_chain(&conn, &["a", "b", "c", "d"]);

    let res = compute_blast_radius_scoped(&conn, &["a"], None, &[], 2, 20).unwrap();

    let names: Vec<&str> = res
        .impacted_symbols
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["b", "c"]);
    assert_eq!(res.stopped_at_depth, Some(2));
}

#[test]
fn a_walk_at_the_largest_depth_never_suggests_a_larger_one() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    call_chain(&conn, &["a", "b", "c", "d", "e", "f", "g"]);

    let res = compute_blast_radius_scoped(&conn, &["a"], None, &[], 5, 20).unwrap();

    assert_eq!(res.impacted_symbols.len(), 5);
    assert_eq!(res.stopped_at_depth, None);
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
    assert!(limited_text.contains("Showing 1 of "));

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
    assert!(!zero_text.contains("No test reaches it through the index"));
}

#[test]
fn blast_radius_probes_the_traversal_ceiling_and_ignores_test_files_that_share_a_stem() {
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
    assert!(result.likely_tests.is_empty(), "{:?}", result.likely_tests);
    assert!(
        serde_json::to_value(&result)
            .unwrap()
            .get("test_file_ceiling_reached")
            .is_none()
    );
}

#[test]
fn a_go_method_called_through_a_receiver_in_a_test_lists_that_test() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in [
        ("go.mod", "module example.com/cmd\n\ngo 1.22\n"),
        (
            "command.go",
            "package cmd\n\ntype Command struct{}\n\nfunc (c *Command) SetHelpCommand(cmd *Command) {}\n",
        ),
        (
            "command_test.go",
            "package cmd\n\nimport \"testing\"\n\nfunc TestSetHelpCommand(t *testing.T) {\n\tc := &Command{}\n\tc.SetHelpCommand(&Command{})\n}\n",
        ),
    ] {
        std::fs::write(root.join(path), content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root.clone());
    scan_workspace(&workspace, &db, true).expect("scan failed");
    let conn = open_read_only(&db).unwrap();

    let result = blast_radius_op(&workspace, &conn, Some("SetHelpCommand"), None, 2, 20).unwrap();

    let tests: Vec<(&str, &str, &str)> = result
        .likely_tests
        .iter()
        .map(|t| (t.name.as_str(), t.path.as_str(), t.reason.as_str()))
        .collect();
    assert_eq!(
        tests,
        [("TestSetHelpCommand", "command_test.go", "direct caller")],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_go_method_whose_receiver_type_is_declared_twice_lists_the_test_that_calls_it_by_name() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in [
        ("go.mod", "module example.com/cmd\n\ngo 1.22\n"),
        ("command.go", "package cmd\n\ntype Command struct{}\n"),
        (
            "help.go",
            "package cmd\n\nfunc (c *Command) SetHelpCommand(cmd *Command) {}\n",
        ),
        (
            "command_windows.go",
            "package cmd\n\ntype Command struct{}\n",
        ),
        (
            "command_test.go",
            "package cmd\n\nimport \"testing\"\n\nfunc TestSetHelpCommand(t *testing.T) {\n\tc := &Command{}\n\tc.SetHelpCommand(&Command{})\n}\n",
        ),
    ] {
        std::fs::write(root.join(path), content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root.clone());
    scan_workspace(&workspace, &db, true).expect("scan failed");
    let conn = open_read_only(&db).unwrap();

    let result = blast_radius_op(&workspace, &conn, Some("SetHelpCommand"), None, 2, 20).unwrap();

    let tests: Vec<(&str, &str, &str)> = result
        .likely_tests
        .iter()
        .map(|t| (t.name.as_str(), t.path.as_str(), t.reason.as_str()))
        .collect();
    assert_eq!(
        tests,
        [(
            "TestSetHelpCommand",
            "command_test.go",
            "calls it by name only (find_references)"
        )],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_test_target_that_no_test_calls_lists_itself() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    std::fs::create_dir_all(root.join("test")).unwrap();
    std::fs::write(
        root.join("test/res.attachment.js"),
        "var express = require('../');\n\ndescribe('res', function(){\n  it('should set Content-Disposition', function(){\n    express();\n  });\n});\n",
    )
    .unwrap();
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root.clone());
    scan_workspace(&workspace, &db, true).expect("scan failed");
    let conn = open_read_only(&db).unwrap();

    let result = blast_radius_op(
        &workspace,
        &conn,
        Some("res"),
        Some("test/res.attachment.js"),
        2,
        20,
    )
    .unwrap();

    let tests: Vec<(&str, usize, &str)> = result
        .likely_tests
        .iter()
        .map(|t| (t.path.as_str(), t.line, t.reason.as_str()))
        .collect();
    assert_eq!(
        tests,
        [("test/res.attachment.js", 3, "the target is a test")],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_go_method_reached_through_receiver_calls_lists_the_test_that_calls_its_caller() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in [
        ("go.mod", "module example.com/cmd\n\ngo 1.22\n"),
        (
            "command.go",
            "package cmd\n\ntype Command struct{}\n\nfunc (c *Command) Execute() error {\n\t_, err := c.ExecuteC()\n\treturn err\n}\n\nfunc (c *Command) ExecuteC() (*Command, error) {\n\treturn c, c.execute()\n}\n",
        ),
        (
            "command_exec.go",
            "package cmd\n\nfunc (c *Command) execute() error {\n\treturn nil\n}\n",
        ),
        (
            "command_test.go",
            "package cmd\n\nimport \"testing\"\n\nfunc TestExecute(t *testing.T) {\n\troot := &Command{}\n\troot.Execute()\n}\n",
        ),
    ] {
        std::fs::write(root.join(path), content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root.clone());
    scan_workspace(&workspace, &db, true).expect("scan failed");
    let conn = open_read_only(&db).unwrap();

    let result = blast_radius_op(&workspace, &conn, Some("execute"), None, 3, 20).unwrap();
    let text = format_blast_radius(&result);

    let impacted: Vec<&str> = result
        .impacted_symbols
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(impacted.contains(&"ExecuteC"), "{text}");
    assert!(impacted.contains(&"Execute"), "{text}");
    let tests: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(tests, ["TestExecute"], "{text}");
}

fn scanned(files: &[(&str, &str)]) -> (tempfile::TempDir, Workspace, Connection) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root);
    scan_workspace(&workspace, &db, true).expect("scan failed");
    let conn = open_read_only(&db).unwrap();
    (temp, workspace, conn)
}

#[test]
fn a_caller_that_only_the_callers_report_ties_to_the_target_is_a_depth_one_impacted_symbol() {
    let (_temp, workspace, conn) = scanned(&[(
        "src/app/command.py",
        "class Command:\n    def execute(self):\n        return None\n\n\ndef run(cmd):\n    return cmd.execute()\n",
    )]);

    let result = blast_radius_op(&workspace, &conn, Some("execute"), None, 2, 20).unwrap();

    let impacted: Vec<(&str, &str, usize, usize, Option<&str>)> = result
        .impacted_symbols
        .iter()
        .map(|s| {
            (
                s.name.as_str(),
                s.path.as_str(),
                s.line,
                s.depth,
                s.reason.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        impacted,
        [(
            "run",
            "src/app/command.py",
            6,
            1,
            Some("direct caller (find_references, name only)")
        )]
    );
    let text = format_blast_radius(&result);
    assert!(!text.contains("No downstream callers"), "{text}");
    assert!(
        text.contains(
            "  - [depth 1] function `run` [line 6] (direct caller (find_references, name only))"
        ),
        "{text}"
    );
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(
        json["impacted_symbols"][0]["reason"],
        "direct caller (find_references, name only)"
    );
    assert_eq!(json["callers_report"]["likely"], 1);
}

#[test]
fn sites_the_index_cannot_tie_to_the_target_are_counted_instead_of_reporting_no_callers() {
    let (_temp, workspace, conn) = scanned(&[
        ("package.json", "{\"name\":\"fx\"}\n"),
        (
            "lib/response.js",
            "var res = module.exports = {};\nres.send = function send(body) {\n  return body;\n};\n",
        ),
        (
            "lib/other.js",
            "var out = module.exports = {};\nout.send = function send(body) {\n  return body;\n};\n",
        ),
        (
            "lib/app.js",
            "var res = require('./response');\nfunction handle(list) {\n  list.push(res.send);\n}\nfunction relay(target) {\n  target.send('ok');\n}\nmodule.exports = { handle, relay };\n",
        ),
    ]);

    let result = blast_radius_op(
        &workspace,
        &conn,
        Some("send"),
        Some("lib/response.js"),
        2,
        20,
    )
    .unwrap();

    let impacted: Vec<(&str, Option<&str>)> = result
        .impacted_symbols
        .iter()
        .map(|s| (s.name.as_str(), s.reason.as_deref()))
        .collect();
    assert_eq!(
        impacted,
        [("handle", Some("direct caller (find_references)"))]
    );
    assert_eq!(result.callers_report.as_ref().unwrap().unresolved, 1);
    let text = format_blast_radius(&result);
    assert!(!text.contains("No downstream callers"), "{text}");
    assert!(
        text.contains(
            "The index could not tie 1 more site of the name to this definition; find_references lists it."
        ),
        "{text}"
    );
}

#[test]
fn a_call_in_a_function_nested_in_a_test_lists_the_enclosing_test() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        (
            "src/pkg/helpers.py",
            "def url_for(endpoint):\n    return \"/\" + endpoint\n",
        ),
        (
            "tests/test_basic.py",
            "from pkg.helpers import url_for\n\n\ndef test_url_processors(app):\n    @app.route(\"/\")\n    def index():\n        return url_for(\"about\")\n\n    assert index() == \"/about\"\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("url_for"), None, 2, 20).unwrap();

    let tests: Vec<(&str, &str, usize, &str)> = result
        .likely_tests
        .iter()
        .map(|t| (t.name.as_str(), t.path.as_str(), t.line, t.reason.as_str()))
        .collect();
    assert_eq!(
        tests,
        [(
            "test_url_processors",
            "tests/test_basic.py",
            4,
            "encloses `index` (direct caller)"
        )],
        "{}",
        format_blast_radius(&result)
    );
    assert!(result.impacted_symbols.is_empty());
    let text = format_blast_radius(&result);
    assert!(
        text.contains("Every caller that find_references ties to this definition is a test.\n"),
        "{text}"
    );
}

const CONFTEST: &str = "import pytest\n\n\n@pytest.fixture\ndef app():\n    return object()\n";
const TEST_BASIC: &str = "def test_one(app):\n    assert app\n\n\ndef test_two():\n    assert True\n\n\ndef test_three():\n    value = 1\n    assert value\n";

fn reasons(result: &code_kb_core::BlastRadiusResult) -> Vec<(&str, &str)> {
    result
        .likely_tests
        .iter()
        .map(|t| (t.name.as_str(), t.reason.as_str()))
        .collect()
}

#[test]
fn a_test_file_target_lists_the_tests_it_defines() {
    let (_temp, workspace, conn) = scanned(&[
        ("tests/conftest.py", CONFTEST),
        ("tests/test_basic.py", TEST_BASIC),
    ]);

    let result =
        blast_radius_op(&workspace, &conn, None, Some("tests/test_basic.py"), 2, 20).unwrap();

    assert_eq!(
        reasons(&result),
        [
            ("test_one", "in the target file"),
            ("test_two", "in the target file"),
            ("test_three", "in the target file"),
        ],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_fixture_file_target_lists_the_tests_that_take_its_fixtures() {
    let (_temp, workspace, conn) = scanned(&[
        ("tests/conftest.py", CONFTEST),
        ("tests/test_basic.py", TEST_BASIC),
    ]);

    let result =
        blast_radius_op(&workspace, &conn, None, Some("tests/conftest.py"), 2, 20).unwrap();

    assert_eq!(
        reasons(&result),
        [("test_one", "uses fixture `app`")],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_fixture_target_lists_the_tests_that_take_it() {
    let (_temp, workspace, conn) = scanned(&[
        ("tests/conftest.py", CONFTEST),
        ("tests/test_basic.py", TEST_BASIC),
    ]);

    let result = blast_radius_op(
        &workspace,
        &conn,
        Some("app"),
        Some("tests/conftest.py"),
        2,
        20,
    )
    .unwrap();

    assert_eq!(
        reasons(&result),
        [("test_one", "uses fixture `app`")],
        "{}",
        format_blast_radius(&result)
    );
}

fn git(root: &std::path::Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .args(["-c", "core.autocrlf=false", "-c", "commit.gpgsign=false"])
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn changed_after_commit(
    files: &[(&str, &str)],
    edits: &[(&str, &str)],
) -> (tempfile::TempDir, Workspace, Connection) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    git(&root, &["init", "-q", "-b", "main"]);
    for (path, content) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "base"]);
    for (path, content) in edits {
        std::fs::write(root.join(path), content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root);
    scan_workspace(&workspace, &db, true).expect("scan failed");
    let conn = open_read_only(&db).unwrap();
    (temp, workspace, conn)
}

#[test]
fn the_tests_whose_own_lines_the_diff_changed_are_tests_to_run() {
    let edited = TEST_BASIC
        .replace("    assert True\n", "    assert True\n    assert 2\n")
        .replace("    value = 1\n", "");
    let (_temp, workspace, conn) = changed_after_commit(
        &[
            ("tests/conftest.py", CONFTEST),
            ("tests/test_basic.py", TEST_BASIC),
        ],
        &[("tests/test_basic.py", &edited)],
    );

    let result = blast_radius_op(&workspace, &conn, None, None, 2, 20).unwrap();

    assert_eq!(
        reasons(&result),
        [
            ("test_two", "changed in the diff"),
            ("test_three", "changed in the diff"),
        ],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_fixture_the_diff_changed_lists_the_tests_that_take_it() {
    let (_temp, workspace, conn) = changed_after_commit(
        &[
            ("tests/conftest.py", CONFTEST),
            ("tests/test_basic.py", TEST_BASIC),
        ],
        &[(
            "tests/conftest.py",
            &CONFTEST.replace("return object()", "return dict()"),
        )],
    );

    let result = blast_radius_op(&workspace, &conn, None, None, 2, 20).unwrap();

    assert_eq!(
        reasons(&result),
        [("test_one", "uses fixture `app`")],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_project_in_a_folder_of_its_repository_reads_the_diff_from_that_folder() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let repo = temp.path().to_path_buf();
    let root = repo.join("app");
    git(&repo, &["init", "-q", "-b", "main"]);
    for (path, content) in [
        ("tests/conftest.py", CONFTEST),
        ("tests/test_basic.py", TEST_BASIC),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, content).unwrap();
    }
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    std::fs::write(
        root.join("tests/test_basic.py"),
        TEST_BASIC.replace("    assert True\n", "    assert 2\n"),
    )
    .unwrap();
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root);
    scan_workspace(&workspace, &db, true).expect("scan failed");
    let conn = open_read_only(&db).unwrap();

    let result = blast_radius_op(&workspace, &conn, None, None, 2, 20).unwrap();

    assert_eq!(result.seeds, ["tests/test_basic.py"]);
    assert_eq!(
        reasons(&result),
        [("test_two", "changed in the diff")],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_test_in_a_changed_test_file_that_calls_changed_code_stays_a_test_to_run() {
    let tests = "from pkg.helpers import helper\n\n\ndef test_one():\n    assert helper()\n\n\ndef test_two():\n    assert True\n\n\ndef test_three():\n    assert helper() == 1\n";
    let (_temp, workspace, conn) = changed_after_commit(
        &[
            ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
            ("src/pkg/__init__.py", ""),
            ("src/pkg/helpers.py", "def helper():\n    return 1\n"),
            ("tests/test_basic.py", tests),
        ],
        &[
            ("src/pkg/helpers.py", "def helper():\n    return 2\n"),
            (
                "tests/test_basic.py",
                &tests.replace("    assert True\n", "    assert 2\n"),
            ),
        ],
    );

    let result = blast_radius_op(&workspace, &conn, None, None, 2, 20).unwrap();

    assert_eq!(
        reasons(&result),
        [
            ("test_two", "changed in the diff"),
            ("test_one", "direct caller"),
            ("test_three", "direct caller"),
        ],
        "{}",
        format_blast_radius(&result)
    );
}

const WSGI_APP: &str = "class App:\n    def __call__(self, environ):\n        return self.wsgi_app(environ)\n\n    def wsgi_app(self, environ):\n        return handle(environ)\n\n\ndef handle(environ):\n    return environ\n";

fn open_ends(result: &code_kb_core::BlastRadiusResult) -> Vec<(&str, &str, usize)> {
    result
        .open_ends
        .iter()
        .map(|end| (end.name.as_str(), end.path.as_str(), end.line))
        .collect()
}

#[test]
fn a_method_that_no_indexed_code_calls_is_an_open_end_of_the_walk() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/app.py", WSGI_APP),
        (
            "tests/test_app.py",
            "from pkg.app import handle\n\n\ndef test_handle():\n    assert handle(1) == 1\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("handle"), None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        reasons(&result),
        [("test_handle", "direct caller")],
        "{text}"
    );
    assert_eq!(
        open_ends(&result),
        [("__call__", "src/pkg/app.py", 2)],
        "{text}"
    );
    assert!(
        text.contains("No indexed code calls these symbols"),
        "{text}"
    );
    assert!(
        text.contains("  - method `App.__call__` (src/pkg/app.py:2)"),
        "{text}"
    );
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["open_ends"][0]["name"], "__call__");
}

#[test]
fn an_open_end_past_the_depth_limit_warns_when_the_largest_depth_lists_no_more_tests() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/app.py", WSGI_APP),
        (
            "tests/test_app.py",
            "from pkg.app import handle\n\n\ndef test_handle():\n    assert handle(1) == 1\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("handle"), None, 1, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        reasons(&result),
        [("test_handle", "direct caller")],
        "{text}"
    );
    assert_eq!(result.stopped_at_depth, Some(1), "{text}");
    assert_eq!(result.tests_past_depth, Some(0), "{text}");
    assert_eq!(
        open_ends(&result),
        [("__call__", "src/pkg/app.py", 2)],
        "{text}"
    );
    assert!(
        text.contains("  - method `App.__call__` (src/pkg/app.py:2)"),
        "{text}"
    );
}

#[test]
fn a_method_of_a_class_defined_in_a_test_lists_that_test_and_is_no_open_end() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        (
            "src/pkg/app.py",
            "def build():\n    return 1\n\n\ndef find(module):\n    return getattr(module, \"create\")()\n",
        ),
        (
            "tests/test_app.py",
            "from pkg.app import build, find\n\n\ndef test_find():\n    class Module:\n        @staticmethod\n        def create():\n            return build()\n\n    assert find(Module) == 1\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("build"), None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        reasons(&result),
        [("test_find", "encloses `create` (direct caller)")],
        "{text}"
    );
    assert!(result.open_ends.is_empty(), "{text}");
    assert!(result.impacted_symbols.is_empty(), "{text}");
}

#[test]
fn a_helper_or_nested_class_of_a_test_class_is_no_test_to_run_but_its_callers_are() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        (
            "tests/test_shapes.py",
            "class TestShapes:\n    class Payload:\n        def size(self):\n            return 1\n\n    class TestData:\n        def value(self):\n            return 2\n\n    def build(self):\n        return self.Payload()\n\n    def test_build(self):\n        assert self.build().size() == 1\n        assert self.TestData().value() == 2\n",
        ),
    ]);

    for target in ["build", "Payload", "TestData"] {
        let result = blast_radius_op(&workspace, &conn, Some(target), None, 2, 20).unwrap();

        let text = format_blast_radius(&result);
        assert!(!text.contains("the target is a test"), "{target}: {text}");
        assert!(
            result
                .likely_tests
                .iter()
                .any(|test| test.name.ends_with("test_build")),
            "{target}: {text}"
        );
    }
}

#[test]
fn a_walk_whose_every_caller_leads_to_a_test_has_no_open_end() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        (
            "src/pkg/util.py",
            "def helper():\n    return 1\n\n\ndef outer():\n    return helper()\n",
        ),
        (
            "tests/test_util.py",
            "from pkg.util import outer\n\n\ndef test_outer():\n    assert outer() == 1\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("helper"), None, 3, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        reasons(&result),
        [("test_outer", "indirect caller [depth 2]")],
        "{text}"
    );
    assert!(result.open_ends.is_empty(), "{text}");
    assert!(!text.contains("No indexed code calls"), "{text}");
    let json = serde_json::to_value(&result).unwrap();
    assert!(json.get("open_ends").is_none(), "{json}");
}

fn add_test(conn: &Connection, test: &str, calls: &str) {
    conn.execute(
        "INSERT INTO symbols (symbol_id, file_id, path, language, name, kind, start_line,
            start_column, end_line, end_column, start_byte, end_byte, is_test, test_container)
         VALUES (?1, 'f2', 'tests/lib_test.rs', 'rust', ?1, 'function', ?2, 0, ?2, 0, 0, 1, 1, 0)",
        rusqlite::params![test, test.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relationships VALUES (?1, ?2, 'calls', 'tests/lib_test.rs', 1, 0)",
        rusqlite::params![test, calls],
    )
    .unwrap();
}

#[test]
fn a_test_list_cut_by_the_depth_limit_says_how_many_more_tests_the_largest_depth_lists() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    call_chain(&conn, &["a", "b", "c", "d"]);
    add_test(&conn, "test_b", "b");
    add_test(&conn, "test_dd", "d");

    let res = compute_blast_radius_scoped(&conn, &["a"], None, &[], 2, 20).unwrap();

    let text = format_blast_radius(&res);
    assert_eq!(
        reasons(&res),
        [("test_b", "indirect caller [depth 2]")],
        "{text}"
    );
    assert_eq!(res.stopped_at_depth, Some(2));
    assert_eq!(res.tests_past_depth, Some(1));
    assert!(
        text.contains("The walk stopped at depth 2. A `depth` of 5 lists 1 more test."),
        "{text}"
    );
}

#[test]
fn a_walk_whose_callers_past_the_depth_reach_no_other_test_gives_no_depth_warning() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);
    call_chain(&conn, &["a", "b", "c", "d"]);
    add_test(&conn, "test_b", "b");

    let res = compute_blast_radius_scoped(&conn, &["a"], None, &[], 2, 20).unwrap();

    let text = format_blast_radius(&res);
    assert_eq!(res.stopped_at_depth, Some(2));
    assert_eq!(res.tests_past_depth, Some(0));
    assert!(!text.contains("The walk stopped"), "{text}");
}

#[test]
fn the_walk_goes_on_from_the_callers_that_only_the_callers_report_finds() {
    let (_temp, workspace, conn) = scanned(&[
        (
            "src/app/command.py",
            "class Command:\n    def execute(self):\n        return None\n\n\ndef run(cmd):\n    return cmd.execute()\n",
        ),
        (
            "tests/test_command.py",
            "from app.command import Command, run\n\n\ndef test_run():\n    assert run(Command()) is None\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("execute"), None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        reasons(&result),
        [("test_run", "indirect caller [depth 2]")],
        "{text}"
    );
    assert!(result.open_ends.is_empty(), "{text}");
}

#[test]
fn a_test_that_only_the_callers_report_ties_to_the_target_joins_the_tests_the_walk_found() {
    let (_temp, workspace, conn) = scanned(&[
        (
            "src/app/command.py",
            "class Command:\n    def execute(self):\n        return None\n",
        ),
        (
            "tests/test_command.py",
            "from app.command import Command\n\n\ndef test_built():\n    cmd = Command()\n    assert cmd.execute() is None\n\n\ndef test_passed(thing):\n    assert thing.execute() is None\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("execute"), None, 2, 20).unwrap();

    assert_eq!(
        reasons(&result),
        [
            ("test_built", "direct caller"),
            ("test_passed", "calls it by name only (find_references)"),
        ],
        "{}",
        format_blast_radius(&result)
    );
}

const JS_RESPONSE: &str = "var res = module.exports = {};\nres.send = function send(body) {\n  return format(body);\n};\nres.json = function json(obj) {\n  return this.send(obj);\n};\nfunction format(body) {\n  return String(body);\n}\n";

const JS_RESPONSE_TEST: &str = "describe('res', function () {\n  it('should send a string', function () {\n    app.use(function (req, res) {\n      res.send('ok');\n    });\n  });\n  it('should send json', function () {\n    app.use(function (req, res) {\n      res.json({});\n    });\n  });\n  it('should post a body', function () {\n    request.send('ok');\n  });\n});\n";

#[test]
fn a_javascript_call_whose_receiver_names_the_object_a_method_is_assigned_on_reaches_its_test() {
    let (_temp, workspace, conn) = scanned(&[
        ("package.json", "{\"name\":\"fx\"}\n"),
        ("lib/response.js", JS_RESPONSE),
        ("test/res.send.js", JS_RESPONSE_TEST),
    ]);

    let result = blast_radius_op(
        &workspace,
        &conn,
        Some("format"),
        Some("lib/response.js"),
        3,
        20,
    )
    .unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        reasons(&result),
        [
            ("should send a string", "indirect caller [depth 2]"),
            ("should send json", "indirect caller [depth 3]")
        ],
        "{text}"
    );
    assert!(result.open_ends.is_empty(), "{text}");
}

#[test]
fn a_javascript_call_stays_untied_when_another_object_of_that_name_has_the_method() {
    let (_temp, workspace, conn) = scanned(&[
        ("package.json", "{\"name\":\"fx\"}\n"),
        ("lib/response.js", JS_RESPONSE),
        (
            "lib/mock.js",
            "var res = module.exports = {};\nres.send = function send(body) {\n  return body;\n};\n",
        ),
        ("test/res.send.js", JS_RESPONSE_TEST),
    ]);

    let result = blast_radius_op(
        &workspace,
        &conn,
        Some("format"),
        Some("lib/response.js"),
        3,
        20,
    )
    .unwrap();

    let text = format_blast_radius(&result);
    assert!(
        !reasons(&result)
            .iter()
            .any(|(test, _)| *test == "should send a string"),
        "{text}"
    );
}

const TWO_HELPERS: &str = "LIMIT = 3\n\n\ndef parse(text):\n    return text.split()\n\n\ndef render(items):\n    return \" \".join(items)\n";

const TWO_HELPER_TESTS: &str = "from pkg.util import parse, render\n\n\ndef test_parse():\n    assert parse(\"a b\") == [\"a\", \"b\"]\n\n\ndef test_render():\n    assert render([\"a\"]) == \"a\"\n";

fn helper_project() -> [(&'static str, &'static str); 4] {
    [
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/util.py", TWO_HELPERS),
        ("tests/test_util.py", TWO_HELPER_TESTS),
    ]
}

#[test]
fn a_diff_inside_one_function_walks_from_that_function_only() {
    let (_temp, workspace, conn) = changed_after_commit(
        &helper_project(),
        &[(
            "src/pkg/util.py",
            &TWO_HELPERS.replace("text.split()", "text.split(\" \")"),
        )],
    );

    let result = blast_radius_op(&workspace, &conn, None, None, 2, 20).unwrap();

    assert_eq!(
        reasons(&result),
        [("test_parse", "direct caller")],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_diff_outside_every_function_walks_from_the_whole_file() {
    let (_temp, workspace, conn) = changed_after_commit(
        &helper_project(),
        &[(
            "src/pkg/util.py",
            &TWO_HELPERS.replace("LIMIT = 3", "LIMIT = 4"),
        )],
    );

    let result = blast_radius_op(&workspace, &conn, None, None, 2, 20).unwrap();

    assert_eq!(
        reasons(&result),
        [
            ("test_parse", "direct caller"),
            ("test_render", "direct caller")
        ],
        "{}",
        format_blast_radius(&result)
    );
}

#[test]
fn a_diff_lists_the_tests_that_only_the_callers_report_of_a_changed_function_finds() {
    let command = "class Command:\n    def execute(self):\n        return None\n";
    let (_temp, workspace, conn) = changed_after_commit(
        &[
            ("pyproject.toml", "[project]\nname = \"app\"\n"),
            ("src/app/__init__.py", ""),
            ("src/app/command.py", command),
            (
                "tests/test_command.py",
                "def test_execute(command):\n    assert command.execute() is None\n",
            ),
        ],
        &[(
            "src/app/command.py",
            &command.replace("return None", "return 0"),
        )],
    );

    let result = blast_radius_op(&workspace, &conn, None, None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        reasons(&result),
        [("test_execute", "calls it by name only (find_references)")],
        "{text}"
    );
    assert!(result.open_ends.is_empty(), "{text}");
}

const CLICK_CLI: &str = "import click\n\n\nclass AppGroup(click.Group):\n    def group(self):\n        return self\n\n\nclass FlaskGroup(AppGroup):\n    def __init__(self, create_app=None):\n        super().__init__()\n        self.create_app = create_app\n\n\n@click.command(\"run\")\ndef run_command(info):\n    return info\n\n\ncli = FlaskGroup()\n";

const CLICK_APP: &str = "from pkg.cli import AppGroup\n\n\nclass Flask:\n    def __init__(self):\n        self.cli = AppGroup()\n";

const CLICK_TESTS: &str = "import re\n\nimport click\n\nfrom pkg.cli import FlaskGroup\nfrom pkg.cli import run_command\n\n\ndef test_run_cert_path():\n    ctx = run_command.make_context(\"run\", [])\n    assert ctx\n\n\ndef test_flaskgroup_app_context():\n    @click.group(cls=FlaskGroup)\n    def cli():\n        pass\n\n    assert cli\n\n\ndef test_help_echo_loading_error():\n    from pkg.cli import cli\n\n    print(cli)\n\n\ndef test_session_expiration():\n    match = re.search(\"a\", \"a\")\n    assert match.group(0)\n";

const CLICK_APP_TESTS: &str = "from pkg.app import Flask\nfrom pkg.cli import run_command\n\n\ndef test_one():\n    assert Flask()\n\n\ndef test_two():\n    assert Flask()\n\n\ndef test_app_runs_command():\n    app = Flask()\n    assert run_command.make_context(\"run\", [app])\n";

fn click_project() -> (tempfile::TempDir, Workspace, Connection) {
    scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/cli.py", CLICK_CLI),
        ("src/pkg/app.py", CLICK_APP),
        ("tests/test_cli.py", CLICK_TESTS),
        ("tests/test_app.py", CLICK_APP_TESTS),
    ])
}

#[test]
fn a_file_target_lists_the_tests_that_read_its_commands_classes_and_module_objects() {
    let (_temp, workspace, conn) = click_project();

    let result = blast_radius_op(&workspace, &conn, None, Some("src/pkg/cli.py"), 2, 20).unwrap();

    let text = format_blast_radius(&result);
    let rows = reasons(&result);
    for expected in [
        (
            "test_run_cert_path",
            "calls `run_command` (find_references)",
        ),
        (
            "test_flaskgroup_app_context",
            "calls `FlaskGroup` (find_references)",
        ),
        (
            "test_help_echo_loading_error",
            "calls `cli` (find_references)",
        ),
    ] {
        assert!(rows.contains(&expected), "{expected:?} missing\n{text}");
    }
    assert!(
        !rows
            .iter()
            .any(|(name, _)| *name == "test_session_expiration"),
        "{text}"
    );
    assert!(
        !result.open_ends.iter().any(|end| end.name == "run_command"),
        "{text}"
    );
}

#[test]
fn a_file_target_lists_a_test_that_the_callers_report_ties_outside_its_constructor_fold() {
    let (_temp, workspace, conn) = click_project();

    let result = blast_radius_op(&workspace, &conn, None, Some("src/pkg/cli.py"), 2, 20).unwrap();

    let text = format_blast_radius(&result);
    let rows = reasons(&result);
    assert!(
        rows.contains(&(
            "test_app_runs_command",
            "calls `run_command` (find_references)"
        )),
        "{text}"
    );
    assert!(
        rows.contains(&(
            "tests/test_app.py",
            "2 tests reach it only through a constructor"
        )),
        "{text}"
    );
}

fn impacted(result: &code_kb_core::BlastRadiusResult) -> Vec<(&str, usize, Option<&str>)> {
    result
        .impacted_symbols
        .iter()
        .map(|s| (s.name.as_str(), s.depth, s.reason.as_deref()))
        .collect()
}

const APP_WITH_METHOD_READS: &str = "class App:\n    def __init_subclass__(cls, **kwargs):\n        for method in (cls.handle_error,):\n            assert method\n\n    def handlers(self):\n        return [self.handle_error]\n\n    def handle_error(self, error):\n        return error\n\n    def dispatch(self):\n        return self.handle_error(1)\n\n    def run(self):\n        return self.handlers()\n";

#[test]
fn a_method_read_that_the_walk_cannot_follow_is_a_depth_one_impacted_symbol() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/app.py", APP_WITH_METHOD_READS),
        (
            "tests/test_app.py",
            "from pkg.app import App\n\n\ndef test_dispatch():\n    app = App()\n    assert app.dispatch() == 1\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("App.handle_error"), None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [
            ("dispatch", 1, None),
            (
                "__init_subclass__",
                1,
                Some("direct caller (find_references)")
            ),
            ("handlers", 1, Some("direct caller (find_references)")),
            ("run", 2, None),
        ],
        "{text}"
    );
    assert_eq!(
        reasons(&result),
        [("test_dispatch", "indirect caller [depth 2]")],
        "{text}"
    );
}

#[test]
fn a_method_read_joins_the_callers_the_walk_found_when_no_test_reaches_the_target() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/app.py", APP_WITH_METHOD_READS),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("App.handle_error"), None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [
            ("dispatch", 1, None),
            (
                "__init_subclass__",
                1,
                Some("direct caller (find_references)")
            ),
            ("handlers", 1, Some("direct caller (find_references)")),
            ("run", 2, None),
        ],
        "{text}"
    );
    assert!(result.likely_tests.is_empty(), "{text}");
}

#[test]
fn a_method_read_of_a_function_the_diff_changed_is_a_depth_one_impacted_symbol() {
    let (_temp, workspace, conn) = changed_after_commit(
        &[
            ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
            ("src/pkg/__init__.py", ""),
            ("src/pkg/app.py", APP_WITH_METHOD_READS),
            (
                "tests/test_app.py",
                "from pkg.app import App\n\n\ndef test_dispatch():\n    app = App()\n    assert app.dispatch() == 1\n",
            ),
        ],
        &[(
            "src/pkg/app.py",
            &APP_WITH_METHOD_READS.replace("return error", "return None"),
        )],
    );

    let result = blast_radius_op(&workspace, &conn, None, None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [
            ("dispatch", 1, None),
            (
                "__init_subclass__",
                1,
                Some("direct caller (find_references)")
            ),
            ("handlers", 1, Some("direct caller (find_references)")),
            ("run", 2, None),
        ],
        "{text}"
    );
    assert_eq!(
        reasons(&result),
        [("test_dispatch", "indirect caller [depth 2]")],
        "{text}"
    );
}

const PY_HANDLER: &str = "class App:\n    def handle_error(self, error):\n        return error\n\n    def handlers(self):\n        return [self.handle_error]\n";

#[test]
fn a_file_target_lists_a_reader_of_its_method_in_another_file_and_the_test_of_that_reader() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/app.py", PY_HANDLER),
        (
            "src/pkg/setup.py",
            "from pkg.app import App\n\n\ndef register(app: App):\n    return [app.handle_error]\n",
        ),
        (
            "tests/test_setup.py",
            "from pkg.setup import register\n\n\ndef test_register():\n    assert register(None)\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, None, Some("src/pkg/app.py"), 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [("register", 1, Some("direct caller (find_references)"))],
        "{text}"
    );
    assert_eq!(
        reasons(&result),
        [("test_register", "indirect caller [depth 2]")],
        "{text}"
    );
}

#[test]
fn a_typescript_file_target_lists_a_reader_of_its_method_in_another_file_and_its_test() {
    let (_temp, workspace, conn) = scanned(&[
        ("package.json", "{\"name\":\"pkg\"}\n"),
        (
            "src/app.ts",
            "export class App {\n  handleError(error: Error): Error {\n    return error;\n  }\n}\n",
        ),
        (
            "src/setup.ts",
            "import { App } from \"./app\";\n\nexport function register(app: App) {\n  return [app.handleError];\n}\n",
        ),
        (
            "test/setup.test.ts",
            "import { register } from \"../src/setup\";\n\ntest(\"registers\", () => {\n  expect(register(null)).toBeTruthy();\n});\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, None, Some("src/app.ts"), 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [("register", 1, Some("direct caller (find_references)"))],
        "{text}"
    );
    assert_eq!(
        reasons(&result),
        [("registers", "indirect caller [depth 2]")],
        "{text}"
    );
}

#[test]
fn a_csharp_file_target_lists_a_method_group_reader_in_another_file_and_its_test() {
    let (_temp, workspace, conn) = scanned(&[
        (
            "App.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>\n",
        ),
        (
            "src/App.cs",
            "namespace Pkg;\n\npublic class App\n{\n    public string HandleError(string error)\n    {\n        return error;\n    }\n}\n",
        ),
        (
            "src/Setup.cs",
            "using System;\n\nnamespace Pkg;\n\npublic static class Setup\n{\n    public static Func<string, string> Register(App app)\n    {\n        return app.HandleError;\n    }\n}\n",
        ),
        (
            "tests/SetupTests.cs",
            "using Xunit;\n\nnamespace Pkg.Tests;\n\npublic class SetupTests\n{\n    [Fact]\n    public void Registers()\n    {\n        Assert.NotNull(Pkg.Setup.Register(new Pkg.App()));\n    }\n}\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, None, Some("src/App.cs"), 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [(
            "Register",
            1,
            Some("direct caller (find_references, name only)")
        )],
        "{text}"
    );
    assert_eq!(
        reasons(&result),
        [("SetupTests::Registers", "indirect caller [depth 2]")],
        "{text}"
    );
}

#[test]
fn a_file_target_joins_no_untyped_member_read_when_the_walk_found_callers() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/app.py", PY_HANDLER),
        (
            "src/pkg/caller.py",
            "from pkg.app import App\n\n\ndef run(app: App):\n    return app.handlers()\n",
        ),
        (
            "src/pkg/reader.py",
            "def collect(thing):\n    return [thing.handle_error]\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, None, Some("src/pkg/app.py"), 2, 20).unwrap();

    let text = format_blast_radius(&result);
    let names: Vec<&str> = impacted(&result).iter().map(|row| row.0).collect();
    assert!(names.contains(&"run"), "{text}");
    assert!(!names.contains(&"collect"), "{text}");
}

#[test]
fn a_file_target_lists_an_untyped_member_read_when_the_walk_found_no_caller() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        ("src/pkg/app.py", PY_HANDLER),
        (
            "src/pkg/reader.py",
            "def collect(thing):\n    return [thing.handle_error]\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, None, Some("src/pkg/app.py"), 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [("collect", 1, Some("direct caller (find_references)"))],
        "{text}"
    );
}

#[test]
fn a_caller_the_report_moves_up_to_depth_one_leads_the_walk_to_its_test() {
    let (_temp, workspace, conn) = scanned(&[
        ("pyproject.toml", "[project]\nname = \"pkg\"\n"),
        ("src/pkg/__init__.py", ""),
        (
            "src/pkg/app.py",
            "class App:\n    def handle_error(self, error):\n        return error\n\n    def dispatch(self):\n        return self.handle_error(1)\n\n    def run(self):\n        handler = self.handle_error\n        return self.dispatch()\n",
        ),
        (
            "tests/test_app.py",
            "from pkg.app import App\n\n\ndef test_run():\n    app = App()\n    assert app.run() == 1\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("App.handle_error"), None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [
            ("dispatch", 1, None),
            ("run", 1, Some("direct caller (find_references)")),
        ],
        "{text}"
    );
    assert_eq!(
        reasons(&result),
        [("test_run", "indirect caller [depth 2]")],
        "{text}"
    );
    assert_eq!(result.tests_past_depth, Some(0), "{text}");
    assert!(result.open_ends.is_empty(), "{text}");
}

#[test]
fn a_typescript_caller_the_report_moves_up_to_depth_one_leads_the_walk_to_its_test() {
    let (_temp, workspace, conn) = scanned(&[
        ("package.json", "{\"name\":\"pkg\"}\n"),
        (
            "src/app.ts",
            "export class App {\n  handleError(error: number): number {\n    return error;\n  }\n\n  dispatch(): number {\n    return this.handleError(1);\n  }\n\n  run(): number {\n    const handler = this.handleError;\n    return this.dispatch();\n  }\n}\n",
        ),
        (
            "test/app.test.ts",
            "import { App } from \"../src/app\";\n\ntest(\"runs\", () => {\n  const app = new App();\n  expect(app.run()).toBe(1);\n});\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("App.handleError"), None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [
            ("dispatch", 1, None),
            ("run", 1, Some("direct caller (find_references)")),
        ],
        "{text}"
    );
    assert_eq!(
        reasons(&result),
        [("runs", "indirect caller [depth 2]")],
        "{text}"
    );
    assert!(result.open_ends.is_empty(), "{text}");
}

#[test]
fn a_csharp_caller_the_report_moves_up_to_depth_one_leads_the_walk_to_its_test() {
    let (_temp, workspace, conn) = scanned(&[
        (
            "App.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\"></Project>\n",
        ),
        (
            "src/App.cs",
            "using System;\n\nnamespace Pkg;\n\npublic class App\n{\n    public int HandleError(int error)\n    {\n        return error;\n    }\n\n    public int Dispatch()\n    {\n        return HandleError(1);\n    }\n\n    public int Run()\n    {\n        Func<int, int> handler = this.HandleError;\n        return Dispatch();\n    }\n}\n",
        ),
        (
            "tests/AppTests.cs",
            "using Pkg;\nusing Xunit;\n\nnamespace Pkg.Tests;\n\npublic class AppTests\n{\n    [Fact]\n    public void Runs()\n    {\n        App app = new App();\n        Assert.Equal(1, app.Run());\n    }\n}\n",
        ),
    ]);

    let result = blast_radius_op(&workspace, &conn, Some("App.HandleError"), None, 2, 20).unwrap();

    let text = format_blast_radius(&result);
    assert_eq!(
        impacted(&result),
        [
            ("Dispatch", 1, None),
            ("Run", 1, Some("direct caller (find_references)")),
        ],
        "{text}"
    );
    assert_eq!(
        reasons(&result),
        [("AppTests::Runs", "indirect caller [depth 2]")],
        "{text}"
    );
    assert!(result.open_ends.is_empty(), "{text}");
}

const METHOD_VALUE_READERS: &[(&str, &str, &str, &str, &str, &str)] = &[
    (
        "go",
        "app.go",
        "package p\n\ntype App struct{}\n\nfunc (a *App) Handle(e string) string {\n\treturn e\n}\n",
        "setup.go",
        "package p\n\nfunc Register(a *App) func(string) string {\n\treturn a.Handle\n}\n",
        "Register",
    ),
    (
        "swift",
        "App.swift",
        "class App {\n    func handle(_ e: String) -> String { return e }\n}\n",
        "Setup.swift",
        "func register(app: App) -> (String) -> String {\n    return app.handle\n}\n",
        "register",
    ),
    (
        "dart",
        "app.dart",
        "class App {\n  String handle(String e) => e;\n}\n",
        "setup.dart",
        "import 'app.dart';\n\nFunction register(App app) {\n  return app.handle;\n}\n",
        "register",
    ),
    (
        "javascript",
        "app.js",
        "export class App {\n  handle(e) { return e; }\n}\n",
        "setup.js",
        "import { App } from './app.js';\n\nexport function register(app) {\n  return [app.handle];\n}\n",
        "register",
    ),
    (
        "scala",
        "App.scala",
        "class App {\n  def handle(e: String): String = e\n}\n",
        "Setup.scala",
        "object Setup {\n  def register(app: App): String => String = app.handle _\n}\n",
        "register",
    ),
    (
        "lua",
        "app.lua",
        "local App = {}\n\nfunction App.handle(e)\n  return e\nend\n\nreturn App\n",
        "setup.lua",
        "local App = require('app')\n\nlocal function register()\n  return App.handle\nend\n",
        "register",
    ),
    (
        "vbnet",
        "App.vb",
        "Public Class App\n    Public Function Handle(e As String) As String\n        Return e\n    End Function\nEnd Class\n",
        "Setup.vb",
        "Public Module Setup\n    Public Function Register(app As App) As Func(Of String, String)\n        Return AddressOf app.Handle\n    End Function\nEnd Module\n",
        "Register",
    ),
    (
        "fsharp",
        "App.fs",
        "module App\n\ntype App() =\n    member this.Handle(e: string) = e\n",
        "Setup.fs",
        "module Setup\n\nlet register (app: App.App) = app.Handle\n",
        "register",
    ),
    (
        "gdscript",
        "app.gd",
        "class_name App\n\nfunc handle(e):\n\treturn e\n",
        "setup.gd",
        "func register(app: App):\n\treturn app.handle\n",
        "register",
    ),
    (
        "rust",
        "src/app.rs",
        "pub struct App;\n\nimpl App {\n    pub fn handle(&self, e: u8) -> u8 {\n        e\n    }\n}\n",
        "src/setup.rs",
        "use crate::app::App;\n\npub fn register() -> fn(&App, u8) -> u8 {\n    App::handle\n}\n",
        "register",
    ),
    (
        "cpp",
        "app.hpp",
        "class App {\npublic:\n    int handle(int e) { return e; }\n};\n",
        "setup.cpp",
        "#include \"app.hpp\"\n\nauto reg() {\n    return &App::handle;\n}\n",
        "reg",
    ),
];

#[test]
fn a_file_target_lists_the_reader_of_its_method_value_in_every_language_with_one() {
    let mut missing = Vec::new();
    for (language, app, app_text, setup, setup_text, reader) in METHOD_VALUE_READERS {
        let (_temp, workspace, conn) = scanned(&[(app, app_text), (setup, setup_text)]);

        let result = blast_radius_op(&workspace, &conn, None, Some(app), 2, 20).unwrap();

        let rows = impacted(&result);
        if !rows
            .iter()
            .any(|(name, depth, _)| name == reader && *depth == 1)
        {
            missing.push(format!("{language}: {rows:?}"));
        }
    }
    assert!(missing.is_empty(), "{missing:#?}");
}

#[test]
fn a_c_test_that_a_framework_registers_outside_a_test_path_reaches_the_function_it_calls() {
    let (temp, workspace, conn) = scanned(&[
        (
            "src/calc.c",
            "int add(int a, int b) { return a + b; }\nint twice(int a) { return add(a, a); }\n",
        ),
        ("src/calc.h", "int add(int a, int b);\n"),
        (
            "src/config.c",
            "#include \"calc.h\"\nint test_mode(void) { return add(0, 0); }\n",
        ),
        (
            "unit/calc_runner.c",
            "#include \"unity.h\"\n#include \"../src/calc.h\"\nvoid setUp(void) {}\nvoid tearDown(void) {}\nvoid test_adds(void) { TEST_ASSERT_EQUAL(3, add(1, 2)); }\nint main(void) { UNITY_BEGIN(); RUN_TEST(test_adds); return UNITY_END(); }\n",
        ),
        (
            "unit/check_calc.c",
            "#include <check.h>\n#include \"../src/calc.h\"\nSTART_TEST(sums_two)\n{\n    ck_assert_int_eq(add(2, 2), 4);\n}\nEND_TEST\nstatic void subtracts(int _i) { ck_assert_int_eq(add(3, -1), 2); }\nSuite *calc_suite(void) {\n    Suite *s = suite_create(\"calc\");\n    TCase *tc = tcase_create(\"core\");\n    tcase_add_test(tc, sums_two);\n    tcase_add_loop_test(tc, subtracts, 0, 2);\n    suite_add_tcase(s, tc);\n    return s;\n}\n",
        ),
        (
            "unit/cmocka_calc.c",
            "#include <stdarg.h>\n#include <stddef.h>\n#include <setjmp.h>\n#include <cmocka.h>\n#include \"../src/calc.h\"\nstatic void adds_negatives(void **state) { assert_int_equal(add(-1, -1), -2); }\nint main(void) {\n    const struct CMUnitTest tests[] = { cmocka_unit_test(adds_negatives) };\n    return cmocka_run_group_tests(tests, NULL, NULL);\n}\n",
        ),
    ]);
    let expected = [
        ("adds_negatives", "unit/cmocka_calc.c"),
        ("subtracts", "unit/check_calc.c"),
        ("sums_two", "unit/check_calc.c"),
        ("test_adds", "unit/calc_runner.c"),
    ];

    let result =
        blast_radius_op(&workspace, &conn, Some("add"), Some("src/calc.c"), 2, 20).unwrap();
    let mut tests: Vec<(&str, &str)> = result
        .likely_tests
        .iter()
        .map(|test| (test.name.as_str(), test.path.as_str()))
        .collect();
    tests.sort_unstable();
    assert_eq!(tests, expected, "{}", format_blast_radius(&result));
    let impacted: Vec<&str> = result
        .impacted_symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert!(impacted.contains(&"test_mode"), "{impacted:?}");
    assert!(impacted.contains(&"twice"), "{impacted:?}");
    assert!(!impacted.contains(&"main"), "{impacted:?}");

    let db = temp.path().join(".code-kb").join("artifact.db");
    let context = code_kb_core::ops::get_context_slice_op(
        &workspace,
        &db,
        &conn,
        "add",
        Some("src/calc.c"),
        false,
    )
    .unwrap();
    let mut related: Vec<(&str, &str)> = context
        .related_tests
        .iter()
        .map(|test| (test.name.as_str(), test.path.as_str()))
        .collect();
    related.sort_unstable();
    assert_eq!(related, expected);
}
