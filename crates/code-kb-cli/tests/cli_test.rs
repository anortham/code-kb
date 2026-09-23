use std::process::Command;

fn setup_test_repo() -> tempfile::TempDir {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let db_dir = root.join(".code-kb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("artifact.db");

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let content = "pub struct Workspace {\n    pub root: String,\n}\n\npub fn run_task() {\n    helper();\n}\n\nfn helper() {}\n";
    std::fs::write(src_dir.join("workspace.rs"), content).unwrap();
    let bytes = content.len() as i64;
    let hash = format!("blake3:{}", blake3::hash(content.as_bytes()).to_hex());

    let conn = code_kb_core::open_read_write(&db_path).unwrap();
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
            relationship_id TEXT PRIMARY KEY, from_symbol_id TEXT, to_symbol_id TEXT,
            kind TEXT, path TEXT, start_line INTEGER, start_column INTEGER
        );
        CREATE TABLE pending_relationships (
            from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT,
            path TEXT, start_line INTEGER, start_column INTEGER
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
        );
        CREATE TABLE type_facts (
            type_fact_id TEXT PRIMARY KEY, symbol_id TEXT NOT NULL, language TEXT,
            resolved_type TEXT NOT NULL, generic_params_json TEXT
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/workspace.rs', 'rust', ?1, ?2, 9, '2026-01-01')",
        rusqlite::params![hash, bytes],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/workspace.rs', 'rust', 'Workspace', 'struct',
            'pub struct Workspace', 'Workspace representation for discovery', 'pub', NULL,
            1, 0, 3, 1, 0, 48, 1, 21, 3, 1, 21, 48, 'b3:hash1',
            NULL, 0, 0
        )",
        rusqlite::params![],
    )
    .unwrap();
    let s2_start = content.find("pub fn run_task()").unwrap();
    let s2_end = content.find("\n\nfn helper").unwrap();
    let s2_body_start = content.find("{\n    helper();\n}").unwrap();
    let s2_body_end = s2_body_start + "{\n    helper();\n}".len();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's2', 'f1', 'src/workspace.rs', 'rust', 'run_task', 'function',
            'pub fn run_task()', 'Executes task workflow', 'pub', NULL,
            5, 0, 7, 1, ?1, ?2, 5, 18, 7, 1, ?3, ?4, 'b3:hash2',
            NULL, 0, 0
        )",
        rusqlite::params![
            s2_start as i64,
            s2_end as i64,
            s2_body_start as i64,
            s2_body_end as i64
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's3', 'f1', 'src/workspace.rs', 'rust', 'helper', 'function',
            'fn helper()', 'Internal helper function', '', NULL,
            9, 0, 9, 14, 85, 99, 9, 12, 9, 14, 97, 99, 'b3:hash3',
            NULL, 0, 0
        )",
        rusqlite::params![],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relationships VALUES ('r1', 's2', 's3', 'calls', 'src/workspace.rs', 6, 4)",
        rusqlite::params![],
    )
    .unwrap();
    code_kb_core::db::ensure_fts_index(&conn).unwrap();
    drop(conn);

    temp_dir
}

#[test]
fn test_cli_outline() {
    let repo = setup_test_repo();
    let root = repo.path();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("outline")
        .output()
        .expect("Failed to execute outline");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("workspace.rs"));

    let json_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("outline")
        .output()
        .expect("Failed to execute outline --json");

    assert!(json_output.status.success());
    let json_val: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert!(json_val.is_array());
}

#[test]
fn test_cli_skeleton() {
    let repo = setup_test_repo();
    let root = repo.path();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("skeleton")
        .arg("src/workspace.rs")
        .output()
        .expect("Failed to execute skeleton");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pub struct Workspace"));
    assert!(stdout.contains("pub fn run_task()"));
}

#[test]
fn test_cli_json_skeleton_propagates_freshness_failure() {
    let repo = setup_test_repo();
    let root = repo.path();
    std::fs::write(
        root.join("src/workspace.rs"),
        "pub fn changed_after_index() {}\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("JULIE_EXTRACT_BIN", root)
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("skeleton")
        .arg("src/workspace.rs")
        .output()
        .expect("Failed to execute JSON skeleton");

    assert!(
        !output.status.success(),
        "JSON skeleton must fail when freshness cannot be restored"
    );
    assert!(
        output.stdout.is_empty(),
        "JSON skeleton must not return stale catalog data"
    );
}

#[test]
fn test_cli_symbol_and_search() {
    let repo = setup_test_repo();
    let root = repo.path();

    // Test primary subcommand "lookup"
    let lookup_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("lookup")
        .arg("Workspace")
        .output()
        .expect("Failed to execute lookup");

    assert!(lookup_output.status.success());
    let lookup_stdout = String::from_utf8_lossy(&lookup_output.stdout);
    assert!(lookup_stdout.contains("pub struct Workspace"));
    assert!(!lookup_stdout.contains("Found 0 symbols"));

    // Test alias "symbol"
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("symbol")
        .arg("Workspace")
        .output()
        .expect("Failed to execute symbol");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pub struct Workspace"));
    assert!(!stdout.contains("Found 0 symbols"));

    let json_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("lookup")
        .arg("Workspace")
        .output()
        .expect("Failed to execute lookup --json");

    assert!(json_output.status.success());
    let json_val: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert!(json_val.is_array());
    assert_eq!(json_val.as_array().unwrap().len(), 1);
    assert_eq!(json_val[0]["name"], "Workspace");

    let search_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("search")
        .arg("discovery")
        .output()
        .expect("Failed to execute search");

    assert!(search_output.status.success());
    let search_stdout = String::from_utf8_lossy(&search_output.stdout);
    assert!(search_stdout.contains("pub struct Workspace"));
    assert!(!search_stdout.contains("Found 0 symbols"));
}

fn mcp_search_names(root: &std::path::Path, query: &str) -> Vec<String> {
    use std::io::{BufRead, Write};
    let mut child = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("serve")
        .arg("--root")
        .arg(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn code-kb serve");
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let requests = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                       "clientInfo": {"name": "parity", "version": "1.0"}}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "search_symbols", "arguments": {"query": query}}}),
    ];
    let mut text = String::new();
    for request in requests {
        stdin.write_all(format!("{request}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        if response["id"] == 2 {
            text = response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string();
        }
    }
    drop(stdin);
    let _ = child.wait();
    text.lines()
        .filter(|line| line.starts_with("- "))
        .map(|line| line.split('`').nth(1).unwrap().to_string())
        .collect()
}

