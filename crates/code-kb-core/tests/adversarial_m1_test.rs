use code_kb_core::db::{Connection, open_read_write};
use code_kb_core::queries::*;
use code_kb_core::sync::reconcile_offline_edits;
use code_kb_core::workspace::{
    Workspace, normalize_path, parse_file_uri, paths_equal, strip_prefix_lossy,
};
use sha2::Digest;
use std::path::{Path, PathBuf};

#[test]
fn test_sync_reconcile_case_mismatch_and_verbatim() {
    let temp = code_kb_core::safe_tempdir();
    let db_path = temp.path().join("test.db");
    let conn = open_read_write(&db_path).unwrap();

    conn.execute_batch(
        "CREATE TABLE files (
            file_id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
            language TEXT,
            content_hash TEXT,
            content_bytes INTEGER,
            line_count INTEGER,
            indexed_at TEXT
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL
        );",
    )
    .unwrap();

    let src_dir = temp.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let main_file = src_dir.join("main.rs");
    let lib_file = src_dir.join("lib.rs");

    let main_content = "fn main() { println!(\"hello\"); }\n";
    let lib_content = "pub fn add(a: i32, b: i32) -> i32 { a + b }\n";

    std::fs::write(&main_file, main_content).unwrap();
    std::fs::write(&lib_file, lib_content).unwrap();

    let main_hash = hex::encode(sha2::Sha256::digest(main_content.as_bytes()));
    let lib_hash = hex::encode(sha2::Sha256::digest(lib_content.as_bytes()));

    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/main.rs', 'rust', ?1, ?2, 1, '2026-09-14T00:00:00Z')",
        rusqlite::params![main_hash, main_content.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO files VALUES ('f2', 'src/lib.rs', 'rust', ?1, ?2, 1, '2026-09-14T00:00:00Z')",
        rusqlite::params![lib_hash, lib_content.len() as i64],
    )
    .unwrap();

    // 1. Scenario A: Inverted drive letter casing
    let mut ws_cased = Workspace::new(temp.path().to_path_buf());
    #[cfg(windows)]
    {
        let root_str = ws_cased.canonical_root.to_string_lossy().to_string();
        if let Some(first_char) = root_str.chars().next() {
            let flipped = if first_char.is_ascii_uppercase() {
                first_char.to_ascii_lowercase()
            } else {
                first_char.to_ascii_uppercase()
            };
            ws_cased.canonical_root = PathBuf::from(format!("{}{}", flipped, &root_str[1..]));
        }
    }

    let report_cased = reconcile_offline_edits(&ws_cased, &db_path, &conn).unwrap();
    assert!(
        report_cased.deleted.is_empty(),
        "Scenario A (drive case mismatch): Files should not be marked deleted: {:?}",
        report_cased.deleted
    );
    assert!(
        report_cased.modified.is_empty(),
        "Scenario A: Unmodified files should not be marked modified: {:?}",
        report_cased.modified
    );

    // 2. Scenario B: Verbatim prefix on workspace canonical root
    let mut ws_verbatim = Workspace::new(temp.path().to_path_buf());
    #[cfg(windows)]
    {
        let root_str = ws_verbatim.canonical_root.to_string_lossy().to_string();
        if !root_str.starts_with(r"\\?\") {
            ws_verbatim.canonical_root = PathBuf::from(format!(r"\\?\{}", root_str));
        }
    }

    let report_verbatim = reconcile_offline_edits(&ws_verbatim, &db_path, &conn).unwrap();
    assert!(
        report_verbatim.deleted.is_empty(),
        "Scenario B (verbatim root): Files should not be marked deleted: {:?}",
        report_verbatim.deleted
    );
    assert!(
        report_verbatim.modified.is_empty(),
        "Scenario B: Files should not be marked modified: {:?}",
        report_verbatim.modified
    );

    // 3. Scenario C: Trailing slash on canonical root
    let mut ws_trailing = Workspace::new(temp.path().to_path_buf());
    let trailing_str = format!("{}/", ws_trailing.canonical_root.to_string_lossy());
    ws_trailing.canonical_root = PathBuf::from(trailing_str);

    let report_trailing = reconcile_offline_edits(&ws_trailing, &db_path, &conn).unwrap();
    assert!(
        report_trailing.deleted.is_empty(),
        "Scenario C (trailing slash): Files should not be marked deleted: {:?}",
        report_trailing.deleted
    );

    // 4. Scenario D: Real edit on disk with drive case mismatch
    let new_lib_content = "pub fn add(a: i32, b: i32) -> i32 { a + b + 1 }\n";
    std::fs::write(&lib_file, new_lib_content).unwrap();

    let report_edit = reconcile_offline_edits(&ws_cased, &db_path, &conn).unwrap();
    assert_eq!(
        report_edit.modified,
        vec!["src/lib.rs".to_string()],
        "Scenario D: Edited file must be detected as modified even with drive casing mismatch"
    );
    assert!(
        report_edit.deleted.is_empty(),
        "Scenario D: No files should be deleted"
    );
}

#[test]
fn test_queries_collate_nocase_exact_lookups() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files (
            file_id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
            language TEXT,
            content_hash TEXT,
            content_bytes INTEGER,
            line_count INTEGER,
            indexed_at TEXT
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            name TEXT,
            kind TEXT,
            signature TEXT,
            doc_comment TEXT,
            visibility TEXT,
            parent_symbol_id TEXT,
            start_line INTEGER,
            start_column INTEGER,
            end_line INTEGER,
            end_column INTEGER,
            start_byte INTEGER,
            end_byte INTEGER,
            body_start_line INTEGER,
            body_start_column INTEGER,
            body_end_line INTEGER,
            body_end_column INTEGER,
            body_start_byte INTEGER,
            body_end_byte INTEGER,
            body_hash TEXT,
            semantic_group TEXT,
            is_test INTEGER,
            test_container INTEGER
        );
        -- Insert with backslashes and camel-casing
        INSERT INTO files VALUES ('f1', 'src\\Services\\PaymentService.rs', 'rust', 'hash1', 200, 20, '2026-09-14T00:00:00Z');
        INSERT INTO symbols VALUES (
            's1', 'f1', 'src\\Services\\PaymentService.rs', 'rust', 'ProcessPayment', 'function',
            'pub fn ProcessPayment(amount: u64)', 'Processes incoming payment', 'pub', NULL,
            1, 0, 10, 0, 0, 200, 2, 4, 9, 1, 35, 195, 'bhash_proc', 'function', 0, 0
        );",
    )
    .unwrap();

    // 1. get_file with mixed cases and slashes
    let query_variations = [
        "src/Services/PaymentService.rs",
        "SRC/SERVICES/PAYMENTSERVICE.RS",
        "src/services/paymentservice.rs",
        "src\\services\\paymentservice.rs",
        "SRC\\SERVICES\\PAYMENTSERVICE.RS",
    ];

    for q in &query_variations {
        let f = get_file(&conn, q)
            .unwrap()
            .unwrap_or_else(|| panic!("get_file failed for query: {}", q));
        assert_eq!(
            f.path, "src/Services/PaymentService.rs",
            "Path must be strictly normalized to forward slashes for query: {}",
            q
        );
    }

    // 2. load_file_symbols with mixed cases and slashes
    for q in &query_variations {
        let syms = load_file_symbols(&conn, q).unwrap();
        assert_eq!(syms.len(), 1, "load_file_symbols failed for query: {}", q);
        assert_eq!(syms[0].name, "ProcessPayment");
        assert_eq!(
            syms[0].path, "src/Services/PaymentService.rs",
            "Symbol path must be strictly normalized to forward slashes for query: {}",
            q
        );
    }

    // 3. get_symbol_by_name with exact full-path filters
    let full_path_filters = [
        Some("SRC/SERVICES/PAYMENTSERVICE.RS"),
        Some("src/services/paymentservice.rs"),
        Some("src\\Services\\PaymentService.rs"),
    ];

    for pf in &full_path_filters {
        let sym = get_symbol_by_name(&conn, "ProcessPayment", *pf)
            .unwrap()
            .unwrap_or_else(|| panic!("get_symbol_by_name failed for filter: {:?}", pf));
        assert_eq!(sym.name, "ProcessPayment");
        assert_eq!(sym.path, "src/Services/PaymentService.rs");
    }

    // 4. get_symbol_by_name_exact with full path filters
    let exact_filters = [
        "src/Services/PaymentService.rs",
        "SRC/SERVICES/PAYMENTSERVICE.RS",
        "src/services/paymentservice.rs",
        "src\\services\\paymentservice.rs",
    ];

    for ef in &exact_filters {
        let sym = get_symbol_by_name_exact(&conn, "ProcessPayment", ef)
            .unwrap()
            .unwrap_or_else(|| panic!("get_symbol_by_name_exact failed for filter: {}", ef));
        assert_eq!(sym.name, "ProcessPayment");
        assert_eq!(sym.path, "src/Services/PaymentService.rs");
    }
}

