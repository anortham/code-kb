use std::process::Command;

fn setup_test_repo() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
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
            fact_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT,
            pattern_id TEXT, kind TEXT, name TEXT, receiver TEXT, symbol_id TEXT,
            scope_symbol_id TEXT, parent_fact_id TEXT, start_line INTEGER,
            start_column INTEGER, end_line INTEGER, end_column INTEGER,
            start_byte INTEGER, end_byte INTEGER, confidence REAL, payload TEXT
        );
        CREATE TABLE literals (
            literal_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT,
            kind TEXT, value TEXT, scope_symbol_id TEXT, start_line INTEGER,
            start_column INTEGER, end_line INTEGER, end_column INTEGER,
            start_byte INTEGER, end_byte INTEGER
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
fn test_cli_symbol_and_search() {
    let repo = setup_test_repo();
    let root = repo.path();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("symbol")
        .arg("Workspace")
        .output()
        .expect("Failed to execute symbol");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Workspace"));

    let search_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("search")
        .arg("discovery")
        .output()
        .expect("Failed to execute search");

    assert!(search_output.status.success());
    let search_stdout = String::from_utf8_lossy(&search_output.stdout);
    assert!(search_stdout.contains("Workspace"));
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
fn test_cli_stats_and_telemetry() {
    let repo = setup_test_repo();
    let root = repo.path();

    let stats_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("stats")
        .output()
        .expect("Failed to execute stats");

    assert!(stats_output.status.success());
    let stdout = String::from_utf8_lossy(&stats_output.stdout);
    assert!(stdout.contains("Telemetry Summary"));

    let json_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
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
fn test_cli_prune() {
    let repo = setup_test_repo();
    let root = repo.path();

    let prune_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("prune")
        .arg("--dry-run")
        .output()
        .expect("Failed to execute prune");

    assert!(prune_output.status.success());
    let stdout = String::from_utf8_lossy(&prune_output.stdout);
    assert!(stdout.contains("No orphaned stores found") || stdout.contains("orphaned"));
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
fn test_cli_edit_atomic_replacement() {
    if code_kb_core::find_julie_extract_binary().is_none() {
        eprintln!("Skipping test_cli_edit_atomic_replacement: julie-extract not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let content = "pub fn run_task() {\n    helper();\n}\n\nfn helper() {}\n";
    std::fs::write(src_dir.join("workspace.rs"), content).unwrap();

    // Scan the workspace first to initialize schema and artifact_metadata
    let scan_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(&root)
        .arg("scan")
        .output()
        .expect("Failed to execute scan");
    assert!(
        scan_output.status.success(),
        "Scan failed: {}",
        String::from_utf8_lossy(&scan_output.stderr)
    );

    let new_body = "{\n    // updated task body\n    let _x = 42;\n    helper();\n}";
    let edit_output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(&root)
        .arg("edit")
        .arg("run_task")
        .arg("--file")
        .arg("src/workspace.rs")
        .arg("--body")
        .arg(new_body)
        .output()
        .expect("Failed to execute edit");

    assert!(
        edit_output.status.success(),
        "Edit command failed: {}",
        String::from_utf8_lossy(&edit_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&edit_output.stdout);
    assert!(stdout.contains("Successfully replaced body") && stdout.contains("Bytes Written:"));

    // Verify file content on disk
    let disk_content = std::fs::read_to_string(root.join("src/workspace.rs")).unwrap();
    assert!(disk_content.contains("let _x = 42;"));
}