#[test]
fn test_cli_search_order_matches_mcp_search_symbols() {
    let repo = setup_test_repo();
    let root = repo.path();
    let query = "workspace task helper";

    let cli_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("search")
        .arg(query)
        .output()
        .expect("Failed to execute search --json");
    assert!(cli_output.status.success());
    let cli_rows: serde_json::Value = serde_json::from_slice(&cli_output.stdout).unwrap();
    let cli_names: Vec<String> = cli_rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["symbol"]["name"].as_str().unwrap().to_string())
        .collect();

    assert_eq!(cli_names.len(), 3);
    assert_eq!(cli_names, mcp_search_names(root, query));
    assert!(
        cli_rows
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("explain").is_none())
    );
}

#[test]
fn test_cli_search_explain_flag() {
    let repo = setup_test_repo();
    let root = repo.path();

    let text = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("search")
        .arg("discovery")
        .arg("--explain")
        .output()
        .expect("Failed to execute search --explain");
    assert!(text.status.success());
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("rerank: 1 candidates in "));
    assert!(stdout.contains(" µs; words discovery "));
    assert!(stdout.contains("  explain: score "));
    assert!(stdout.contains("[word] bm25 "));

    let json = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("search")
        .arg("discovery")
        .arg("--explain")
        .output()
        .expect("Failed to execute search --explain --json");
    assert!(json.status.success());
    let rows: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let explain = &rows[0]["explain"];
    assert_eq!(explain["branches"], serde_json::json!(["word"]));
    assert_eq!(explain["candidates"], 1);
    assert!(explain["bm25"].as_f64().unwrap() < 0.0);
    assert_eq!(explain["terms"][0][1], "doc");
    assert!(explain["term_score"].as_f64().unwrap() > 0.0);
    assert_eq!(explain["word_weights"][0][0], "discovery");
    assert!(rows[0]["score"].as_f64().unwrap() > 0.0);
}

#[test]
fn test_cli_body_and_slice() {
    let repo = setup_test_repo();
    let root = repo.path();

    let body_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("body")
        .arg("run_task")
        .output()
        .expect("Failed to execute body");

    assert!(body_output.status.success());
    let stdout = String::from_utf8_lossy(&body_output.stdout);
    assert!(stdout.contains("helper();"));

    let lookup = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .args([
            "--root",
            root.to_str().unwrap(),
            "--json",
            "lookup",
            "run_task",
        ])
        .output()
        .unwrap();
    let id = serde_json::from_slice::<serde_json::Value>(&lookup.stdout).unwrap()[0]["symbol_id"]
        .as_str()
        .unwrap()
        .to_string();
    let selected_body = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .args(["--root", root.to_str().unwrap(), "body", "--symbol-id", &id])
        .output()
        .unwrap();
    assert!(selected_body.status.success());
    assert!(String::from_utf8_lossy(&selected_body.stdout).contains("helper();"));
    for command in [
        vec!["context", "--symbol-id", &id],
        vec!["refs", "--symbol-id", &id, "--direction", "callees"],
        vec!["blast-radius", "--symbol-id", &id],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .args(command)
            .output()
            .unwrap();
        assert!(output.status.success());
    }

    let conflict = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .args([
            "--root",
            root.to_str().unwrap(),
            "body",
            "run_task",
            "--symbol-id",
            &id,
        ])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("exactly one"));
    let empty_id = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .args(["--root", root.to_str().unwrap(), "body", "--symbol-id", ""])
        .output()
        .unwrap();
    assert!(!empty_id.status.success());
    assert!(String::from_utf8_lossy(&empty_id.stderr).contains("must not be empty"));

    // Test primary subcommand "context"
    let context_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("context")
        .arg("run_task")
        .output()
        .expect("Failed to execute context");

    assert!(context_output.status.success());
    let context_stdout = String::from_utf8_lossy(&context_output.stdout);
    assert!(context_stdout.contains("Target: `run_task`"));
    assert!(context_stdout.contains("helper"));

    // Test alias "slice"
    let slice_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("slice")
        .arg("run_task")
        .output()
        .expect("Failed to execute slice");

    assert!(slice_output.status.success());
    let slice_stdout = String::from_utf8_lossy(&slice_output.stdout);
    assert!(slice_stdout.contains("Target: `run_task`"));
    assert!(slice_stdout.contains("helper"));
}