#[test]
fn test_forward_slash_invariants_across_internal_models() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files (
            file_id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
            language TEXT,
            content_hash TEXT,
            content_bytes INTEGER,
            line_count INTEGER,
            indexed_at TEXT
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            name TEXT,
            kind TEXT,
            signature TEXT,
            doc_comment TEXT,
            visibility TEXT,
            parent_symbol_id TEXT,
            start_line INTEGER,
            start_column INTEGER,
            end_line INTEGER,
            end_column INTEGER,
            start_byte INTEGER,
            end_byte INTEGER,
            body_start_line INTEGER,
            body_start_column INTEGER,
            body_end_line INTEGER,
            body_end_column INTEGER,
            body_start_byte INTEGER,
            body_end_byte INTEGER,
            body_hash TEXT,
            semantic_group TEXT,
            is_test INTEGER,
            test_container INTEGER
        );
        CREATE TABLE relationships (
            relationship_id TEXT PRIMARY KEY,
            from_symbol_id TEXT,
            to_symbol_id TEXT,
            kind TEXT,
            path TEXT,
            start_line INTEGER,
            start_column INTEGER
        );
        CREATE TABLE pending_relationships (
            from_symbol_id TEXT,
            target_terminal_name TEXT,
            kind TEXT,
            path TEXT,
            start_line INTEGER,
            start_column INTEGER
        );
        CREATE TABLE structural_facts (
            structural_fact_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            pattern_id TEXT,
            capture_name TEXT,
            node_kind TEXT,
            containing_symbol_id TEXT,
            start_line INTEGER,
            end_line INTEGER,
            confidence REAL
        );
        CREATE TABLE literals (
            literal_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            kind TEXT,
            literal_text TEXT,
            carrier TEXT,
            containing_symbol_id TEXT,
            start_line INTEGER,
            start_column INTEGER,
            end_line INTEGER,
            end_column INTEGER,
            start_byte INTEGER,
            end_byte INTEGER
        );

        -- Insert all data using Windows backslashes
        INSERT INTO files VALUES ('f1', 'src\\models\\user.rs', 'rust', 'h1', 100, 10, '2026-09-14');
        INSERT INTO symbols VALUES (
            's1', 'f1', 'src\\models\\user.rs', 'rust', 'User', 'struct',
            'pub struct User', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 100,
            1, 0, 5, 0, 0, 100, 'hash1', 'struct', 0, 0
        );
        INSERT INTO symbols VALUES (
            's2', 'f1', 'src\\models\\user.rs', 'rust', 'save_user', 'function',
            'pub fn save_user()', NULL, 'pub', NULL, 6, 0, 10, 0, 101, 200,
            6, 0, 10, 0, 101, 200, 'hash2', 'function', 0, 0
        );
        INSERT INTO relationships VALUES ('r1', 's2', 's1', 'constructs', 'src\\models\\user.rs', 7, 4);
        INSERT INTO structural_facts VALUES (
            'fact1', 'f1', 'src\\models\\user.rs', 'rust', 'route', 'route',
            'endpoint', 's2', 6, 6, 1.0
        );
        INSERT INTO literals VALUES (
            'lit1', 'f1', 'src\\models\\user.rs', 'rust', 'string',
            'user_table', 'carrier1', 's2', 8, 14, 8, 24, 150, 160
        );",
    )
    .unwrap();

    // 1. load_scoped_files
    let files = load_scoped_files(&conn, None).unwrap();
    for f in files {
        assert!(
            !f.path.contains('\\'),
            "FileFact path contains backslash: {}",
            f.path
        );
    }

    // 2. find_references (callers)
    let callers = find_references(&conn, "User", "callers", 10).unwrap();
    for r in callers {
        assert!(
            !r.path.contains('\\'),
            "ReferenceSite path contains backslash: {}",
            r.path
        );
    }

    // 3. find_callee_signatures (internal only)
    let callee_sigs = find_callee_signatures(&conn, "save_user", "s2", 10, false).unwrap();
    for entry in callee_sigs {
        assert!(
            !entry.contains('\\'),
            "Callee signature contains backslash: {}",
            entry
        );
    }

    // 4. find_structural_facts
    let facts = find_structural_facts(&conn, "%", 10).unwrap();
    for fact in facts {
        assert!(
            !fact.path.contains('\\'),
            "StructuralFact path contains backslash: {}",
            fact.path
        );
    }

    // 5. find_literals
    let lits = find_literals(&conn, "%", 10).unwrap();
    for lit in lits {
        assert!(
            !lit.path.contains('\\'),
            "LiteralFact path contains backslash: {}",
            lit.path
        );
    }

    // 6. compute_blast_radius
    let blast = compute_blast_radius(&conn, &["User"], &[], 2, 10).unwrap();
    for sym in blast.impacted_symbols {
        assert!(
            !sym.path.contains('\\'),
            "ImpactedSymbol path contains backslash: {}",
            sym.path
        );
    }
    for test in blast.likely_tests {
        assert!(
            !test.path.contains('\\'),
            "TestTarget path contains backslash: {}",
            test.path
        );
    }
}

#[test]
fn test_paths_equal_adversarial_stress() {
    assert!(paths_equal(
        Path::new("src/lib.rs"),
        Path::new("src/lib.rs")
    ));
    assert!(paths_equal(
        Path::new("src/lib.rs"),
        Path::new("src\\lib.rs")
    ));
    assert!(!paths_equal(
        Path::new("src/lib.rs"),
        Path::new("src/main.rs")
    ));

    #[cfg(windows)]
    {
        // 1. Casing variations
        assert!(paths_equal(
            Path::new(r"C:\source\code-kb\src\lib.rs"),
            Path::new(r"c:\source\code-kb\src\lib.rs")
        ));
        assert!(paths_equal(
            Path::new(r"C:\Source\Code-Kb\Src\Lib.rs"),
            Path::new(r"c:\source\code-kb\src\lib.rs")
        ));

        // 2. Trailing slashes
        assert!(paths_equal(
            Path::new(r"C:\source\code-kb\"),
            Path::new(r"c:\source\code-kb")
        ));
        assert!(paths_equal(
            Path::new(r"C:/source/code-kb/"),
            Path::new(r"c:\source\code-kb")
        ));

        // 3. Verbatim prefixes vs standard
        assert!(paths_equal(
            Path::new(r"\\?\C:\source\code-kb\src\lib.rs"),
            Path::new(r"C:\source\code-kb\src\lib.rs")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\c:\source\code-kb\src\lib.rs"),
            Path::new(r"C:\source\code-kb\src\lib.rs")
        ));

        // 4. Verbatim with mixed slashes
        assert!(paths_equal(
            Path::new(r"\\?\C:/source/code-kb/src/lib.rs"),
            Path::new(r"c:\source\code-kb\src\lib.rs")
        ));

        // 5. Different drive letters must NOT match
        assert!(!paths_equal(
            Path::new(r"C:\source\code-kb\src\lib.rs"),
            Path::new(r"D:\source\code-kb\src\lib.rs")
        ));

        // 6. Prefix path vs longer path must NOT match
        assert!(!paths_equal(
            Path::new(r"C:\source\code-kb"),
            Path::new(r"C:\source\code-kb\src")
        ));

        // 7. UNC paths
        assert!(paths_equal(
            Path::new(r"\\server\share\file.txt"),
            Path::new(r"\\SERVER\SHARE\file.txt")
        ));
    }
}

#[test]
fn test_parse_file_uri_standard_variations() {
    #[cfg(windows)]
    {
        // Two-slash
        let p1 = parse_file_uri("file://C:/projects/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p1,
            normalize_path(Path::new(r"C:\projects\code-kb\src\lib.rs"))
        );

        // Three-slash
        let p2 = parse_file_uri("file:///C:/projects/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p2,
            normalize_path(Path::new(r"C:\projects\code-kb\src\lib.rs"))
        );

        // Lowercase drive letter
        let p3 = parse_file_uri("file://c:/projects/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p3,
            normalize_path(Path::new(r"c:\projects\code-kb\src\lib.rs"))
        );

        // Percent-encoded spaces
        let p4 = parse_file_uri("file:///C:/My%20Projects/Code%20KB/lib.rs").unwrap();
        assert_eq!(
            p4,
            normalize_path(Path::new(r"C:\My Projects\Code KB\lib.rs"))
        );

        let p5 = parse_file_uri("file://C:/My%20Projects/Code%20KB/lib.rs").unwrap();
        assert_eq!(
            p5,
            normalize_path(Path::new(r"C:\My Projects\Code KB\lib.rs"))
        );
    }
}

#[test]
fn test_strip_prefix_lossy_adversarial_stress() {
    #[cfg(windows)]
    {
        // 1. Verbatim path vs plain base
        let path = Path::new(r"\\?\C:\repo\src\main.rs");
        let base = Path::new(r"C:\repo");
        let rel = strip_prefix_lossy(path, base).expect("Must strip prefix with verbatim path");
        assert_eq!(
            code_kb_core::workspace::to_forward_slash(rel),
            "src/main.rs"
        );

        // 2. Plain path vs verbatim base
        let path = Path::new(r"C:\repo\src\main.rs");
        let base = Path::new(r"\\?\C:\repo");
        let rel = strip_prefix_lossy(path, base).expect("Must strip prefix with verbatim base");
        assert_eq!(
            code_kb_core::workspace::to_forward_slash(rel),
            "src/main.rs"
        );

        // 3. Drive casing difference
        let path = Path::new(r"c:\repo\src\main.rs");
        let base = Path::new(r"C:\repo");
        let rel =
            strip_prefix_lossy(path, base).expect("Must strip prefix with drive casing difference");
        assert_eq!(
            code_kb_core::workspace::to_forward_slash(rel),
            "src/main.rs"
        );

        // 4. Directory casing difference
        let path = Path::new(r"C:\REPO\src\main.rs");
        let base = Path::new(r"C:\repo");
        let rel = strip_prefix_lossy(path, base)
            .expect("Must strip prefix with directory casing difference");
        assert_eq!(
            code_kb_core::workspace::to_forward_slash(rel),
            "src/main.rs"
        );

        // 5. Trailing slash on base
        let path = Path::new(r"C:\repo\src\main.rs");
        let base = Path::new(r"C:\repo\");
        let rel =
            strip_prefix_lossy(path, base).expect("Must strip prefix with trailing slash on base");
        assert_eq!(
            code_kb_core::workspace::to_forward_slash(rel),
            "src/main.rs"
        );

        // 6. Forward slashes in base
        let path = Path::new(r"C:\repo\src\main.rs");
        let base = Path::new("C:/repo");
        let rel =
            strip_prefix_lossy(path, base).expect("Must strip prefix with forward slashes in base");
        assert_eq!(
            code_kb_core::workspace::to_forward_slash(rel),
            "src/main.rs"
        );

        // 7. Different drives must return None
        let path = Path::new(r"D:\repo\src\main.rs");
        let base = Path::new(r"C:\repo");
        assert!(strip_prefix_lossy(path, base).is_none());

        // 8. Path shorter than base must return None
        let path = Path::new(r"C:\repo");
        let base = Path::new(r"C:\repo\src");
        assert!(strip_prefix_lossy(path, base).is_none());
    }
}

// ------------------------------------------------------------------------------------------------
// EMPIRICAL BUG REPRODUCTIONS & REMEDIATION VERIFICATIONS
// The following 4 tests verify remediation of specific bugs discovered during adversarial stress testing.
// ------------------------------------------------------------------------------------------------

/// Bug 1 Remediation: `find_callee_signatures` emits forward slashes for external callees
/// even when `pending_relationships.path` contains Windows backslashes.
#[test]
fn test_bug_repro_find_callee_signatures_external_path_backslash() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            name TEXT,
            kind TEXT,
            signature TEXT,
            doc_comment TEXT,
            visibility TEXT,
            parent_symbol_id TEXT,
            start_line INTEGER,
            start_column INTEGER,
            end_line INTEGER,
            end_column INTEGER,
            start_byte INTEGER,
            end_byte INTEGER,
            body_start_line INTEGER,
            body_start_column INTEGER,
            body_end_line INTEGER,
            body_end_column INTEGER,
            body_start_byte INTEGER,
            body_end_byte INTEGER,
            body_hash TEXT,
            semantic_group TEXT,
            is_test INTEGER,
            test_container INTEGER
        );
        CREATE TABLE relationships (
            relationship_id TEXT PRIMARY KEY,
            from_symbol_id TEXT,
            to_symbol_id TEXT,
            kind TEXT,
            path TEXT,
            start_line INTEGER,
            start_column INTEGER
        );
        CREATE TABLE pending_relationships (
            from_symbol_id TEXT,
            target_terminal_name TEXT,
            kind TEXT,
            path TEXT,
            start_line INTEGER,
            start_column INTEGER
        );
        INSERT INTO symbols VALUES (
            's1', 'f1', 'src\\models\\user.rs', 'rust', 'save_user', 'function',
            'pub fn save_user()', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'hash1', 'function', 0, 0
        );
        -- Insert pending external callee with Windows backslashes in path
        INSERT INTO pending_relationships VALUES ('s1', 'db_insert', 'call', 'src\\models\\user.rs', 8, 4);",
    )
    .unwrap();

    let callee_sigs = find_callee_signatures(&conn, "save_user", "s1", 10, true).unwrap();
    assert_eq!(callee_sigs.len(), 1);
    assert!(
        !callee_sigs[0].contains('\\'),
        "External callee signature must strictly format paths with forward slashes: {}",
        callee_sigs[0]
    );
    assert_eq!(
        callee_sigs[0], "db_insert (src/models/user.rs:8)",
        "External callee signature path must match expected forward slash string"
    );
}

