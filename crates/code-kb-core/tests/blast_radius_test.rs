use rusqlite::Connection;

use code_kb_core::{
    Workspace, blast_radius_op, compute_blast_radius, find_references, find_references_ext,
    format_blast_radius, open_read_write, safe_tempdir,
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
    assert!(formatted.contains("### Likely Tests to Run (1 found)"));
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
        Err(code_kb_core::QueryError::SymbolNotFound(name)) if name == "missing"
    ));
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
            ('c1', 'f3', 'src/caller_alpha.rs', 'rust', 'call_alpha', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO relationships VALUES
            ('c1', 's1', 'calls', 'src/caller_alpha.rs', 1, 0);",
    )
    .unwrap();

    let res = compute_blast_radius(&conn, &["handle"], &["src/alpha.rs"], 1, 20).unwrap();
    assert_eq!(res.impacted_symbols.len(), 1);
    assert_eq!(res.impacted_symbols[0].name, "call_alpha");
}