#[test]
fn cli_read_tools_reject_missing_empty_and_conflicting_selectors() {
    let repo = setup_test_repo();
    let root = repo.path();
    let cases: [(&str, &[&str], &str); 12] = [
        ("body", &[], "exactly one non-empty"),
        ("body", &[""], "must not be empty"),
        ("body", &["--symbol-id", ""], "must not be empty"),
        (
            "body",
            &["run_task", "--symbol-id", "opaque"],
            "exactly one of",
        ),
        ("context", &[], "exactly one non-empty"),
        ("context", &[""], "must not be empty"),
        ("context", &["--symbol-id", ""], "must not be empty"),
        (
            "context",
            &["run_task", "--symbol-id", "opaque"],
            "exactly one of",
        ),
        ("refs", &[], "exactly one non-empty"),
        ("refs", &[""], "must not be empty"),
        ("refs", &["--symbol-id", ""], "must not be empty"),
        (
            "refs",
            &["run_task", "--symbol-id", "opaque"],
            "exactly one of",
        ),
    ];

    for (tool, selector_args, error_text) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg(tool)
            .args(selector_args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{tool} {selector_args:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(error_text),
            "{tool} {selector_args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn cli_blast_radius_rejects_empty_ids_and_keeps_git_discovery() {
    let repo = setup_test_repo();
    let root = repo.path();
    std::fs::write(root.join(".code-kb/.gitignore"), "*\n").unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args([
                "-c",
                "user.name=code-kb test",
                "-c",
                "user.email=code-kb@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "fixture",
            ])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    let source = root.join("src/workspace.rs");
    std::fs::write(&source, "pub fn changed() {}\n").unwrap();

    let discovery = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("blast-radius")
        .output()
        .unwrap();
    assert!(
        discovery.status.success(),
        "{}",
        String::from_utf8_lossy(&discovery.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&discovery.stdout).unwrap();
    assert_eq!(result["seed_type"], "file");
    assert_eq!(result["seeds"], serde_json::json!(["src/workspace.rs"]));

    for (args, error_text) in [
        (vec!["blast-radius", "--symbol-id", ""], "must not be empty"),
        (
            vec!["blast-radius", "run_task", "--symbol-id", "opaque"],
            "only one",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .args(&args)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(error_text));
    }
}

#[test]
fn symbol_ids_select_same_parent_cpp_overloads_across_read_tools() {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("overloads.cpp"),
        "struct Parent { int run() { return 1; } int run(int) { return 2; } };\nint first() { Parent p; return p.run(); }\nint second() { Parent p; return p.run(2); }\n",
    )
    .unwrap();
    std::fs::write(src.join("other.cpp"), "int other() { return 0; }\n").unwrap();
    let bin = env!("CARGO_BIN_EXE_code-kb");
    assert!(
        Command::new(bin)
            .args(["--root", root.to_str().unwrap(), "scan", "--force"])
            .status()
            .unwrap()
            .success()
    );
    let lookup = Command::new(bin)
        .args([
            "--root",
            root.to_str().unwrap(),
            "--json",
            "lookup",
            "run",
            "--include-tests",
        ])
        .output()
        .unwrap();
    assert!(lookup.status.success());
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&lookup.stdout).unwrap();
    let ids: Vec<String> = rows
        .iter()
        .filter(|row| row["name"] == "run")
        .map(|row| row["symbol_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 2, "{rows:?}");
    let conn = code_kb_core::open_read_write(&root.join(".code-kb/artifact.db")).unwrap();
    conn.execute_batch(
        "PRAGMA foreign_keys = OFF; DELETE FROM pending_relationships; DELETE FROM identifiers;",
    )
    .unwrap();
    let first: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'first'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let second: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'second'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    conn.execute("INSERT INTO relationships (reference_site_id, file_id, from_symbol_id, to_symbol_id, kind, path, start_line, start_column, confidence) SELECT ?1, file_id, ?2, ?3, 'calls', 'src/overloads.cpp', 2, 0, 1.0 FROM symbols WHERE symbol_id = ?2", rusqlite::params!["resolved-first", first, ids[0]]).unwrap();
    conn.execute("INSERT INTO relationships (reference_site_id, file_id, from_symbol_id, to_symbol_id, kind, path, start_line, start_column, confidence) SELECT ?1, file_id, ?2, ?3, 'calls', 'src/overloads.cpp', 3, 0, 1.0 FROM symbols WHERE symbol_id = ?2", rusqlite::params!["resolved-second", second, ids[1]]).unwrap();
    drop(conn);
    let bodies: Vec<String> = ids
        .iter()
        .map(|id| {
            let output = Command::new(bin)
                .args(["--root", root.to_str().unwrap(), "body", "--symbol-id", id])
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap()
        })
        .collect();
    assert_ne!(bodies[0], bodies[1]);
    let outputs: Vec<Vec<String>> = ["context", "refs", "blast-radius"]
        .iter()
        .map(|command| {
            ids.iter()
                .map(|id| {
                    let output = Command::new(bin)
                        .arg("--root")
                        .arg(root)
                        .args([*command, "--symbol-id", id])
                        .output()
                        .unwrap();
                    assert!(
                        output.status.success(),
                        "{}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                    String::from_utf8(output.stdout).unwrap()
                })
                .collect()
        })
        .collect();
    assert_ne!(outputs[0][0], outputs[0][1]);
    assert!(outputs[1][0].contains("`first`") && outputs[1][0].contains("kind: calls"));
    assert!(!outputs[1][0].contains("second"));
    assert!(outputs[1][1].contains("`second`") && outputs[1][1].contains("kind: calls"));
    assert!(!outputs[1][1].contains("first"));
    assert!(outputs[2][0].contains("first"));
    assert!(!outputs[2][0].contains("second"));
    assert!(outputs[2][1].contains("second"));
    assert!(!outputs[2][1].contains("first"));

    let matching_file = src.join("overloads.cpp");
    let guarded = Command::new(bin)
        .args(["--root", root.to_str().unwrap(), "--json", "blast-radius"])
        .args(["--symbol-id", &ids[0], "--file"])
        .arg(&matching_file)
        .output()
        .unwrap();
    assert!(guarded.status.success());
    let guarded: serde_json::Value = serde_json::from_slice(&guarded.stdout).unwrap();
    assert_eq!(guarded["seed_type"], "symbol");
    assert_eq!(guarded["seeds"], serde_json::json!([ids[0]]));

    let mismatched = Command::new(bin)
        .args(["--root", root.to_str().unwrap(), "blast-radius"])
        .args(["--symbol-id", &ids[0], "--file"])
        .arg(src.join("other.cpp"))
        .output()
        .unwrap();
    assert!(!mismatched.status.success());
    assert!(String::from_utf8_lossy(&mismatched.stderr).contains("not requested file"));
}

#[test]
fn test_cli_refs() {
    let repo = setup_test_repo();
    let root = repo.path();

    let callers_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("refs")
        .arg("helper")
        .arg("--direction")
        .arg("callers")
        .output()
        .expect("Failed to execute refs callers");

    assert!(callers_output.status.success());
    let stdout = String::from_utf8_lossy(&callers_output.stdout);
    assert!(stdout.contains("run_task"));

    let callees_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("refs")
        .arg("run_task")
        .arg("--direction")
        .arg("callees")
        .output()
        .expect("Failed to execute refs callees");

    assert!(callees_output.status.success());
    let callees_stdout = String::from_utf8_lossy(&callees_output.stdout);
    assert!(callees_stdout.contains("helper"));
}

#[test]
fn test_cli_zero_limit_does_not_claim_no_matches() {
    let repo = setup_test_repo();
    let root = repo.path();

    for args in [
        ["lookup", "Workspace", "--limit", "0"],
        ["search", "discovery", "--limit", "0"],
        ["refs", "helper", "--limit", "0"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("limit is 0"), "{args:?}: {text}");
        assert!(!text.contains("No symbols found"), "{args:?}: {text}");
        assert!(!text.contains("(none)"), "{args:?}: {text}");
    }
}

#[test]
fn test_cli_refs_with_file_filter() {
    let repo = setup_test_repo();
    let root = repo.path();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("refs")
        .arg("helper")
        .arg("--file")
        .arg("src/workspace.rs")
        .output()
        .expect("Failed to execute refs with file filter");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("run_task"));

    let short_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("refs")
        .arg("helper")
        .arg("-f")
        .arg("src/workspace.rs")
        .output()
        .expect("Failed to execute refs with -f");

    assert!(short_output.status.success());
    let short_stdout = String::from_utf8_lossy(&short_output.stdout);
    assert!(short_stdout.contains("run_task"));
}

#[test]
fn test_cli_blast_radius_and_impact() {
    let repo = setup_test_repo();
    let root = repo.path();

    let br_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("blast-radius")
        .arg("helper")
        .output()
        .expect("Failed to execute blast-radius");

    assert!(br_output.status.success());
    let stdout = String::from_utf8_lossy(&br_output.stdout);
    assert!(stdout.contains("run_task"));

    let impact_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("impact")
        .arg("helper")
        .output()
        .expect("Failed to execute impact");

    assert!(impact_output.status.success());
    let impact_stdout = String::from_utf8_lossy(&impact_output.stdout);
    assert!(impact_stdout.contains("run_task"));
}

#[test]
fn test_cli_blast_radius_zero_limit_reports_metadata() {
    let repo = setup_test_repo();
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(repo.path())
        .args(["--json", "blast-radius", "helper", "--limit", "0"])
        .output()
        .expect("Failed to execute blast-radius --json");

    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["likely_tests"], serde_json::json!([]));
    assert_eq!(result["impacted_symbols"], serde_json::json!([]));
    assert_eq!(result["likely_tests_truncated"], false);
    assert_eq!(result["impacted_symbols_truncated"], true);
    assert!(result["test_file_ceiling_reached"].is_boolean());
}

#[test]
fn test_cli_stats_and_telemetry() {
    let repo = setup_test_repo();
    let root = repo.path();

    let stats_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .arg("stats")
        .output()
        .expect("Failed to execute stats");

    assert!(stats_output.status.success());
    let stdout = String::from_utf8_lossy(&stats_output.stdout);
    assert!(stdout.contains("Telemetry Summary"));

    let json_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("telemetry")
        .output()
        .expect("Failed to execute telemetry --json");

    assert!(json_output.status.success());
    let json_val: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert!(json_val.get("total_calls").is_some());
}

#[test]
fn test_cli_stats_since_flag() {
    let repo = setup_test_repo();
    let root = repo.path();

    let stats_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .arg("stats")
        .arg("--since")
        .arg("30d")
        .output()
        .expect("Failed to execute stats --since 30d");

    assert!(stats_output.status.success());
    let stdout = String::from_utf8_lossy(&stats_output.stdout);
    assert!(stdout.contains("Telemetry Summary"));
    assert!(stdout.contains("Window: 30d"));

    // Test invalid --since value fails
    let invalid_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .arg("stats")
        .arg("--since")
        .arg("invalid_window_xyz")
        .output()
        .expect("Failed to execute stats with invalid window");

    assert!(!invalid_output.status.success());
    let stderr = String::from_utf8_lossy(&invalid_output.stderr);
    assert!(stderr.contains("Invalid time window 'invalid_window_xyz'"));
}

#[test]
fn test_cli_stats_workspace_json() {
    let repo = setup_test_repo();
    let root = repo.path();

    let json_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .arg("stats")
        .arg("--workspace")
        .arg("--json")
        .output()
        .expect("Failed to execute stats --workspace --json");

    assert!(json_output.status.success());
    let json_val: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert!(json_val.get("total_calls").is_some());
    let scope = json_val
        .get("scope_description")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(scope.contains("Workspace:"));
}

#[test]
fn test_cli_bug_report() {
    let repo = setup_test_repo();
    let root = repo.path();

    let report_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .arg("bug-report")
        .arg("--title")
        .arg("test bug")
        .output()
        .expect("Failed to execute bug-report");

    assert!(report_output.status.success());
    let stdout = String::from_utf8_lossy(&report_output.stdout);
    assert!(stdout.contains("Environment"));
    assert!(stdout.contains("https://github.com/anortham/code-kb/issues/new"));
    assert!(stdout.contains("test+bug") || stdout.contains("test%20bug"));
}

#[test]
fn test_cli_bug_report_ignores_unindexed_repo_and_preserves_existing_ignore() {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root)
            .status()
            .unwrap()
            .success()
    );
    let telemetry = root.join(".telemetry_test");

    let run_bug_report = || {
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", &telemetry)
            .arg("--root")
            .arg(root)
            .arg("bug-report")
            .arg("--json")
            .output()
            .expect("Failed to execute bug-report")
    };

    let report = run_bug_report();
    assert!(report.status.success());
    let gitignore = root.join(".code-kb").join(".gitignore");
    assert_eq!(std::fs::read_to_string(&gitignore).unwrap(), "*\n");
    assert!(root.join(".code-kb").join("logs").is_dir());
    assert!(!root.join(".code-kb").join("artifact.db").exists());
    let status = Command::new("git")
        .args(["status", "--short"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(status.status.success());
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(!status.contains(".code-kb"), "{status}");

    std::fs::write(&gitignore, "custom ignore rules\n").unwrap();
    let report = run_bug_report();
    assert!(report.status.success());
    assert_eq!(
        std::fs::read_to_string(gitignore).unwrap(),
        "custom ignore rules\n"
    );
    assert!(!root.join(".code-kb").join("artifact.db").exists());
}

#[test]
fn test_cli_bug_report_json() {
    let repo = setup_test_repo();
    let root = repo.path();

    let json_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .arg("bug-report")
        .arg("--json")
        .output()
        .expect("Failed to execute bug-report --json");

    assert!(json_output.status.success());
    let json_val: serde_json::Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert!(json_val.get("github_issue_url").is_some());
    assert!(json_val.get("markdown_body").is_some());
    assert!(json_val.get("os_info").is_some());
}

#[test]
fn test_cli_stats_scopes_errors_to_active_workspace() {
    let telem_dir = code_kb_core::safe_tempdir();

    // 1. Pre-seed global telemetry DB with error from unrelated workspace B
    let global_db = telem_dir.path().join("telemetry.db");
    let conn = rusqlite::Connection::open(&global_db).unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE tool_telemetry (
            id TEXT PRIMARY KEY,
            timestamp TEXT NOT NULL,
            workspace_root TEXT,
            workspace_name TEXT,
            tool TEXT NOT NULL,
            duration_ms INTEGER NOT NULL,
            outcome TEXT NOT NULL,
            error_message TEXT,
            result_count INTEGER NOT NULL,
            bytes_returned INTEGER NOT NULL,
            est_tokens INTEGER NOT NULL,
            est_tokens_saved INTEGER NOT NULL DEFAULT 0,
            code_kb_version TEXT NOT NULL
        );
        INSERT INTO tool_telemetry (
            id, timestamp, workspace_root, workspace_name, tool,
            duration_ms, outcome, error_message, result_count,
            bytes_returned, est_tokens, est_tokens_saved, code_kb_version
        ) VALUES (
            'err-unrelated', datetime('now'), '/other/private/workspace', 'private-repo',
            'get_symbol_body', 10, 'error', 'SECRET_LEAK_IN_OTHER_REPO', 0, 0, 0, 0, '{}'
        );",
        env!("CARGO_PKG_VERSION")
    ))
    .unwrap();
    drop(conn);

    let repo = setup_test_repo();
    let root = repo.path();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", telem_dir.path())
        .arg("--root")
        .arg(root)
        .arg("stats")
        .output()
        .expect("Failed to execute global stats");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("SECRET_LEAK_IN_OTHER_REPO"),
        "Global stats must not leak other workspace errors"
    );
}