/// Bug 2 Remediation: `get_symbol_by_name` partial/suffix path filter succeeds when SQLite has backslashes
/// using both forward-slash and backslash patterns with literal backslash matching.
#[test]
fn test_bug_repro_get_symbol_by_name_suffix_filter_with_backslash_db() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            name TEXT,
            kind TEXT,
            signature TEXT,
            doc_comment TEXT,
            visibility TEXT,
            parent_symbol_id TEXT,
            start_line INTEGER,
            start_column INTEGER,
            end_line INTEGER,
            end_column INTEGER,
            start_byte INTEGER,
            end_byte INTEGER,
            body_start_line INTEGER,
            body_start_column INTEGER,
            body_end_line INTEGER,
            body_end_column INTEGER,
            body_start_byte INTEGER,
            body_end_byte INTEGER,
            body_hash TEXT,
            semantic_group TEXT,
            is_test INTEGER,
            test_container INTEGER
        );
        -- Insert with backslashes
        INSERT INTO symbols VALUES (
            's1', 'f1', 'src\\Services\\PaymentService.rs', 'rust', 'ProcessPayment', 'function',
            'pub fn ProcessPayment()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
            1, 0, 5, 0, 0, 50, 'hash1', 'function', 0, 0
        );",
    )
    .unwrap();

    // 1. Filename suffix filter "PaymentService.rs"
    let sym = get_symbol_by_name(&conn, "ProcessPayment", Some("PaymentService.rs")).unwrap();
    assert!(
        sym.is_some(),
        "Suffix filter must find symbol when DB contains backslashes"
    );
    let s = sym.unwrap();
    assert_eq!(s.name, "ProcessPayment");
    assert_eq!(s.path, "src/Services/PaymentService.rs");

    // 2. Multi-segment suffix filter with forward slash
    let sym2 =
        get_symbol_by_name(&conn, "ProcessPayment", Some("Services/PaymentService.rs")).unwrap();
    assert!(
        sym2.is_some(),
        "Multi-segment forward slash suffix filter must find symbol when DB contains backslashes"
    );

    // 3. Multi-segment suffix filter with backslash
    let sym3 =
        get_symbol_by_name(&conn, "ProcessPayment", Some(r"Services\PaymentService.rs")).unwrap();
    assert!(
        sym3.is_some(),
        "Multi-segment backslash suffix filter must find symbol when DB contains backslashes"
    );

    // 4. Case-insensitive suffix filter
    let sym4 = get_symbol_by_name(&conn, "ProcessPayment", Some("paymentservice.rs")).unwrap();
    assert!(
        sym4.is_some(),
        "Case-insensitive suffix filter must find symbol when DB contains backslashes"
    );

    // 5. Negative boundary test: ensure substring without path separator does not match
    let sym5 = get_symbol_by_name(&conn, "ProcessPayment", Some("FooPaymentService.rs")).unwrap();
    assert!(
        sym5.is_none(),
        "Substring match without path separator must not match"
    );
}

/// Bug 3 Remediation: `load_scoped_files` and `load_scoped_outline_symbols` match case-insensitively
/// and support both forward-slash and backslash paths with dual-separator depth bounding.
#[test]
fn test_bug_repro_load_scoped_files_case_insensitivity() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files (
            file_id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
            language TEXT,
            content_hash TEXT,
            content_bytes INTEGER,
            line_count INTEGER,
            indexed_at TEXT
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            name TEXT,
            kind TEXT,
            signature TEXT,
            doc_comment TEXT,
            visibility TEXT,
            parent_symbol_id TEXT,
            start_line INTEGER,
            start_column INTEGER,
            end_line INTEGER,
            end_column INTEGER,
            start_byte INTEGER,
            end_byte INTEGER,
            body_start_line INTEGER,
            body_start_column INTEGER,
            body_end_line INTEGER,
            body_end_column INTEGER,
            body_start_byte INTEGER,
            body_end_byte INTEGER,
            body_hash TEXT,
            semantic_group TEXT,
            is_test INTEGER,
            test_container INTEGER
        );
        -- Insert forward slash file
        INSERT INTO files VALUES ('f1', 'src/Services/PaymentService.rs', 'rust', 'h1', 100, 10, '2026-09-14');
        -- Insert backslash file
        INSERT INTO files VALUES ('f2', 'src\\Services\\OtherService.rs', 'rust', 'h2', 150, 15, '2026-09-14');
        INSERT INTO symbols VALUES (
            's1', 'f1', 'src/Services/PaymentService.rs', 'rust', 'process', 'function',
            'pub fn process()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh1', 'function', 0, 0
        );
        INSERT INTO symbols VALUES (
            's2', 'f2', 'src\\Services\\OtherService.rs', 'rust', 'other', 'function',
            'pub fn other()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh2', 'function', 0, 0
        );",
    )
    .unwrap();

    // 1. Exact path lookup with uppercase
    let files = load_scoped_files(&conn, Some("SRC/SERVICES/PAYMENTSERVICE.RS")).unwrap();
    assert_eq!(
        files.len(),
        1,
        "load_scoped_files must find uppercase forward slash file"
    );
    assert_eq!(files[0].path, "src/Services/PaymentService.rs");

    // 2. Exact backslash path lookup with uppercase
    let files_bs = load_scoped_files(&conn, Some("SRC\\SERVICES\\OTHERSERVICE.RS")).unwrap();
    assert_eq!(
        files_bs.len(),
        1,
        "load_scoped_files must find uppercase backslash file"
    );
    assert_eq!(files_bs[0].path, "src/Services/OtherService.rs");

    // 3. Directory scope with mixed case and forward slash
    let scoped_fwd = load_scoped_files(&conn, Some("SRC/SERVICES")).unwrap();
    assert_eq!(
        scoped_fwd.len(),
        2,
        "Directory scope with forward slash must find both files"
    );

    // 4. Directory scope with mixed case and backslash
    let scoped_bs = load_scoped_files(&conn, Some("src\\services")).unwrap();
    assert_eq!(
        scoped_bs.len(),
        2,
        "Directory scope with backslash must find both files"
    );

    // 5. Scoped outline symbols
    let syms = load_scoped_outline_symbols(&conn, Some("SRC/SERVICES"), 2, 5).unwrap();
    assert_eq!(
        syms.len(),
        2,
        "Scoped outline symbols must find symbols for both files"
    );
    assert!(syms.contains_key("src/Services/PaymentService.rs"));
    assert!(syms.contains_key("src/Services/OtherService.rs"));

    // 6. End-to-end codebase_outline_op with case-insensitive scope and backslashes
    let ws = Workspace::new(PathBuf::from(r"C:\test_repo"));
    let outline_dir =
        code_kb_core::codebase_outline_op(&ws, &conn, 3, Some("SRC/SERVICES")).unwrap();
    assert!(
        outline_dir.contains("PaymentService.rs"),
        "Outline must contain PaymentService.rs: {}",
        outline_dir
    );
    assert!(
        outline_dir.contains("OtherService.rs"),
        "Outline must contain OtherService.rs: {}",
        outline_dir
    );

    let outline_file =
        code_kb_core::codebase_outline_op(&ws, &conn, 3, Some("SRC/SERVICES/PAYMENTSERVICE.RS"))
            .unwrap();
    assert!(
        outline_file.contains("PaymentService.rs"),
        "Outline for single file must contain PaymentService.rs: {}",
        outline_file
    );
}

/// Bug 4 Remediation: `parse_file_uri` converts pipe character `|` into a valid drive colon `:`
#[test]
fn test_bug_repro_parse_file_uri_pipe_character() {
    #[cfg(windows)]
    {
        // 1. Three-slash uppercase
        let p1 = parse_file_uri("file:///C|/projects/code-kb/src/lib.rs").unwrap();
        assert!(
            !p1.to_string_lossy().contains('|'),
            "parse_file_uri emitted invalid path containing pipe character: {:?}",
            p1
        );
        assert_eq!(
            p1,
            normalize_path(Path::new(r"C:\projects\code-kb\src\lib.rs"))
        );

        // 2. Two-slash uppercase
        let p2 = parse_file_uri("file://C|/projects/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p2,
            normalize_path(Path::new(r"C:\projects\code-kb\src\lib.rs"))
        );

        // 3. Lowercase drive with pipe
        let p3 = parse_file_uri("file:///c|/projects/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p3,
            normalize_path(Path::new(r"c:\projects\code-kb\src\lib.rs"))
        );

        // 4. Percent-encoded pipe %7C and %7c
        let p4 = parse_file_uri("file:///C%7C/projects/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p4,
            normalize_path(Path::new(r"C:\projects\code-kb\src\lib.rs"))
        );

        let p5 = parse_file_uri("file://C%7c/projects/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p5,
            normalize_path(Path::new(r"C:\projects\code-kb\src\lib.rs"))
        );

        // 5. Plain path with pipe
        let p6 = parse_file_uri("C|/projects/code-kb/src/lib.rs").unwrap();
        assert_eq!(
            p6,
            normalize_path(Path::new(r"C:\projects\code-kb\src\lib.rs"))
        );

        // 6. Mixed backslashes with pipe
        let p7 = parse_file_uri(r"file:///C|\projects\code-kb\src\lib.rs").unwrap();
        assert_eq!(
            p7,
            normalize_path(Path::new(r"C:\projects\code-kb\src\lib.rs"))
        );
    }
}