#[test]
fn test_cli_facts() {
    let repo = setup_test_repo();
    let root = repo.path();

    let facts_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("facts")
        .output()
        .expect("Failed to execute facts");

    assert!(facts_output.status.success());
    let stdout = String::from_utf8_lossy(&facts_output.stdout);
    assert!(stdout.contains("No structural facts") || stdout.contains("Available"));
}

#[test]
fn test_cli_facts_with_config_alias_and_path_filter() {
    let repo = setup_test_repo();
    let root = repo.path();

    let db_path = root.join(".code-kb/artifact.db");
    let conn = code_kb_core::open_read_write(&db_path).unwrap();
    conn.execute(
        "INSERT INTO structural_facts VALUES ('sf1', 'f1', 'Cargo.toml', 'toml', 'toml.key_value.v1', 'package.name', 'table', NULL, 1, 2, 1.0, NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO structural_facts VALUES ('sf2', 'f1', 'src/workspace.rs', 'rust', 'toml.key_value.v1', 'dependencies', 'table', NULL, 1, 2, 1.0, NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO literals VALUES ('lit1', 'f1', 'Cargo.toml', 'toml', 'toml', '\"code-kb\"', 'carrier', NULL, 1, 0, 1, 9, 0, 9)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO literals VALUES ('lit2', 'f1', 'src/workspace.rs', 'rust', 'toml', '\"serde\"', 'carrier', NULL, 1, 0, 1, 7, 0, 7)",
        [],
    )
    .unwrap();
    drop(conn);

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("facts")
        .arg("config")
        .output()
        .expect("Failed to execute facts");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Cargo.toml") && stdout.contains("src/workspace.rs"),
        "stdout should contain both files: {}",
        stdout
    );

    // Test with --path filter scoping to Cargo.toml
    let output_filtered = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("facts")
        .arg("config")
        .arg("--path")
        .arg("Cargo.toml")
        .output()
        .expect("Failed to execute facts with path filter");
    assert!(output_filtered.status.success());
    let stdout_filt = String::from_utf8_lossy(&output_filtered.stdout);
    assert!(stdout_filt.contains("Cargo.toml"));
    assert!(!stdout_filt.contains("src/workspace.rs"));

    // Test with --file alias scoping to src
    let output_alias = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("facts")
        .arg("config")
        .arg("--file")
        .arg("src")
        .output()
        .expect("Failed to execute facts with file alias");
    assert!(output_alias.status.success());
    let stdout_alias = String::from_utf8_lossy(&output_alias.stdout);
    assert!(!stdout_alias.contains("Cargo.toml"));
    assert!(stdout_alias.contains("src/workspace.rs"));

    // Test category discovery with --path filter (Finding 6)
    let output_cat_path = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("facts")
        .arg("--path")
        .arg("Cargo.toml")
        .output()
        .expect("Failed to execute facts discovery with path filter");
    assert!(output_cat_path.status.success());
    let stdout_cat = String::from_utf8_lossy(&output_cat_path.stdout);
    assert!(stdout_cat.contains("`toml.key_value.v1` (1 occurrences)"));
}

#[test]
fn test_cli_facts_alias_limits_and_listing() {
    let repo = setup_test_repo();
    let root = repo.path();

    let db_path = root.join(".code-kb/artifact.db");
    let conn = code_kb_core::open_read_write(&db_path).unwrap();
    conn.execute_batch(
        "INSERT INTO structural_facts VALUES
            ('sf_css', 'f1', 'web/site.css', 'css', 'css.media_query.v1', 'media', 'media_statement', NULL, 1, 1, 1.0, NULL),
            ('sf_sql1', 'f1', 'db/a.sql', 'sql', 'sql.select_query.v1', 'select', 'select_statement', NULL, 1, 1, 1.0, NULL),
            ('sf_sql2', 'f1', 'db/a.sql', 'sql', 'sql.insert_statement.v1', 'insert', 'insert_statement', NULL, 2, 2, 1.0, NULL);
         INSERT INTO literals VALUES
            ('lit_a', 'f1', 'db/a.sql', 'sql', 'sql_query', 'SELECT 1', 'string', NULL, 1, 0, 1, 8, 0, 8),
            ('lit_b', 'f1', 'db/a.sql', 'sql', 'sql_query', 'SELECT 2', 'string', NULL, 2, 0, 2, 8, 9, 17);",
    )
    .unwrap();
    drop(conn);

    let run = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .args(args)
            .output()
            .expect("Failed to execute facts");
        assert!(out.status.success(), "{args:?} failed");
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    let sql = run(&["facts", "sql"]);
    assert!(!sql.contains("css.media_query.v1"), "{sql}");
    assert!(sql.contains("sql.select_query.v1"), "{sql}");

    let capped = run(&["facts", "sql", "--limit", "1"]);
    assert!(capped.contains("Matching literals"), "{capped}");
    assert_eq!(capped.matches("limit reached").count(), 2, "{capped}");

    let zero = run(&["facts", "sql", "--limit", "0"]);
    assert!(zero.contains("limit is 0"), "{zero}");
    assert!(!zero.contains("No facts match"), "{zero}");

    let listing = run(&["facts"]);
    assert!(listing.starts_with("Aliases:"), "{listing}");
    assert!(listing.contains("sql (2 patterns, 2 facts)"), "{listing}");
}