/// Adversarial Stress Test: Defect 3 Scoped outline queries, depth limits, and mixed slash/casing combinations.
#[test]
fn test_adversarial_stress_scoped_outline_and_depth_limits() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files (
            file_id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
            language TEXT,
            content_hash TEXT,
            content_bytes INTEGER,
            line_count INTEGER,
            indexed_at TEXT
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            name TEXT,
            kind TEXT,
            signature TEXT,
            doc_comment TEXT,
            visibility TEXT,
            parent_symbol_id TEXT,
            start_line INTEGER,
            start_column INTEGER,
            end_line INTEGER,
            end_column INTEGER,
            start_byte INTEGER,
            end_byte INTEGER,
            body_start_line INTEGER,
            body_start_column INTEGER,
            body_end_line INTEGER,
            body_end_column INTEGER,
            body_start_byte INTEGER,
            body_end_byte INTEGER,
            body_hash TEXT,
            semantic_group TEXT,
            is_test INTEGER,
            test_container INTEGER
        );
        -- Root files (depth 0 relative to root)
        INSERT INTO files VALUES ('f0_1', 'Cargo.toml', 'toml', 'h0_1', 50, 5, '2026-09-14');
        INSERT INTO files VALUES ('f0_2', 'README.md', 'markdown', 'h0_2', 100, 10, '2026-09-14');
        -- Level 1 files (1 separator)
        INSERT INTO files VALUES ('f1_1', 'src/lib.rs', 'rust', 'h1_1', 120, 12, '2026-09-14');
        INSERT INTO files VALUES ('f1_2', 'src\\main.rs', 'rust', 'h1_2', 130, 13, '2026-09-14');
        -- Level 2 files (2 separators, mixed slashes and mixed casing)
        INSERT INTO files VALUES ('f2_1', 'src/Services/AuthService.rs', 'rust', 'h2_1', 200, 20, '2026-09-14');
        INSERT INTO files VALUES ('f2_2', 'src\\Services\\PaymentService.rs', 'rust', 'h2_2', 210, 21, '2026-09-14');
        INSERT INTO files VALUES ('f2_3', 'src/models\\User.rs', 'rust', 'h2_3', 220, 22, '2026-09-14');
        -- Level 3 files (3 separators)
        INSERT INTO files VALUES ('f3_1', 'src\\Services\\Auth\\Token.rs', 'rust', 'h3_1', 300, 30, '2026-09-14');
        INSERT INTO files VALUES ('f3_2', 'src/Services/Auth/Session.rs', 'rust', 'h3_2', 310, 31, '2026-09-14');
        -- Level 4 file (4 separators)
        INSERT INTO files VALUES ('f4_1', 'src\\Services\\Auth\\Crypto\\Keys.rs', 'rust', 'h4_1', 400, 40, '2026-09-14');
        -- Special directory characters to stress escape_like: '_' vs '-' and '%'
        INSERT INTO files VALUES ('f_spec1', 'special_dir/alpha.rs', 'rust', 'h_s1', 50, 5, '2026-09-14');
        INSERT INTO files VALUES ('f_spec2', 'special-dir/beta.rs', 'rust', 'h_s2', 50, 5, '2026-09-14');
        INSERT INTO files VALUES ('f_spec3', 'special%20dir/gamma.rs', 'rust', 'h_s3', 50, 5, '2026-09-14');

        -- Symbols corresponding to files
        INSERT INTO symbols VALUES ('s0_1', 'f0_1', 'Cargo.toml', 'toml', 'package', 'struct', 'package', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh0_1', 'struct', 0, 0);
        INSERT INTO symbols VALUES ('s1_1', 'f1_1', 'src/lib.rs', 'rust', 'lib_init', 'function', 'pub fn lib_init()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh1_1', 'function', 0, 0);
        INSERT INTO symbols VALUES ('s1_2', 'f1_2', 'src\\main.rs', 'rust', 'main', 'function', 'fn main()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh1_2', 'function', 0, 0);
        INSERT INTO symbols VALUES ('s2_1', 'f2_1', 'src/Services/AuthService.rs', 'rust', 'login', 'function', 'pub fn login()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh2_1', 'function', 0, 0);
        INSERT INTO symbols VALUES ('s2_2', 'f2_2', 'src\\Services\\PaymentService.rs', 'rust', 'pay', 'function', 'pub fn pay()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh2_2', 'function', 0, 0);
        INSERT INTO symbols VALUES ('s3_1', 'f3_1', 'src\\Services\\Auth\\Token.rs', 'rust', 'validate_token', 'function', 'pub fn validate_token()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh3_1', 'function', 0, 0);
        INSERT INTO symbols VALUES ('s4_1', 'f4_1', 'src\\Services\\Auth\\Crypto\\Keys.rs', 'rust', 'generate_key', 'function', 'pub fn generate_key()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh4_1', 'function', 0, 0);
        INSERT INTO symbols VALUES ('s_sp1', 'f_spec1', 'special_dir/alpha.rs', 'rust', 'alpha_fn', 'function', 'pub fn alpha_fn()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50, 1, 0, 5, 0, 0, 50, 'bh_sp1', 'function', 0, 0);
        ",
    ).unwrap();

    let ws = Workspace::new(PathBuf::from(r"C:\test_project"));

    // 1. Root-level outline depth tests
    // depth 1: Only files with 0 slashes (Cargo.toml, README.md)
    let syms_d1 = load_scoped_outline_symbols(&conn, None, 1, 5).unwrap();
    assert!(syms_d1.contains_key("Cargo.toml"));
    assert!(!syms_d1.contains_key("src/lib.rs"));
    assert!(!syms_d1.contains_key("src/main.rs"));

    // depth 2: Root files and files with 1 slash
    let syms_d2 = load_scoped_outline_symbols(&conn, None, 2, 5).unwrap();
    assert!(syms_d2.contains_key("Cargo.toml"));
    assert!(syms_d2.contains_key("src/lib.rs"));
    assert!(syms_d2.contains_key("src/main.rs")); // backslash row normalized to forward slash
    assert!(!syms_d2.contains_key("src/Services/AuthService.rs"));

    // 2. Scoped outline depth tests under "src/Services"
    let filter_cases = [
        "src/Services",
        "SRC/SERVICES",
        r"src\Services",
        r"SRC\SERVICES",
        "src/Services/",
        r"src\Services\",
    ];

    for filter in &filter_cases {
        // Scope depth 1: only immediate children (AuthService.rs, PaymentService.rs)
        let syms_scoped_d1 = load_scoped_outline_symbols(&conn, Some(*filter), 1, 5).unwrap();
        assert!(
            syms_scoped_d1.contains_key("src/Services/AuthService.rs"),
            "Filter {} depth 1 must contain AuthService",
            filter
        );
        assert!(
            syms_scoped_d1.contains_key("src/Services/PaymentService.rs"),
            "Filter {} depth 1 must contain PaymentService",
            filter
        );
        assert!(
            !syms_scoped_d1.contains_key("src/Services/Auth/Token.rs"),
            "Filter {} depth 1 must NOT contain Token.rs",
            filter
        );
        assert!(
            !syms_scoped_d1.contains_key("src/Services/Auth/Crypto/Keys.rs"),
            "Filter {} depth 1 must NOT contain Keys.rs",
            filter
        );

        // Scope depth 2: includes Token.rs and Session.rs
        let syms_scoped_d2 = load_scoped_outline_symbols(&conn, Some(*filter), 2, 5).unwrap();
        assert!(syms_scoped_d2.contains_key("src/Services/AuthService.rs"));
        assert!(syms_scoped_d2.contains_key("src/Services/PaymentService.rs"));
        assert!(syms_scoped_d2.contains_key("src/Services/Auth/Token.rs"));
        assert!(
            !syms_scoped_d2.contains_key("src/Services/Auth/Crypto/Keys.rs"),
            "Filter {} depth 2 must NOT contain Keys.rs",
            filter
        );

        // Scope depth 3: includes Keys.rs
        let syms_scoped_d3 = load_scoped_outline_symbols(&conn, Some(*filter), 3, 5).unwrap();
        assert!(syms_scoped_d3.contains_key("src/Services/Auth/Crypto/Keys.rs"));

        // End-to-end codebase_outline_op verification
        let outline = code_kb_core::codebase_outline_op(&ws, &conn, 1, Some(*filter)).unwrap();
        assert!(
            outline.contains("AuthService.rs"),
            "Outline must contain AuthService.rs: {}",
            outline
        );
        assert!(
            outline.contains("PaymentService.rs"),
            "Outline must contain PaymentService.rs: {}",
            outline
        );
        assert!(
            outline.contains("login"),
            "Outline must contain symbol login: {}",
            outline
        );
        assert!(
            outline.contains("pay"),
            "Outline must contain symbol pay: {}",
            outline
        );
        assert!(
            !outline.contains("generate_key"),
            "Outline depth 1 must not contain generate_key: {}",
            outline
        );
    }

    // 3. Stress-testing LIKE wildcard escaping with underscore and percent
    let underscore_files = load_scoped_files(&conn, Some("special_dir")).unwrap();
    assert_eq!(underscore_files.len(), 1);
    assert_eq!(underscore_files[0].path, "special_dir/alpha.rs");

    let dash_files = load_scoped_files(&conn, Some("special-dir")).unwrap();
    assert_eq!(dash_files.len(), 1);
    assert_eq!(dash_files[0].path, "special-dir/beta.rs");

    let percent_files = load_scoped_files(&conn, Some("special%20dir")).unwrap();
    assert_eq!(percent_files.len(), 1);
    assert_eq!(percent_files[0].path, "special%20dir/gamma.rs");

    // 4. Non-existent filter returns OpError::FileNotFound
    let missing_err =
        code_kb_core::codebase_outline_op(&ws, &conn, 2, Some("nonexistent/dir")).unwrap_err();
    match missing_err {
        code_kb_core::ops::OpError::FileNotFound(f) => assert_eq!(f, "nonexistent/dir"),
        other => panic!("Expected FileNotFound, got {:?}", other),
    }
}