#[test]
fn test_cli_hook_session_start() {
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("hook")
        .arg("SessionStart")
        .output()
        .expect("Failed to execute hook SessionStart");

    assert!(output.status.success());
    let val: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hook_output = val
        .get("hookSpecificOutput")
        .expect("hookSpecificOutput key");
    assert_eq!(hook_output.get("hookEventName").unwrap(), "SessionStart");
    let ctx = hook_output
        .get("additionalContext")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(ctx.contains("Code Intelligence: Always use `code-kb` MCP tools"));
    assert!(ctx.contains("EnterWorktree"));
    assert!(ctx.contains("absolute path inside it"));
    assert!(ctx.contains("Binding is server-wide"));
}

#[test]
fn test_cli_hook_default_is_session_start() {
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("hook")
        .output()
        .expect("Failed to execute hook");

    assert!(output.status.success());
    let val: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hook_output = val
        .get("hookSpecificOutput")
        .expect("hookSpecificOutput key");
    assert_eq!(hook_output.get("hookEventName").unwrap(), "SessionStart");
}

#[test]
fn test_cli_hook_subagent_start() {
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("hook")
        .arg("SubagentStart")
        .output()
        .expect("Failed to execute hook SubagentStart");

    assert!(output.status.success());
    let val: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let hook_output = val
        .get("hookSpecificOutput")
        .expect("hookSpecificOutput key");
    assert_eq!(hook_output.get("hookEventName").unwrap(), "SubagentStart");
    let ctx = hook_output
        .get("additionalContext")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(ctx.contains("Code Intelligence: Always use `code-kb` MCP tools"));
}

#[test]
fn test_cli_hook_outside_repo() {
    let temp_dir = code_kb_core::safe_tempdir();
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .current_dir(temp_dir.path())
        .arg("hook")
        .output()
        .expect("Failed to execute hook outside repo");

    assert!(output.status.success());
    let val: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(val.get("hookSpecificOutput").is_some());
}

#[test]
fn test_cli_hook_copilot_env() {
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("COPILOT_PLUGIN_DATA", "1")
        .arg("hook")
        .arg("SessionStart")
        .output()
        .expect("Failed to execute hook with COPILOT_PLUGIN_DATA");

    assert!(output.status.success());
    let val: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(val.get("hookSpecificOutput").is_none());
    let ctx = val.get("additionalContext").unwrap().as_str().unwrap();
    assert!(ctx.contains("Code Intelligence: Always use `code-kb` MCP tools"));

    let subagent_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("COPILOT_PLUGIN_DATA", "1")
        .arg("hook")
        .arg("SubagentStart")
        .output()
        .expect("Failed to execute hook SubagentStart with COPILOT_PLUGIN_DATA");

    assert!(subagent_output.status.success());
    let val2: serde_json::Value = serde_json::from_slice(&subagent_output.stdout).unwrap();
    assert!(val2.as_object().unwrap().is_empty());
}

#[test]
fn test_cli_hook_pre_invocation() {
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("hook")
        .arg("PreInvocation")
        .output()
        .expect("Failed to execute hook PreInvocation");

    assert!(output.status.success());
    let val: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let steps = val
        .get("injectSteps")
        .expect("injectSteps key for Antigravity contract")
        .as_array()
        .expect("injectSteps array");
    assert_eq!(steps.len(), 1);
    let msg = steps[0]
        .get("ephemeralMessage")
        .expect("ephemeralMessage key")
        .as_str()
        .expect("string message");
    assert!(msg.contains("Code Intelligence: Always use `code-kb` MCP tools"));
}

#[test]
fn test_agents_and_claude_md_sync_contract() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let agents = std::fs::read_to_string(root.join("AGENTS.md")).unwrap();
    let claude = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
    assert_eq!(
        agents, claude,
        "AGENTS.md and CLAUDE.md must be byte-for-byte identical"
    );
}

#[test]
fn test_skills_md_sync_contract() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let skills: Vec<_> = std::fs::read_dir(root.join("skills"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        skills.len() >= 2,
        "expected the code-kb and telemetry skills"
    );
    for skill in skills {
        let relative = std::path::Path::new("skills").join(&skill).join("SKILL.md");
        let skill_root = std::fs::read_to_string(root.join(&relative)).unwrap();
        let skill_plugin =
            std::fs::read_to_string(root.join(".claude-plugin").join(&relative)).unwrap();
        assert_eq!(
            skill_root,
            skill_plugin,
            "{} and .claude-plugin/{} must be byte-for-byte identical",
            relative.display(),
            relative.display()
        );
    }
}