/// Adversarial Stress Test: Defect 4 Comprehensive matrix for file URI pipe normalization.
#[test]
fn test_adversarial_stress_parse_file_uri_comprehensive_matrix() {
    #[cfg(windows)]
    {
        // 1. Drive roots with trailing slash
        let valid_roots = [
            ("file:///C|/", r"C:\"),
            ("file://C|/", r"C:\"),
            ("C|/", r"C:\"),
            ("C|\\", r"C:\"),
        ];
        for (uri, expected) in &valid_roots {
            let res =
                parse_file_uri(uri).unwrap_or_else(|| panic!("Failed to parse root URI: {}", uri));
            assert!(
                !res.to_string_lossy().contains('|'),
                "Result for '{}' contains pipe: {:?}",
                uri,
                res
            );
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for '{}'",
                uri
            );
        }

        // Defect 4.1 Remediation: parse_file_uri handles drive roots without trailing slashes
        // without panicking in url::Url::to_file_path().
        let root_no_slash_cases = [
            ("file:///C|", r"C:\"),
            ("file://C|", r"C:\"),
            ("file:///c|", r"c:\"),
            ("file://c|", r"c:\"),
            ("file:///C:", r"C:\"),
            ("file://C:", r"C:\"),
            ("file:///c:", r"c:\"),
            ("file://c:", r"c:\"),
            ("file:///C%7C", r"C:\"),
            ("file://C%7c", r"C:\"),
            ("file:///C%3A", r"C:\"),
            ("file://c%3a", r"c:\"),
            ("file:///C|?query=foo", r"C:\"),
            ("file:///C|#anchor", r"C:\"),
            ("file://localhost/C|", r"C:\"),
            ("file://localhost/C|/", r"C:\"),
            ("file://localhost/c|", r"c:\"),
            ("file://LOCALHOST/C|", r"C:\"),
        ];
        for (uri, expected) in &root_no_slash_cases {
            let panic_res = std::panic::catch_unwind(|| parse_file_uri(uri));
            assert!(
                panic_res.is_ok(),
                "Expected parse_file_uri('{}') NOT to panic",
                uri
            );
            let res = panic_res
                .unwrap()
                .unwrap_or_else(|| panic!("Expected parse_file_uri('{}') to return Some", uri));
            assert!(
                !res.to_string_lossy().contains('|'),
                "Result for '{}' contains invalid pipe: {:?}",
                uri,
                res
            );
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for '{}'",
                uri
            );
        }

        // Defect 4.2 Remediation: file://localhost with pipe delimiter normalizes cleanly without retaining pipe
        let localhost_pipe_cases = [
            ("file://localhost/C|/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file://localhost/c|/path/to/file.rs", r"c:\path\to\file.rs"),
            ("file://LOCALHOST/C|/path/to/file.rs", r"C:\path\to\file.rs"),
            (
                r"file://localhost\C|\path\to\file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                "file:///localhost/C|/path/to/file.rs",
                r"C:\path\to\file.rs",
            ),
        ];
        for (uri, expected) in &localhost_pipe_cases {
            let res =
                parse_file_uri(uri).unwrap_or_else(|| panic!("Failed for localhost URI: {}", uri));
            assert!(
                !res.to_string_lossy().contains('|'),
                "Expected file://localhost pipe URI '{}' to normalize without pipe: {:?}",
                uri,
                res
            );
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for '{}'",
                uri
            );
        }

        // 2. Drive letter casing and variety (D, e, Z, y)
        let drives = [
            ("file:///D|/path/to/file.rs", r"D:\path\to\file.rs"),
            ("file://d|/path/to/file.rs", r"d:\path\to\file.rs"),
            ("file:///Z|/deep/dir/mod.rs", r"Z:\deep\dir\mod.rs"),
            ("file://y|/deep/dir/mod.rs", r"y:\deep\dir\mod.rs"),
            ("D|/path/to/file.rs", r"D:\path\to\file.rs"),
            ("d|\\path\\to\\file.rs", r"d:\path\to\file.rs"),
        ];
        for (uri, expected) in &drives {
            let res = parse_file_uri(uri).unwrap_or_else(|| panic!("Failed for {}", uri));
            assert!(!res.to_string_lossy().contains('|'));
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for {}",
                uri
            );
        }

        // 3. Percent-encoded colon (%3A and %3a)
        let percent_colons = [
            ("file:///C%3A/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file:///c%3a/path/to/file.rs", r"c:\path\to\file.rs"),
            ("file://C%3A/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file://c%3a/path/to/file.rs", r"c:\path\to\file.rs"),
        ];
        for (uri, expected) in &percent_colons {
            let res = parse_file_uri(uri).unwrap_or_else(|| panic!("Failed for {}", uri));
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for {}",
                uri
            );
        }

        // 4. URL query strings and fragments with pipes
        let query_and_frag = [
            (
                "file:///C|/folder/file.rs?query=param",
                r"C:\folder\file.rs",
            ),
            ("file:///C|/folder/file.rs#L42", r"C:\folder\file.rs"),
            ("file://C|/folder/file.rs?foo=bar#baz", r"C:\folder\file.rs"),
        ];
        for (uri, expected) in &query_and_frag {
            let res = parse_file_uri(uri).unwrap_or_else(|| panic!("Failed for {}", uri));
            assert!(!res.to_string_lossy().contains('|'));
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for {}",
                uri
            );
        }

        // 5. Encoded spaces combined with pipe
        let spaces = [
            (
                "file:///C|/Program%20Files/App/file.rs",
                r"C:\Program Files\App\file.rs",
            ),
            (
                "file://C|/Program%20Files/App/file.rs",
                r"C:\Program Files\App\file.rs",
            ),
            (
                "file:///C%7C/Program%20Files/App/file.rs",
                r"C:\Program Files\App\file.rs",
            ),
            (
                "file://C%7c/Program%20Files/App/file.rs",
                r"C:\Program Files\App\file.rs",
            ),
        ];
        for (uri, expected) in &spaces {
            let res = parse_file_uri(uri).unwrap_or_else(|| panic!("Failed for {}", uri));
            assert!(!res.to_string_lossy().contains('|'));
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for {}",
                uri
            );
        }

        // 6. Mixed forward/backward slashes with pipe
        let mixed = [
            (
                r"file:///C|\Program Files/App\file.rs",
                r"C:\Program Files\App\file.rs",
            ),
            (
                r"file://C|\Program Files/App\file.rs",
                r"C:\Program Files\App\file.rs",
            ),
            (
                r"C|\Program Files/App\file.rs",
                r"C:\Program Files\App\file.rs",
            ),
        ];
        for (uri, expected) in &mixed {
            let res = parse_file_uri(uri).unwrap_or_else(|| panic!("Failed for {}", uri));
            assert!(!res.to_string_lossy().contains('|'));
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for {}",
                uri
            );
        }
    }
}

/// Adversarial Boundary & Fuzz Test for Defect 4.1 and 4.2
#[test]
fn test_adversarial_stress_defect4_boundary_cases() {
    #[cfg(windows)]
    {
        // 1. Localhost variations with colon, pipe, encoded, and backslashes
        let localhost_cases = [
            ("file://localhost/C:", r"C:\"),
            ("file://localhost/c:", r"c:\"),
            ("file://localhost/C:/", r"C:\"),
            ("file://LOCALHOST/C:/", r"C:\"),
            ("file://localhost/C:/path/to/file.rs", r"C:\path\to\file.rs"),
            (
                r"file://localhost\C:\path\to\file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                "file:///localhost/C:/path/to/file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                "file://localhost/C%7C/path/to/file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                "file://localhost/C%3A/path/to/file.rs",
                r"C:\path\to\file.rs",
            ),
            ("file://localhost/C%7C", r"C:\"),
            ("file://localhost/C%3A", r"C:\"),
            ("file://localhost/C|?query=foo#anchor", r"C:\"),
            ("file://localhost/C:?query=foo#anchor", r"C:\"),
            ("file://localhost/C|/dir/subdir/", r"C:\dir\subdir"),
            ("file://localhost/C:/dir/subdir/", r"C:\dir\subdir"),
        ];
        for (uri, expected) in &localhost_cases {
            let panic_res = std::panic::catch_unwind(|| parse_file_uri(uri));
            assert!(panic_res.is_ok(), "URI panicked: {}", uri);
            let res = panic_res
                .unwrap()
                .unwrap_or_else(|| panic!("Returned None for {}", uri));
            assert!(
                !res.to_string_lossy().contains('|'),
                "Contains pipe for {}: {:?}",
                uri,
                res
            );
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for {}",
                uri
            );
        }

        // 2. Query and fragment edge cases on drive roots without trailing slash
        let root_query_frag_cases = [
            ("file:///C:?", r"C:\"),
            ("file:///C:#", r"C:\"),
            ("file:///C:?#", r"C:\"),
            ("file:///C|?", r"C:\"),
            ("file:///C|#", r"C:\"),
            ("file:///C|?#", r"C:\"),
            ("file://C:?", r"C:\"),
            ("file://C:#", r"C:\"),
            ("file://C|?", r"C:\"),
            ("file://C|#", r"C:\"),
            ("file:///C%7C?", r"C:\"),
            ("file:///C%7C#", r"C:\"),
            ("file:///C%3A?", r"C:\"),
            ("file:///C%3A#", r"C:\"),
            ("file:///C:?query=foo&bar=baz", r"C:\"),
            ("file:///C|?query=foo&bar=baz", r"C:\"),
            ("file:///C:#section1", r"C:\"),
            ("file:///C|#section1", r"C:\"),
            ("file:///C:?query=foo#section1", r"C:\"),
            ("file:///C|?query=foo#section1", r"C:\"),
            ("file://localhost/C:?query=foo#section1", r"C:\"),
            ("file://localhost/C|?query=foo#section1", r"C:\"),
        ];
        for (uri, expected) in &root_query_frag_cases {
            let panic_res = std::panic::catch_unwind(|| parse_file_uri(uri));
            assert!(panic_res.is_ok(), "URI panicked: {}", uri);
            let res = panic_res
                .unwrap()
                .unwrap_or_else(|| panic!("Returned None for {}", uri));
            assert!(
                !res.to_string_lossy().contains('|'),
                "Contains pipe for {}: {:?}",
                uri,
                res
            );
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for {}",
                uri
            );
        }

        // 3. Drive-relative syntax like "C:file.rs" or "file:///C:file.rs" should NOT panic
        let non_absolute_cases = [
            "file:///C:file.rs",
            "file://C:file.rs",
            "file:///C|file.rs",
            "file://C|file.rs",
            "C:file.rs",
            "C|file.rs",
        ];
        for uri in &non_absolute_cases {
            let panic_res = std::panic::catch_unwind(|| parse_file_uri(uri));
            assert!(panic_res.is_ok(), "Drive-relative URI panicked: {}", uri);
        }

        // 4. Degenerate and empty URI inputs must never panic
        let degenerate_cases = [
            "",
            "   ",
            "file:",
            "file:/",
            "file://",
            "file:///",
            "file:////",
            "file://///",
            "file://localhost",
            "file://localhost/",
            r"file://localhost\",
            "file://localhost/dir",
            "file:///dir",
            "file://127.0.0.1/share/file.rs",
            "file://server/share/file.rs",
            "///",
            ":",
            "|",
            "C",
            "C:",
            "C|",
            "C:/",
            r"C:\",
            "/",
            r"\",
        ];
        for uri in &degenerate_cases {
            let panic_res = std::panic::catch_unwind(|| parse_file_uri(uri));
            assert!(panic_res.is_ok(), "Degenerate URI panicked: '{}'", uri);
        }
    }
}

// ------------------------------------------------------------------------------------------------
// ADVERSARIAL STRESS TESTS: DEFECT 1 & DEFECT 2 (CHALLENGER M1-ITER2-1)
// ------------------------------------------------------------------------------------------------

fn setup_symbols_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE files (
            file_id TEXT PRIMARY KEY,
            path TEXT NOT NULL,
            language TEXT,
            content_hash TEXT,
            content_bytes INTEGER,
            line_count INTEGER,
            indexed_at TEXT
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY,
            file_id TEXT,
            path TEXT NOT NULL,
            language TEXT,
            name TEXT,
            kind TEXT,
            signature TEXT,
            doc_comment TEXT,
            visibility TEXT,
            parent_symbol_id TEXT,
            start_line INTEGER,
            start_column INTEGER,
            end_line INTEGER,
            end_column INTEGER,
            start_byte INTEGER,
            end_byte INTEGER,
            body_start_line INTEGER,
            body_start_column INTEGER,
            body_end_line INTEGER,
            body_end_column INTEGER,
            body_start_byte INTEGER,
            body_end_byte INTEGER,
            body_hash TEXT,
            semantic_group TEXT,
            is_test INTEGER,
            test_container INTEGER
        );
        CREATE TABLE relationships (
            relationship_id TEXT PRIMARY KEY,
            from_symbol_id TEXT,
            to_symbol_id TEXT,
            kind TEXT,
            path TEXT,
            start_line INTEGER,
            start_column INTEGER
        );
        CREATE TABLE pending_relationships (
            from_symbol_id TEXT,
            target_terminal_name TEXT,
            kind TEXT,
            path TEXT,
            start_line INTEGER,
            start_column INTEGER
        );",
    )
    .unwrap();
    conn
}

/// Adversarial Stress Test: Defect 1 External callee backslash handling across deep nesting,
/// mixed slashes, Windows drive paths, deduplication, and include_external toggle.
#[test]
fn test_adversarial_defect1_external_callee_backslash_permutations() {
    let conn = setup_symbols_test_db();
    conn.execute_batch(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/api/auth.rs', 'rust', 'authenticate_user', 'function',
            'pub fn authenticate_user()', NULL, 'pub', NULL, 1, 0, 20, 0, 0, 200,
            1, 0, 20, 0, 0, 200, 'hash1', 'function', 0, 0
        );
        -- Deep nested backslash path
        INSERT INTO pending_relationships VALUES ('s1', 'verify_token', 'call', 'src\\security\\crypto\\jwt\\verifier.rs', 45, 8);
        -- Mixed slash path
        INSERT INTO pending_relationships VALUES ('s1', 'query_db', 'call', 'src/database\\postgres\\pool.rs', 102, 12);
        -- Absolute / Windows drive path
        INSERT INTO pending_relationships VALUES ('s1', 'write_audit_log', 'call', 'C:\\logs\\audit\\writer.rs', 15, 4);
        -- Duplicate entry to test deduplication
        INSERT INTO pending_relationships VALUES ('s1', 'verify_token', 'call', 'src\\security\\crypto\\jwt\\verifier.rs', 45, 8);
        ",
    )
    .unwrap();

    // 1. With include_external = false
    let no_ext = find_callee_signatures(&conn, "authenticate_user", "s1", 10, false).unwrap();
    assert!(
        no_ext.is_empty(),
        "When include_external is false, external callees must be empty: {:?}",
        no_ext
    );

    // 2. With include_external = true
    let with_ext = find_callee_signatures(&conn, "authenticate_user", "s1", 10, true).unwrap();
    assert_eq!(
        with_ext.len(),
        3,
        "Expected 3 unique external callees, got {:?}",
        with_ext
    );

    for callee in &with_ext {
        assert!(
            !callee.contains('\\'),
            "External callee signature must strictly format paths with forward slashes: {}",
            callee
        );
    }

    assert_eq!(
        with_ext[0],
        "verify_token (src/security/crypto/jwt/verifier.rs:45)"
    );
    assert_eq!(with_ext[1], "query_db (src/database/postgres/pool.rs:102)");
    assert_eq!(with_ext[2], "write_audit_log (C:/logs/audit/writer.rs:15)");
}

/// Adversarial Stress Test: Defect 2 Single-segment suffix path filter matrix across forward and backslash DBs,
/// root files, casing permutations (lower, upper, mixed), and leading/trailing separators.
#[test]
fn test_adversarial_defect2_suffix_path_filter_single_segment_matrix() {
    let conn = setup_symbols_test_db();
    conn.execute_batch(
        "
        -- 1. Forward-slash path in DB
        INSERT INTO symbols VALUES (
            's_fwd', 'f1', 'src/controllers/user_controller.rs', 'rust', 'UserController', 'struct',
            'pub struct UserController', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h1', 'struct', 0, 0
        );
        -- 2. Backslash path in DB
        INSERT INTO symbols VALUES (
            's_bs', 'f2', 'src\\handlers\\order_handler.rs', 'rust', 'OrderHandler', 'struct',
            'pub struct OrderHandler', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h2', 'struct', 0, 0
        );
        -- 3. Root file (no directories) in DB
        INSERT INTO symbols VALUES (
            's_root', 'f3', 'main.rs', 'rust', 'RunApp', 'function',
            'pub fn RunApp()', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h3', 'function', 0, 0
        );
        ",
    )
    .unwrap();

    // Permutations for forward-slash DB symbol:
    let fwd_queries = [
        "user_controller.rs",
        "USER_CONTROLLER.RS",
        "User_Controller.Rs",
        "/user_controller.rs",
        r"\user_controller.rs",
        "user_controller.rs/",
        r"user_controller.rs\",
        r"\user_controller.rs\",
        "/user_controller.rs/",
    ];
    for q in &fwd_queries {
        let sym = get_symbol_by_name(&conn, "UserController", Some(q))
            .unwrap()
            .unwrap_or_else(|| panic!("Failed to find UserController with filter: {}", q));
        assert_eq!(sym.name, "UserController");
        assert_eq!(sym.path, "src/controllers/user_controller.rs");
    }

    // Permutations for backslash DB symbol:
    let bs_queries = [
        "order_handler.rs",
        "ORDER_HANDLER.RS",
        "Order_Handler.Rs",
        "/order_handler.rs",
        r"\order_handler.rs",
        "order_handler.rs/",
        r"order_handler.rs\",
        r"\order_handler.rs\",
        "/order_handler.rs/",
    ];
    for q in &bs_queries {
        let sym = get_symbol_by_name(&conn, "OrderHandler", Some(q))
            .unwrap()
            .unwrap_or_else(|| panic!("Failed to find OrderHandler with filter: {}", q));
        assert_eq!(sym.name, "OrderHandler");
        assert_eq!(sym.path, "src/handlers/order_handler.rs");
    }

    // Permutations for root file:
    let root_queries = [
        "main.rs",
        "MAIN.RS",
        "Main.Rs",
        "/main.rs",
        r"\main.rs",
        "main.rs/",
        r"main.rs\",
    ];
    for q in &root_queries {
        let sym = get_symbol_by_name(&conn, "RunApp", Some(q))
            .unwrap()
            .unwrap_or_else(|| panic!("Failed to find RunApp with filter: {}", q));
        assert_eq!(sym.name, "RunApp");
        assert_eq!(sym.path, "main.rs");
    }
}

/// Adversarial Stress Test: Defect 2 Multi-segment suffix path filter matrix across forward and backslash DBs,
/// multi-hop paths, mixed slashes, casing permutations, and leading/trailing separators.
#[test]
fn test_adversarial_defect2_suffix_path_filter_multi_segment_matrix() {
    let conn = setup_symbols_test_db();
    conn.execute_batch(
        "
        INSERT INTO symbols VALUES (
            's_fwd', 'f1', 'src/services/billing/tax_calculator.rs', 'rust', 'TaxCalculator', 'struct',
            'pub struct TaxCalculator', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h1', 'struct', 0, 0
        );
        INSERT INTO symbols VALUES (
            's_bs', 'f2', 'src\\services\\shipping\\rate_estimator.rs', 'rust', 'RateEstimator', 'struct',
            'pub struct RateEstimator', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h2', 'struct', 0, 0
        );
        ",
    )
    .unwrap();

    // Test 2-segment, 3-segment, and 4-segment filters in forward-slash DB
    let fwd_multi_filters = [
        // Forward slashes
        "billing/tax_calculator.rs",
        "services/billing/tax_calculator.rs",
        "src/services/billing/tax_calculator.rs",
        // Backslashes
        r"billing\tax_calculator.rs",
        r"services\billing\tax_calculator.rs",
        r"src\services\billing\tax_calculator.rs",
        // Case variations
        "BILLING/TAX_CALCULATOR.RS",
        r"Billing\Tax_Calculator.Rs",
        "Services/Billing/Tax_Calculator.Rs",
        r"SERVICES\BILLING\TAX_CALCULATOR.RS",
        r"SRC\SERVICES\BILLING\TAX_CALCULATOR.RS",
        // Leading / trailing slashes
        "/billing/tax_calculator.rs",
        r"\billing\tax_calculator.rs",
        "billing/tax_calculator.rs/",
        r"billing\tax_calculator.rs\",
        r"\billing\tax_calculator.rs\",
    ];

    for q in &fwd_multi_filters {
        let sym = get_symbol_by_name(&conn, "TaxCalculator", Some(q))
            .unwrap()
            .unwrap_or_else(|| panic!("Failed to find TaxCalculator with filter: {}", q));
        assert_eq!(sym.name, "TaxCalculator");
        assert_eq!(sym.path, "src/services/billing/tax_calculator.rs");
    }

    // Test 2-segment, 3-segment, and 4-segment filters in backslash DB
    let bs_multi_filters = [
        // Forward slashes
        "shipping/rate_estimator.rs",
        "services/shipping/rate_estimator.rs",
        "src/services/shipping/rate_estimator.rs",
        // Backslashes
        r"shipping\rate_estimator.rs",
        r"services\shipping\rate_estimator.rs",
        r"src\services\shipping\rate_estimator.rs",
        // Case variations
        "SHIPPING/RATE_ESTIMATOR.RS",
        r"Shipping\Rate_Estimator.Rs",
        "Services/Shipping/Rate_Estimator.Rs",
        r"SERVICES\SHIPPING\RATE_ESTIMATOR.RS",
        r"SRC\SERVICES\SHIPPING\RATE_ESTIMATOR.RS",
        // Leading / trailing slashes
        "/shipping/rate_estimator.rs",
        r"\shipping\rate_estimator.rs",
        "shipping/rate_estimator.rs/",
        r"shipping\rate_estimator.rs\",
        r"\shipping\rate_estimator.rs\",
    ];

    for q in &bs_multi_filters {
        let sym = get_symbol_by_name(&conn, "RateEstimator", Some(q))
            .unwrap()
            .unwrap_or_else(|| panic!("Failed to find RateEstimator with filter: {}", q));
        assert_eq!(sym.name, "RateEstimator");
        assert_eq!(sym.path, "src/services/shipping/rate_estimator.rs");
    }
}

/// Adversarial Stress Test: Defect 2 LIKE special character escaping (`_` and `%`) in path filters.
#[test]
fn test_adversarial_defect2_path_filter_special_chars_escaping() {
    let conn = setup_symbols_test_db();
    conn.execute_batch(
        "
        -- Two files differing only by an underscore vs single character
        INSERT INTO symbols VALUES (
            's_target', 'f1', 'src/models/user_account.rs', 'rust', 'TargetModel', 'struct',
            'pub struct TargetModel', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h1', 'struct', 0, 0
        );
        INSERT INTO symbols VALUES (
            's_other', 'f2', 'src/models/userXaccount.rs', 'rust', 'TargetModel', 'struct',
            'pub struct TargetModel', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h2', 'struct', 0, 0
        );
        -- Same pair in backslash format
        INSERT INTO symbols VALUES (
            's_target_bs', 'f3', 'src\\models\\admin_account.rs', 'rust', 'AdminModel', 'struct',
            'pub struct AdminModel', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h3', 'struct', 0, 0
        );
        INSERT INTO symbols VALUES (
            's_other_bs', 'f4', 'src\\models\\adminZaccount.rs', 'rust', 'AdminModel', 'struct',
            'pub struct AdminModel', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h4', 'struct', 0, 0
        );
        -- File with percent sign in directory name
        INSERT INTO symbols VALUES (
            's_pct', 'f5', 'src/special%dir/item.rs', 'rust', 'SpecialItem', 'struct',
            'pub struct SpecialItem', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h5', 'struct', 0, 0
        );
        INSERT INTO symbols VALUES (
            's_pct_bs', 'f6', 'src\\special%dir\\item_bs.rs', 'rust', 'SpecialItemBs', 'struct',
            'pub struct SpecialItemBs', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100,
            1, 0, 10, 0, 0, 100, 'h6', 'struct', 0, 0
        );
        ",
    )
    .unwrap();

    // 1. In forward-slash DB: user_account.rs must match user_account.rs and NOT match userXaccount.rs
    let sym = get_symbol_by_name(&conn, "TargetModel", Some("user_account.rs")).unwrap();
    assert!(
        sym.is_some(),
        "user_account.rs should match exactly one target"
    );
    assert_eq!(sym.unwrap().path, "src/models/user_account.rs");

    let sym_multi =
        get_symbol_by_name(&conn, "TargetModel", Some("models/user_account.rs")).unwrap();
    assert_eq!(sym_multi.unwrap().path, "src/models/user_account.rs");

    // 2. In backslash DB: admin_account.rs must match admin_account.rs and NOT match adminZaccount.rs
    let sym_bs = get_symbol_by_name(&conn, "AdminModel", Some("admin_account.rs")).unwrap();
    assert!(
        sym_bs.is_some(),
        "admin_account.rs should match exactly one target in backslash DB"
    );
    assert_eq!(sym_bs.unwrap().path, "src/models/admin_account.rs");

    let sym_bs_multi =
        get_symbol_by_name(&conn, "AdminModel", Some(r"models\admin_account.rs")).unwrap();
    assert_eq!(sym_bs_multi.unwrap().path, "src/models/admin_account.rs");

    // 3. Percent sign in path
    let sym_pct1 = get_symbol_by_name(&conn, "SpecialItem", Some("special%dir/item.rs")).unwrap();
    assert!(sym_pct1.is_some());
    assert_eq!(sym_pct1.unwrap().path, "src/special%dir/item.rs");

    let sym_pct2 =
        get_symbol_by_name(&conn, "SpecialItemBs", Some(r"special%dir\item_bs.rs")).unwrap();
    assert!(sym_pct2.is_some());
    assert_eq!(sym_pct2.unwrap().path, "src/special%dir/item_bs.rs");
}

/// Adversarial Stress Test: Defect 2 Negative non-boundary matches across filenames, directories, and extensions.
#[test]
fn test_adversarial_defect2_negative_non_boundary_matches() {
    let conn = setup_symbols_test_db();
    conn.execute_batch(
        "
        INSERT INTO symbols VALUES (
            's1', 'f1', 'src/Services/PaymentService.rs', 'rust', 'ProcessPayment', 'function',
            'pub fn ProcessPayment()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
            1, 0, 5, 0, 0, 50, 'h1', 'function', 0, 0
        );
        INSERT INTO symbols VALUES (
            's2', 'f2', 'src\\Services\\OrderService.rs', 'rust', 'ProcessOrder', 'function',
            'pub fn ProcessOrder()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
            1, 0, 5, 0, 0, 50, 'h2', 'function', 0, 0
        );
        ",
    )
    .unwrap();

    let non_boundary_negatives_payment = [
        // Substrings inside filename without slash boundary
        "Payment.rs",
        "Service.rs",
        "entService.rs",
        "ymentService.rs",
        "FooPaymentService.rs",
        "PaymentService",
        "PaymentService.rsx",
        "PaymentService.rs.bak",
        // Substrings inside directory without boundary
        "vice/PaymentService.rs",
        r"vice\PaymentService.rs",
        "vices/PaymentService.rs",
        r"vices\PaymentService.rs",
        "FooServices/PaymentService.rs",
        r"FooServices\PaymentService.rs",
        // Directory only without filename
        "Services",
        "Services/",
        r"Services\",
        "src/Services",
        r"src\Services",
        // Earlier prefix only
        "src",
        "src/",
    ];

    for neg in &non_boundary_negatives_payment {
        let sym = get_symbol_by_name(&conn, "ProcessPayment", Some(neg)).unwrap();
        assert!(
            sym.is_none(),
            "Non-boundary filter '{}' must NOT match 'src/Services/PaymentService.rs'",
            neg
        );
    }

    let non_boundary_negatives_order = [
        // Substrings inside filename without slash boundary
        "Order.rs",
        "Service.rs",
        "derService.rs",
        "FooOrderService.rs",
        "OrderService",
        "OrderService.rsx",
        // Substrings inside directory without boundary
        "vice/OrderService.rs",
        r"vice\OrderService.rs",
        "vices/OrderService.rs",
        r"vices\OrderService.rs",
        "FooServices/OrderService.rs",
        r"FooServices\OrderService.rs",
        // Directory only without filename
        "Services",
        "Services/",
        r"Services\",
    ];

    for neg in &non_boundary_negatives_order {
        let sym = get_symbol_by_name(&conn, "ProcessOrder", Some(neg)).unwrap();
        assert!(
            sym.is_none(),
            "Non-boundary filter '{}' must NOT match 'src\\Services\\OrderService.rs'",
            neg
        );
    }
}

/// Adversarial Stress Test: Defect 2 Disambiguation of identical symbol names across directories with slashes.
#[test]
fn test_adversarial_defect2_disambiguation_with_slashes() {
    let conn = setup_symbols_test_db();
    conn.execute_batch(
        "
        INSERT INTO symbols VALUES (
            's_http', 'f1', 'src\\http\\client.rs', 'rust', 'build_client', 'function',
            'pub fn build_client() -> HttpClient', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
            1, 0, 5, 0, 0, 50, 'h1', 'function', 0, 0
        );
        INSERT INTO symbols VALUES (
            's_grpc', 'f2', 'src/grpc/client.rs', 'rust', 'build_client', 'function',
            'pub fn build_client() -> GrpcClient', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
            1, 0, 5, 0, 0, 50, 'h2', 'function', 0, 0
        );
        ",
    )
    .unwrap();

    // 1. Ambiguous query with just symbol name
    let err = get_symbol_by_name(&conn, "build_client", None);
    match err {
        Err(QueryError::AmbiguousSymbol(name, count, candidates)) => {
            assert_eq!(name, "build_client");
            assert_eq!(count, 2);
            assert!(candidates.contains("src/http/client.rs"));
            assert!(candidates.contains("src/grpc/client.rs"));
            assert!(
                !candidates.contains('\\'),
                "Candidates list must use forward slashes: {}",
                candidates
            );
        }
        other => panic!("Expected AmbiguousSymbol error, got {:?}", other),
    }

    // 2. Ambiguous query with just filename "client.rs" (both have client.rs)
    let err_file = get_symbol_by_name(&conn, "build_client", Some("client.rs"));
    match err_file {
        Err(QueryError::AmbiguousSymbol(name, count, _)) => {
            assert_eq!(name, "build_client");
            assert_eq!(count, 2);
        }
        other => panic!(
            "Expected AmbiguousSymbol error for client.rs, got {:?}",
            other
        ),
    }

    // 3. Disambiguate http client using forward slash
    let sym_http1 = get_symbol_by_name(&conn, "build_client", Some("http/client.rs"))
        .unwrap()
        .unwrap();
    assert_eq!(sym_http1.symbol_id, "s_http");
    assert_eq!(sym_http1.path, "src/http/client.rs");

    // 4. Disambiguate http client using backslash
    let sym_http2 = get_symbol_by_name(&conn, "build_client", Some(r"http\client.rs"))
        .unwrap()
        .unwrap();
    assert_eq!(sym_http2.symbol_id, "s_http");
    assert_eq!(sym_http2.path, "src/http/client.rs");

    // 5. Disambiguate grpc client using forward slash
    let sym_grpc1 = get_symbol_by_name(&conn, "build_client", Some("grpc/client.rs"))
        .unwrap()
        .unwrap();
    assert_eq!(sym_grpc1.symbol_id, "s_grpc");
    assert_eq!(sym_grpc1.path, "src/grpc/client.rs");

    // 6. Disambiguate grpc client using backslash
    let sym_grpc2 = get_symbol_by_name(&conn, "build_client", Some(r"grpc\client.rs"))
        .unwrap()
        .unwrap();
    assert_eq!(sym_grpc2.symbol_id, "s_grpc");
    assert_eq!(sym_grpc2.path, "src/grpc/client.rs");

    // 7. Case variations in disambiguation
    let sym_http_case = get_symbol_by_name(&conn, "build_client", Some("HTTP/CLIENT.RS"))
        .unwrap()
        .unwrap();
    assert_eq!(sym_http_case.symbol_id, "s_http");

    let sym_grpc_case = get_symbol_by_name(&conn, "build_client", Some(r"GRPC\CLIENT.RS"))
        .unwrap()
        .unwrap();
    assert_eq!(sym_grpc_case.symbol_id, "s_grpc");
}

/// Adversarial Stress Test: Defect 2 Exact path matching (`get_symbol_by_name_exact`) matrix.
#[test]
fn test_adversarial_defect2_exact_path_matching_matrix() {
    let conn = setup_symbols_test_db();
    conn.execute_batch(
        "
        INSERT INTO symbols VALUES (
            's_fwd', 'f1', 'src/services/payment.rs', 'rust', 'Process', 'function',
            'pub fn Process()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
            1, 0, 5, 0, 0, 50, 'h1', 'function', 0, 0
        );
        INSERT INTO symbols VALUES (
            's_bs', 'f2', 'src\\services\\order.rs', 'rust', 'Process', 'function',
            'pub fn Process()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
            1, 0, 5, 0, 0, 50, 'h2', 'function', 0, 0
        );
        ",
    )
    .unwrap();

    // Exact matches must work across both slashes and casing:
    let sym1 = get_symbol_by_name_exact(&conn, "Process", "src/services/payment.rs").unwrap();
    assert!(sym1.is_some());
    assert_eq!(sym1.unwrap().path, "src/services/payment.rs");

    let sym2 = get_symbol_by_name_exact(&conn, "Process", r"src\services\payment.rs").unwrap();
    assert!(sym2.is_some());
    assert_eq!(sym2.unwrap().path, "src/services/payment.rs");

    let sym3 = get_symbol_by_name_exact(&conn, "Process", "SRC/SERVICES/PAYMENT.RS").unwrap();
    assert!(sym3.is_some());
    assert_eq!(sym3.unwrap().path, "src/services/payment.rs");

    // Exact match for backslash DB row:
    let sym4 = get_symbol_by_name_exact(&conn, "Process", "src/services/order.rs").unwrap();
    assert!(sym4.is_some());
    assert_eq!(sym4.unwrap().path, "src/services/order.rs");

    let sym5 = get_symbol_by_name_exact(&conn, "Process", r"src\services\order.rs").unwrap();
    assert!(sym5.is_some());
    assert_eq!(sym5.unwrap().path, "src/services/order.rs");

    let sym6 = get_symbol_by_name_exact(&conn, "Process", r"SRC\SERVICES\ORDER.RS").unwrap();
    assert!(sym6.is_some());
    assert_eq!(sym6.unwrap().path, "src/services/order.rs");

    // Suffixes must NOT match under get_symbol_by_name_exact:
    assert!(
        get_symbol_by_name_exact(&conn, "Process", "payment.rs")
            .unwrap()
            .is_none()
    );
    assert!(
        get_symbol_by_name_exact(&conn, "Process", "services/payment.rs")
            .unwrap()
            .is_none()
    );
    assert!(
        get_symbol_by_name_exact(&conn, "Process", r"services\payment.rs")
            .unwrap()
            .is_none()
    );
    assert!(
        get_symbol_by_name_exact(&conn, "Process", "order.rs")
            .unwrap()
            .is_none()
    );
    assert!(
        get_symbol_by_name_exact(&conn, "Process", "services/order.rs")
            .unwrap()
            .is_none()
    );
    assert!(
        get_symbol_by_name_exact(&conn, "Process", r"services\order.rs")
            .unwrap()
            .is_none()
    );
}

/// Adversarial Stress Test: Defect 4.1 & 4.2 Comprehensive Matrix
/// Tests drive roots (with/without trailing slash, ?, #, percent encodings, casing)
/// and localhost permutations (slashes, casing, paths, spaces, queries, fragments, no slashes).
#[test]
fn test_adversarial_defect4_remediation_deep_stress_matrix() {
    #[cfg(windows)]
    {
        // 1. Drive roots with and without trailing slash, casing, percent-encoding, query, fragment
        let root_matrix = [
            // Drive root no slash
            ("file:///C|", r"C:\"),
            ("file://C|", r"C:\"),
            ("file:///c|", r"c:\"),
            ("file://c|", r"c:\"),
            ("file:///C:", r"C:\"),
            ("file://C:", r"C:\"),
            ("file:///c:", r"c:\"),
            ("file://c:", r"c:\"),
            // Percent-encoded drive delimiters
            ("file:///C%7C", r"C:\"),
            ("file://C%7C", r"C:\"),
            ("file:///C%7c", r"C:\"),
            ("file://C%7c", r"C:\"),
            ("file:///c%7C", r"c:\"),
            ("file://c%7c", r"c:\"),
            ("file:///C%3A", r"C:\"),
            ("file://C%3A", r"C:\"),
            ("file:///C%3a", r"C:\"),
            ("file://C%3a", r"C:\"),
            ("file:///c%3A", r"c:\"),
            ("file://c%3a", r"c:\"),
            // Alternative drive letters
            ("file:///D|", r"D:\"),
            ("file:///d|", r"d:\"),
            ("file://D:", r"D:\"),
            ("file:///z:", r"z:\"),
            ("file://Z|", r"Z:\"),
            // Queries and fragments on roots
            ("file:///C|?foo=bar", r"C:\"),
            ("file://C|?foo=bar", r"C:\"),
            ("file:///c|?foo=bar", r"c:\"),
            ("file://c|?foo=bar", r"c:\"),
            ("file:///C:?foo=bar", r"C:\"),
            ("file://C:?foo=bar", r"C:\"),
            ("file:///c:?foo=bar", r"c:\"),
            ("file://c:?foo=bar", r"c:\"),
            ("file:///C|#frag", r"C:\"),
            ("file://C|#frag", r"C:\"),
            ("file:///c|#frag", r"c:\"),
            ("file://c|#frag", r"c:\"),
            ("file:///C:#frag", r"C:\"),
            ("file://C:#frag", r"C:\"),
            ("file:///c:#frag", r"c:\"),
            ("file://c:#frag", r"c:\"),
            ("file:///C|?foo=bar#frag", r"C:\"),
            ("file:///C:?foo=bar#frag", r"C:\"),
            ("file:///C%7C?foo=bar", r"C:\"),
            ("file:///C%7C#frag", r"C:\"),
            ("file:///C%3A?foo=bar", r"C:\"),
            ("file:///C%3A#frag", r"C:\"),
            // Four slashes
            ("file:////C|", r"C:\"),
            ("file:////C|/", r"C:\"),
            ("file:////c|", r"c:\"),
            ("file:////C:", r"C:\"),
        ];

        for (uri, expected) in &root_matrix {
            let res = std::panic::catch_unwind(|| parse_file_uri(uri));
            assert!(res.is_ok(), "URI '{}' must not panic", uri);
            let path_opt = res.unwrap();
            assert!(path_opt.is_some(), "URI '{}' must return Some", uri);
            let path = path_opt.unwrap();
            assert!(
                !path.to_string_lossy().contains('|'),
                "URI '{}' must not contain pipe in resolved path: {:?}",
                uri,
                path
            );
            assert_eq!(
                path,
                normalize_path(Path::new(expected)),
                "Mismatch for root URI '{}'",
                uri
            );
        }

        // 2. Localhost root variants (with/without slash, pipe/colon, casing, query, fragment)
        let localhost_roots = [
            ("file://localhost/C|", r"C:\"),
            ("file://localhost/C|/", r"C:\"),
            ("file://localhost/c|", r"c:\"),
            ("file://localhost/c|/", r"c:\"),
            ("file://localhost/C:", r"C:\"),
            ("file://localhost/C:/", r"C:\"),
            ("file://localhost/c:", r"c:\"),
            ("file://localhost/c:/", r"c:\"),
            ("file://LOCALHOST/C|", r"C:\"),
            ("file://LOCALHOST/C|/", r"C:\"),
            ("file://LOCALHOST/c|", r"c:\"),
            ("file://LOCALHOST/c|/", r"c:\"),
            ("file://LOCALHOST/C:", r"C:\"),
            ("file://LOCALHOST/C:/", r"C:\"),
            ("file://LOCALHOST/c:", r"c:\"),
            ("file://LOCALHOST/c:/", r"c:\"),
            ("file://LocalHost/C|", r"C:\"),
            ("file://LocalHost/c|", r"c:\"),
            ("file://LocalHost/C:", r"C:\"),
            ("file://LocalHost/c:", r"c:\"),
            ("file://localhost/C%7C", r"C:\"),
            ("file://localhost/c%7c", r"c:\"),
            ("file://localhost/C%3A", r"C:\"),
            ("file://localhost/c%3a", r"c:\"),
            ("file://localhost/C|?foo=bar", r"C:\"),
            ("file://localhost/C|#frag", r"C:\"),
            ("file://localhost/C:?foo=bar", r"C:\"),
            ("file://localhost/C:#frag", r"C:\"),
            ("file://localhost/c|?foo=bar", r"c:\"),
            ("file://localhost/c|#frag", r"c:\"),
            ("file://localhost/c:?foo=bar", r"c:\"),
            ("file://localhost/c:#frag", r"c:\"),
            // Backslash localhost variants
            (r"file://localhost\C|", r"C:\"),
            (r"file://localhost\c|", r"c:\"),
            (r"file://localhost\C:", r"C:\"),
            (r"file://localhost\c:", r"c:\"),
            (r"file://localhost\C|\", r"C:\"),
            (r"file://localhost\c|\", r"c:\"),
            (r"file://localhost\C:\", r"C:\"),
            (r"file://localhost\c:\", r"c:\"),
            (r"file://LOCALHOST\C|", r"C:\"),
            (r"file://LOCALHOST\C|\", r"C:\"),
            // Three-slash localhost roots
            ("file:///localhost/C|", r"C:\"),
            ("file:///localhost/C|/", r"C:\"),
            ("file:///localhost/c|", r"c:\"),
            ("file:///localhost/c|/", r"c:\"),
            ("file:///localhost/C:", r"C:\"),
            ("file:///localhost/C:/", r"C:\"),
            ("file:///localhost/c:", r"c:\"),
            ("file:///localhost/c:/", r"c:\"),
        ];

        for (uri, expected) in &localhost_roots {
            let res = std::panic::catch_unwind(|| parse_file_uri(uri));
            assert!(res.is_ok(), "Localhost root URI '{}' must not panic", uri);
            let path_opt = res.unwrap();
            assert!(
                path_opt.is_some(),
                "Localhost root URI '{}' must return Some",
                uri
            );
            let path = path_opt.unwrap();
            assert!(
                !path.to_string_lossy().contains('|'),
                "Localhost root URI '{}' must not contain pipe: {:?}",
                uri,
                path
            );
            assert_eq!(
                path,
                normalize_path(Path::new(expected)),
                "Mismatch for localhost root URI '{}'",
                uri
            );
        }

        // 3. Localhost path variants (casing, slashes, spaces, encoded, subdirs)
        let localhost_paths = [
            ("file://localhost/C|/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file://localhost/c|/path/to/file.rs", r"c:\path\to\file.rs"),
            ("file://localhost/C:/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file://localhost/c:/path/to/file.rs", r"c:\path\to\file.rs"),
            ("file://LOCALHOST/C|/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file://LOCALHOST/c|/path/to/file.rs", r"c:\path\to\file.rs"),
            ("file://LOCALHOST/C:/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file://LOCALHOST/c:/path/to/file.rs", r"c:\path\to\file.rs"),
            ("file://LocalHost/C|/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file://LocalHost/c|/path/to/file.rs", r"c:\path\to\file.rs"),
            ("file://LocalHost/C:/path/to/file.rs", r"C:\path\to\file.rs"),
            ("file://LocalHost/c:/path/to/file.rs", r"c:\path\to\file.rs"),
            (
                "file://localhost/C|/folder/sub/code.rs?rev=1#L10",
                r"C:\folder\sub\code.rs",
            ),
            (
                "file://localhost/C|/Program Files/App/file.rs",
                r"C:\Program Files\App\file.rs",
            ),
            (
                "file://localhost/C|/Program%20Files/App/file.rs",
                r"C:\Program Files\App\file.rs",
            ),
            (
                "file://localhost/C:/Program%20Files/App/file.rs",
                r"C:\Program Files\App\file.rs",
            ),
            (
                r"file://localhost\C|\path\to\file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                r"file://localhost\c|\path\to\file.rs",
                r"c:\path\to\file.rs",
            ),
            (
                r"file://localhost\C:\path\to\file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                r"file://localhost\c:\path\to\file.rs",
                r"c:\path\to\file.rs",
            ),
            (
                r"file://LOCALHOST\C|\path\to\file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                r"file://LOCALHOST\C:\path\to\file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                "file:///localhost/C|/path/to/file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                "file:///localhost/c|/path/to/file.rs",
                r"c:\path\to\file.rs",
            ),
            (
                "file:///localhost/C:/path/to/file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                "file:///localhost/c:/path/to/file.rs",
                r"c:\path\to\file.rs",
            ),
            (
                "file:///LOCALHOST/C|/path/to/file.rs",
                r"C:\path\to\file.rs",
            ),
            (
                "file:///LOCALHOST/c|/path/to/file.rs",
                r"c:\path\to\file.rs",
            ),
            ("file:////C|/path/to/file.rs", r"C:\path\to\file.rs"),
        ];

        for (uri, expected) in &localhost_paths {
            let res = std::panic::catch_unwind(|| parse_file_uri(uri));
            assert!(res.is_ok(), "Localhost path URI '{}' must not panic", uri);
            let path_opt = res.unwrap();
            assert!(
                path_opt.is_some(),
                "Localhost path URI '{}' must return Some",
                uri
            );
            let path = path_opt.unwrap();
            assert!(
                !path.to_string_lossy().contains('|'),
                "Localhost path URI '{}' must not contain pipe: {:?}",
                uri,
                path
            );
            assert_eq!(
                path,
                normalize_path(Path::new(expected)),
                "Mismatch for localhost path URI '{}'",
                uri
            );
        }

        // 4. Plain strings (non-file://) with pipes and colons
        let plain_strings = [
            ("C|", r"C:\"),
            ("c|", r"c:\"),
            ("C:", r"C:\"),
            ("c:", r"c:\"),
            ("/C|", r"C:\"),
            ("/c|", r"c:\"),
            ("C|/path/to/file.rs", r"C:\path\to\file.rs"),
            ("c|/path/to/file.rs", r"c:\path\to\file.rs"),
            (r"C|\path\to\file.rs", r"C:\path\to\file.rs"),
            (r"c|\path\to\file.rs", r"c:\path\to\file.rs"),
            ("/C|/path/to/file.rs", r"C:\path\to\file.rs"),
        ];

        for (plain, expected) in &plain_strings {
            let res = parse_file_uri(plain)
                .unwrap_or_else(|| panic!("Failed for plain string: {}", plain));
            assert!(
                !res.to_string_lossy().contains('|'),
                "Result for '{}' contains pipe: {:?}",
                plain,
                res
            );
            assert_eq!(
                res,
                normalize_path(Path::new(expected)),
                "Mismatch for plain string '{}'",
                plain
            );
        }

        // 5. Robustness / No-Panic checks on extreme boundary inputs
        let boundary_cases = [
            "",
            "file://",
            "file:///",
            "file://localhost",
            "file://localhost/",
            "file:///|",
            "file:///123|",
            "file:///:::",
            "file:///C:foo",
            "file:///C|foo",
            "file:///Ä|/",
        ];

        for input in &boundary_cases {
            let panic_res = std::panic::catch_unwind(|| {
                let _ = parse_file_uri(input);
            });
            assert!(
                panic_res.is_ok(),
                "Extreme input '{}' must not panic",
                input
            );
        }
    }
}