#[test]
fn test_cli_windows_path_argument_variations() {
    let repo = setup_test_repo();
    let root = repo.path();

    // 1. Backslash relative path in skeleton
    let out1 = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("skeleton")
        .arg(r"src\workspace.rs")
        .output()
        .expect("Failed to execute skeleton with backslashes");
    assert!(out1.status.success());
    let stdout1 = String::from_utf8_lossy(&out1.stdout);
    assert!(stdout1.contains("pub struct Workspace"));

    // 2. Mixed slashes in skeleton
    let out2 = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("skeleton")
        .arg(r".\src/workspace.rs")
        .output()
        .expect("Failed to execute skeleton with mixed slashes");
    assert!(out2.status.success());
    let stdout2 = String::from_utf8_lossy(&out2.stdout);
    assert!(stdout2.contains("pub struct Workspace"));

    // 3. Backslash path filter in symbol
    let out3 = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("symbol")
        .arg("Workspace")
        .arg("--path")
        .arg(r"src\workspace.rs")
        .output()
        .expect("Failed to execute symbol with backslash --path filter");
    assert!(out3.status.success());
    let stdout3 = String::from_utf8_lossy(&out3.stdout);
    assert!(stdout3.contains("pub struct Workspace"));

    // 4. Inverted drive letter casing in --root
    #[cfg(windows)]
    {
        let root_str = root.to_string_lossy().to_string();
        let inverted_root = if root_str.len() >= 2 && root_str.as_bytes()[1] == b':' {
            let first_char = root_str.chars().next().unwrap();
            let toggled = if first_char.is_ascii_uppercase() {
                first_char.to_ascii_lowercase()
            } else {
                first_char.to_ascii_uppercase()
            };
            format!("{}{}", toggled, &root_str[1..])
        } else {
            root_str.clone()
        };
        let out4 = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(&inverted_root)
            .arg("outline")
            .output()
            .expect("Failed to execute outline with inverted drive casing");
        assert!(out4.status.success());
        let stdout4 = String::from_utf8_lossy(&out4.stdout);
        assert!(stdout4.contains("workspace.rs"));
    }
}

#[test]
fn test_cli_json_strict_forward_slash_invariants() {
    let repo = setup_test_repo();
    let root = repo.path();

    // Populate a sample structural fact and literal so facts returns non-empty data with paths
    {
        let db_path = root.join(".code-kb").join("artifact.db");
        let conn = code_kb_core::open_read_write(&db_path).unwrap();
        conn.execute(
            "INSERT INTO structural_facts VALUES ('sf1', 'f1', 'src/workspace.rs', 'rust', 'axum.route.v1', 'get_index', 'route', 's1', 1, 10, 1.0, NULL)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO literals VALUES ('lit1', 'f1', 'src/workspace.rs', 'rust', 'string', 'hello', 'identifier', 's1', 1, 0, 1, 5, 0, 5)",
            [],
        ).unwrap();
    }

    fn assert_all_paths_forward_slash(val: &serde_json::Value, found_paths: &mut usize) {
        match val {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    let k_lower = k.to_ascii_lowercase();
                    if (k_lower.contains("path") || k_lower.contains("file")) && v.is_string() {
                        let s = v.as_str().unwrap();
                        assert!(
                            !s.contains('\\'),
                            "Key '{k}' contains backslash in JSON output: '{s}'"
                        );
                        *found_paths += 1;
                    }
                    assert_all_paths_forward_slash(v, found_paths);
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    assert_all_paths_forward_slash(item, found_paths);
                }
            }
            _ => {}
        }
    }

    let commands: Vec<Vec<&str>> = vec![
        vec!["--json", "outline"],
        vec!["--json", "skeleton", "src/workspace.rs"],
        vec!["--json", "symbol", "Workspace"],
        vec!["--json", "lookup", "Workspace"],
        vec!["--json", "search", "discovery"],
        vec!["--json", "body", "run_task"],
        vec!["--json", "slice", "run_task"],
        vec!["--json", "context", "run_task"],
        vec!["--json", "refs", "helper"],
        vec!["--json", "blast-radius", "helper"],
        vec!["--json", "facts"],
        vec!["--json", "facts", "route"],
    ];

    for cmd_args in commands {
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .args(&cmd_args)
            .output()
            .unwrap_or_else(|e| panic!("Failed to execute command {:?}: {e}", cmd_args));
        assert!(
            out.status.success(),
            "Command failed: {:?}\nstderr: {}",
            cmd_args,
            String::from_utf8_lossy(&out.stderr)
        );
        let val: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "Failed to parse JSON for {:?}: {e}\nstdout: {}",
                cmd_args,
                String::from_utf8_lossy(&out.stdout)
            )
        });
        let mut found = 0;
        assert_all_paths_forward_slash(&val, &mut found);
    }
}

#[test]
fn test_cli_argument_aliases() {
    let temp_repo = setup_test_repo();
    let root = temp_repo.path();

    // 1. --file-path on refs
    let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args([
            "--json",
            "refs",
            "helper",
            "--file-path",
            "src/workspace.rs",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "refs --file-path failed");

    // 2. --path on refs
    let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(["--json", "refs", "helper", "--path", "src/workspace.rs"])
        .output()
        .unwrap();
    assert!(out.status.success(), "refs --path failed");

    // 3. --file-path on blast-radius
    let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(["--json", "blast-radius", "--file-path", "src/workspace.rs"])
        .output()
        .unwrap();
    assert!(out.status.success(), "blast-radius --file-path failed");
}

#[cfg(unix)]
#[test]
fn test_cli_scan_prefers_the_pinned_extractor_over_a_vendored_one_above_cwd() {
    use std::os::unix::fs::PermissionsExt;
    let real = code_kb_core::find_julie_extract_binary().expect("julie-extract must be present");
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path();
    std::fs::create_dir_all(root.join(".tools")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn from_pinned() {}\n").unwrap();
    let vendored = root.join(".tools/julie-extract");
    std::fs::write(&vendored, "#!/bin/sh\necho 'julie-extract 0.0.1'\nexit 7\n").unwrap();
    std::fs::set_permissions(&vendored, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        real.parent().unwrap().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env_remove("JULIE_EXTRACT_BIN")
        .env("PATH", path)
        .current_dir(root)
        .arg("scan")
        .output()
        .expect("Failed to execute scan");

    assert!(
        output.status.success(),
        "scan must use the pinned extractor, not the vendored 0.0.1: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let conn = code_kb_core::open_read_only(&root.join(".code-kb/artifact.db")).unwrap();
    let version: String = conn
        .query_row(
            "SELECT value FROM artifact_metadata WHERE key = 'binary_version'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(version, "0.0.1");
}

#[cfg(unix)]
fn scan_with_fake_extractor() -> (tempfile::TempDir, Vec<String>, u32) {
    use std::os::unix::fs::PermissionsExt;
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    let args_log = root.join("extractor-args.txt");
    let fake = root.join("fake-julie-extract");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version) echo 'julie-extract {}'; exit 0;; esac\nprintf '%s\\n' \"$@\" > \"$EXTRACTOR_ARGS_LOG\"\n",
            code_kb_core::PINNED_JULIE_VERSION
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("JULIE_EXTRACT_BIN", &fake)
        .env("EXTRACTOR_ARGS_LOG", &args_log)
        .current_dir(root)
        .arg("scan")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("Failed to spawn scan");
    let code_kb_pid = child.id();
    let _ = child.wait_with_output();

    let args = std::fs::read_to_string(&args_log).expect("fake extractor must have run");
    let args = args.lines().map(str::to_string).collect();
    (temp_dir, args, code_kb_pid)
}

#[cfg(unix)]
#[test]
fn test_cli_scan_passes_its_own_pid_to_the_extractor_watchdog() {
    let (_repo, args, code_kb_pid) = scan_with_fake_extractor();
    let idx = args
        .iter()
        .position(|a| a == "--parent-pid")
        .expect("scan must pass --parent-pid");
    assert_eq!(args[idx + 1], code_kb_pid.to_string());
}

#[cfg(unix)]
#[test]
fn test_cli_scan_requests_the_facts_extraction_level() {
    let (_repo, args, _) = scan_with_fake_extractor();
    let idx = args
        .iter()
        .position(|a| a == "--level")
        .expect("scan must pass --level");
    assert_eq!(args[idx + 1], "facts");
}

#[test]
fn test_cli_query_builds_and_refreshes_the_index_without_scan() {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub fn indexed_on_first_query() {}\n",
    )
    .unwrap();

    let lookup = |name: &str| {
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .current_dir(root)
            .args(["lookup", name])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "lookup {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    };

    assert!(lookup("indexed_on_first_query").contains("indexed_on_first_query"));
    assert!(root.join(".code-kb/artifact.db").exists());

    std::fs::write(
        root.join("src/later.rs"),
        "pub fn added_while_no_server_ran() {}\n",
    )
    .unwrap();
    assert!(lookup("added_while_no_server_ran").contains("added_while_no_server_ran"));
}

#[test]
fn test_cli_lookup_reports_a_broken_search_index() {
    let repo = setup_test_repo();
    let root = repo.path();

    let conn = code_kb_core::open_read_write(&root.join(".code-kb").join("artifact.db")).unwrap();
    conn.execute_batch("DROP TABLE symbols_fts_data;").unwrap();
    drop(conn);

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("lookup")
        .arg("ZzzAbsentSymbol")
        .output()
        .expect("Failed to execute lookup");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("Database query error"), "stderr: {stderr}");
    assert!(stderr.contains("symbols_fts"), "stderr: {stderr}");
    assert!(!stdout.contains("No exact name match"), "stdout: {stdout}");
}

#[test]
fn test_guidance_docs_use_canonical_parameter_names() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    for doc in [
        "hooks/code-kb-routing-block.md",
        "crates/code-kb-cli/src/routing-block.md",
        "skills/code-kb/SKILL.md",
        "README.md",
    ] {
        let text = std::fs::read_to_string(root.join(doc)).unwrap();
        for (index, line) in text.lines().enumerate() {
            let position = format!("{doc}:{}", index + 1);
            assert!(
                !line.contains("codebase_outline(subpath"),
                "{position} calls codebase_outline with subpath"
            );
            assert!(
                !line.contains("blast_radius(path="),
                "{position} calls blast_radius with path"
            );
        }
    }
}

#[test]
fn test_routing_block_omits_edit_tools_and_stays_under_three_kilobytes() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let text = std::fs::read_to_string(root.join("hooks/code-kb-routing-block.md")).unwrap();
    assert!(
        !text.contains("edit_file("),
        "routing block still names edit_file"
    );
    assert!(
        !text.contains("replace_symbol_body("),
        "routing block still names replace_symbol_body"
    );
    assert!(text.len() < 3072, "routing block is {} bytes", text.len());
}

#[test]
fn test_cli_rejects_removed_edit_commands() {
    for command in ["edit", "edit-file"] {
        let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg(command)
            .output()
            .expect("failed to execute code-kb");
        assert!(!output.status.success(), "{command} remains available");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unrecognized subcommand"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_todo_file_holds_no_checklist() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let text = std::fs::read_to_string(root.join("TODO.md")).unwrap();
    for (index, line) in text.lines().enumerate() {
        assert!(
            !line.trim_start().starts_with("- ["),
            "TODO.md:{} still holds a checklist item",
            index + 1
        );
    }
    assert!(text.contains("docs/plans/"), "TODO.md omits the plans path");
}

#[test]
fn test_routing_block_sync_contract() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let hook_copy = std::fs::read_to_string(root.join("hooks/code-kb-routing-block.md")).unwrap();
    let compiled_copy = std::fs::read_to_string(manifest_dir.join("src/routing-block.md")).unwrap();
    assert_eq!(
        hook_copy, compiled_copy,
        "hooks/code-kb-routing-block.md and crates/code-kb-cli/src/routing-block.md must be byte-for-byte identical"
    );
}
