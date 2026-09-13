use serde_json::{Value, json};
use std::io::Write;
use std::process::{Child, Command, Stdio};

struct ChildGuard(Child);

impl std::ops::Deref for ChildGuard {
    type Target = Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn test_mcp_stdio_handshake_and_tools() {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let db_dir = root.join(".code-kb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("artifact.db");

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let content = "pub struct Workspace {\n    pub root: String,\n}\n";
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
        "INSERT INTO files VALUES ('f1', 'src/workspace.rs', 'rust', ?1, ?2, 3, '2026-01-01')",
        rusqlite::params![hash, bytes],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/workspace.rs', 'rust', 'Workspace', 'struct',
            'pub struct Workspace', 'Workspace representation for code-kb workspace discovery root', 'pub', NULL,
            1, 0, 3, 1, 0, ?1, 1, 21, 3, 1, 21, ?1, 'b3:hash',
            NULL, 0, 0
        )",
        rusqlite::params![bytes],
    )
    .unwrap();
    code_kb_core::db::ensure_fts_index(&conn).unwrap();
    drop(conn);

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = std::io::BufReader::new(stdout);

    // 1. Send initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-agent",
                "version": "1.0"
            }
        }
    });

    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line = String::new();
    use std::io::BufRead;
    reader.read_line(&mut response_line).unwrap();

    let resp: Value = serde_json::from_str(&response_line).expect("Failed to parse JSON response");
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["serverInfo"]["name"], "code-kb");
    assert_eq!(
        resp["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        resp["result"]["instructions"]
            .as_str()
            .is_some_and(|instructions| !instructions.is_empty())
    );

    // 2. Send tools/list
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    let mut line2 = serde_json::to_string(&list_req).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line2 = String::new();
    reader.read_line(&mut response_line2).unwrap();

    let resp2: Value =
        serde_json::from_str(&response_line2).expect("Failed to parse JSON response");
    assert_eq!(resp2["id"], 2);
    let tools = resp2["result"]["tools"]
        .as_array()
        .expect("Expected tools array");
    assert_eq!(tools.len(), 10);

    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();

    assert!(tool_names.contains(&"codebase_outline"));
    assert!(tool_names.contains(&"file_skeleton"));
    assert!(tool_names.contains(&"find_symbol"));
    assert!(tool_names.contains(&"search_symbols"));
    assert!(tool_names.contains(&"get_symbol_body"));
    assert!(tool_names.contains(&"get_context_slice"));
    assert!(tool_names.contains(&"find_references"));
    assert!(tool_names.contains(&"find_structural_facts"));
    assert!(tool_names.contains(&"blast_radius"));
    assert!(tool_names.contains(&"replace_symbol_body"));

    // Verify zero workspace pollution across all tools
    for tool in tools {
        let schema = &tool["inputSchema"];
        let props = &schema["properties"];
        assert!(
            props.get("workspace").is_none()
                && props.get("workspace_id").is_none()
                && props.get("repo_path").is_none()
                && props.get("root_dir").is_none(),
            "Tool '{}' should NOT expose workspace parameters in schema",
            tool["name"]
        );
    }

    // 3. Send tools/call find_symbol
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "find_symbol",
            "arguments": {
                "query": "Workspace",
                "kind": "struct"
            }
        }
    });

    let mut line3 = serde_json::to_string(&call_req).unwrap();
    line3.push('\n');
    stdin.write_all(line3.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line3 = String::new();
    reader.read_line(&mut response_line3).unwrap();

    let resp3: Value =
        serde_json::from_str(&response_line3).expect("Failed to parse JSON response");
    assert_eq!(resp3["id"], 3);
    let content_text = resp3["result"]["content"][0]["text"].as_str().unwrap();
    assert!(content_text.contains("Workspace"));

    // 4. Send tools/call search_symbols (FTS5 conceptual search)
    let search_req = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "search_symbols",
            "arguments": {
                "query": "workspace discovery root"
            }
        }
    });

    let mut line4 = serde_json::to_string(&search_req).unwrap();
    line4.push('\n');
    stdin.write_all(line4.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line4 = String::new();
    reader.read_line(&mut response_line4).unwrap();
    let resp4: Value =
        serde_json::from_str(&response_line4).expect("Failed to parse JSON response");
    assert_eq!(resp4["id"], 4);
    let search_text = resp4["result"]["content"][0]["text"].as_str().unwrap();
    assert!(search_text.contains("Workspace"));

    // 5. Test find_references with alias "symbol" and omitted "direction" (should default to callers)
    let refs_req = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "find_references",
            "arguments": {
                "symbol": "Workspace"
            }
        }
    });
    let mut line5 = serde_json::to_string(&refs_req).unwrap();
    line5.push('\n');
    stdin.write_all(line5.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line5 = String::new();
    reader.read_line(&mut response_line5).unwrap();
    let resp5: Value =
        serde_json::from_str(&response_line5).expect("Failed to parse JSON response");
    assert_eq!(resp5["id"], 5);
    assert!(resp5["error"].is_null());
    assert_ne!(resp5["result"]["isError"], true);
    let refs_text = resp5["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        refs_text.contains("Callers")
            || refs_text.contains("No callers found")
            || refs_text.contains("Workspace")
    );

    // 6. Test find_structural_facts with no category argument (lists categories)
    let facts_req = json!({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "find_structural_facts",
            "arguments": {}
        }
    });
    let mut line6 = serde_json::to_string(&facts_req).unwrap();
    line6.push('\n');
    stdin.write_all(line6.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line6 = String::new();
    reader.read_line(&mut response_line6).unwrap();
    let resp6: Value =
        serde_json::from_str(&response_line6).expect("Failed to parse JSON response");
    assert_eq!(resp6["id"], 6);
    assert!(resp6["error"].is_null());
    assert_ne!(resp6["result"]["isError"], true);
    let facts_text = resp6["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        facts_text.contains("Available structural fact categories")
            || facts_text.contains("No structural facts")
    );

    // 7. Test file_skeleton with alias "file" instead of "file_path"
    let skeleton_req = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": {
                "file": "src/workspace.rs"
            }
        }
    });
    let mut line7 = serde_json::to_string(&skeleton_req).unwrap();
    line7.push('\n');
    stdin.write_all(line7.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line7 = String::new();
    reader.read_line(&mut response_line7).unwrap();
    let resp7: Value =
        serde_json::from_str(&response_line7).expect("Failed to parse JSON response");
    assert_eq!(resp7["id"], 7);
    let skeleton_text = resp7["result"]["content"][0]["text"].as_str().unwrap();
    assert!(skeleton_text.contains("pub struct Workspace"));

    // 8. Test blast_radius via MCP
    let blast_req = json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "tools/call",
        "params": {
            "name": "blast_radius",
            "arguments": {
                "symbol": "Workspace"
            }
        }
    });
    let mut line8 = serde_json::to_string(&blast_req).unwrap();
    line8.push('\n');
    stdin.write_all(line8.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line8 = String::new();
    reader.read_line(&mut response_line8).unwrap();
    let resp8: Value =
        serde_json::from_str(&response_line8).expect("Failed to parse JSON response");
    assert_eq!(resp8["id"], 8);
    let blast_text = resp8["result"]["content"][0]["text"].as_str().unwrap();
    assert!(blast_text.contains("Blast Radius"));

    // 9. Test get_context_slice via MCP (verifies include_external parameter and tool dispatch)
    let slice_req = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "get_context_slice",
            "arguments": {
                "symbol_name": "Workspace",
                "include_external": false
            }
        }
    });
    let mut line9 = serde_json::to_string(&slice_req).unwrap();
    line9.push('\n');
    stdin.write_all(line9.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line9 = String::new();
    reader.read_line(&mut response_line9).unwrap();
    let resp9: Value =
        serde_json::from_str(&response_line9).expect("Failed to parse JSON response");
    assert_eq!(resp9["id"], 9);
    assert_ne!(resp9["result"]["isError"], true);
    let slice_text = resp9["result"]["content"][0]["text"].as_str().unwrap();
    assert!(slice_text.contains("Workspace"));

    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/roots/list_changed"
    });
    let mut notification_line = serde_json::to_string(&notification).unwrap();
    notification_line.push('\n');
    stdin.write_all(notification_line.as_bytes()).unwrap();

    let ping_req = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "ping"
    });
    let mut ping_line = serde_json::to_string(&ping_req).unwrap();
    ping_line.push('\n');
    stdin.write_all(ping_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line9 = String::new();
    reader.read_line(&mut response_line9).unwrap();
    let resp9: Value =
        serde_json::from_str(&response_line9).expect("Failed to parse JSON response");
    assert_eq!(resp9["id"], 9);

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_mcp_invalid_path_does_not_poison_session() {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let db_dir = root.join(".code-kb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("artifact.db");

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let content = "pub fn valid_func() {}\n";
    let file_path = src_dir.join("lib.rs");
    std::fs::write(&file_path, content).unwrap();

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
        );",
    )
    .unwrap();
    let bytes = content.len() as i64;
    let hash = format!("blake3:{}", blake3::hash(content.as_bytes()).to_hex());
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/lib.rs', 'rust', ?1, ?2, 1, '2026-01-01')",
        rusqlite::params![hash, bytes],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/lib.rs', 'rust', 'valid_func', 'function',
            'pub fn valid_func()', NULL, 'pub', NULL,
            1, 0, 1, 22, 0, ?1, 1, 0, 1, 22, 0, ?1, 'b3:hash',
            NULL, 0, 0
        )",
        rusqlite::params![bytes],
    )
    .unwrap();
    code_kb_core::db::ensure_fts_index(&conn).unwrap();
    drop(conn);

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = std::io::BufReader::new(stdout);
    use std::io::BufRead;

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-agent", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    // 2. Call file_skeleton with an invalid non-workspace path
    let invalid_call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": {
                "file": "/nonexistent_abs_path/nowhere/does_not_exist.rs"
            }
        }
    });
    let mut line2 = serde_json::to_string(&invalid_call).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();
    let resp2: Value = serde_json::from_str(&resp_line2).unwrap();
    assert_eq!(resp2["id"], 2);
    assert!(resp2["result"]["isError"] == true || resp2["error"].is_object());

    // 3. Subsequent call with valid relative path must STILL succeed (session not poisoned)
    let valid_call = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": {
                "file": "src/lib.rs"
            }
        }
    });
    let mut line3 = serde_json::to_string(&valid_call).unwrap();
    line3.push('\n');
    stdin.write_all(line3.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line3 = String::new();
    reader.read_line(&mut resp_line3).unwrap();
    let resp3: Value = serde_json::from_str(&resp_line3).unwrap();
    assert_eq!(resp3["id"], 3);
    assert_ne!(resp3["result"]["isError"], true);
    let text = resp3["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("valid_func"),
        "Must retrieve valid_func from original workspace"
    );

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_mcp_worktree_rebind() {
    let temp_dir = code_kb_core::safe_tempdir();
    let main_root = temp_dir.path().join("main_repo");
    let wt_root = main_root.join(".worktrees").join("feature-x");

    std::fs::create_dir_all(main_root.join(".git")).unwrap();
    std::fs::create_dir_all(main_root.join(".code-kb")).unwrap();
    std::fs::create_dir_all(main_root.join("src")).unwrap();

    let main_file = main_root.join("src").join("main.rs");
    let main_content = "pub fn main_fn() {}\n";
    std::fs::write(&main_file, main_content).unwrap();

    // Set up main repo DB
    let main_db = main_root.join(".code-kb").join("artifact.db");
    let conn = code_kb_core::open_read_write(&main_db).unwrap();
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
        );",
    )
    .unwrap();
    let bytes = main_content.len() as i64;
    let hash = format!("blake3:{}", blake3::hash(main_content.as_bytes()).to_hex());
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/main.rs', 'rust', ?1, ?2, 1, '2026-01-01')",
        rusqlite::params![hash, bytes],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/main.rs', 'rust', 'main_fn', 'function',
            'pub fn main_fn()', NULL, 'pub', NULL,
            1, 0, 1, 19, 0, ?1, 1, 0, 1, 19, 0, ?1, 'b3:hash',
            NULL, 0, 0
        )",
        rusqlite::params![bytes],
    )
    .unwrap();
    code_kb_core::db::ensure_fts_index(&conn).unwrap();
    drop(conn);

    // Set up worktree: .git file pointing to main gitdir, plus its own DB
    std::fs::create_dir_all(wt_root.join(".code-kb")).unwrap();
    std::fs::create_dir_all(wt_root.join("src")).unwrap();
    let gitdir_path = main_root.join(".git").join("worktrees").join("feature-x");
    std::fs::create_dir_all(&gitdir_path).unwrap();
    std::fs::write(
        wt_root.join(".git"),
        format!("gitdir: {}\n", gitdir_path.display()),
    )
    .unwrap();

    let wt_file = wt_root.join("src").join("feature.rs");
    let wt_content = "pub fn feature_fn() {}\n";
    std::fs::write(&wt_file, wt_content).unwrap();

    let wt_db = wt_root.join(".code-kb").join("artifact.db");
    let conn_wt = code_kb_core::open_read_write(&wt_db).unwrap();
    conn_wt.execute_batch(
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
        );",
    )
    .unwrap();
    let wt_bytes = wt_content.len() as i64;
    let wt_hash = format!("blake3:{}", blake3::hash(wt_content.as_bytes()).to_hex());
    conn_wt
        .execute(
            "INSERT INTO files VALUES ('f2', 'src/feature.rs', 'rust', ?1, ?2, 1, '2026-01-01')",
            rusqlite::params![wt_hash, wt_bytes],
        )
        .unwrap();
    conn_wt
        .execute(
            "INSERT INTO symbols VALUES (
            's2', 'f2', 'src/feature.rs', 'rust', 'feature_fn', 'function',
            'pub fn feature_fn()', NULL, 'pub', NULL,
            1, 0, 1, 22, 0, ?1, 1, 0, 1, 22, 0, ?1, 'b3:hash',
            NULL, 0, 0
        )",
            rusqlite::params![wt_bytes],
        )
        .unwrap();
    code_kb_core::db::ensure_fts_index(&conn_wt).unwrap();
    drop(conn_wt);

    // Spawn server pointing to main_root
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&main_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = std::io::BufReader::new(stdout);
    use std::io::BufRead;

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-agent", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    // 2. Query symbol from main_repo
    let main_call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "find_symbol",
            "arguments": { "query": "main_fn" }
        }
    });
    let mut line2 = serde_json::to_string(&main_call).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();
    let resp2: Value = serde_json::from_str(&resp_line2).unwrap();
    assert_eq!(resp2["id"], 2);
    assert!(
        resp2["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("main_fn")
    );

    // 3. Query file_skeleton with absolute path to file in worktree
    let wt_call = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": wt_file.to_string_lossy().to_string() }
        }
    });
    let mut line3 = serde_json::to_string(&wt_call).unwrap();
    line3.push('\n');
    stdin.write_all(line3.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line3 = String::new();
    reader.read_line(&mut resp_line3).unwrap();
    let resp3: Value = serde_json::from_str(&resp_line3).unwrap();
    assert_eq!(resp3["id"], 3);
    assert!(
        resp3["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("feature_fn")
    );

    // 4. Query file_skeleton with absolute path back to main repo
    let back_call = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": main_file.to_string_lossy().to_string() }
        }
    });
    let mut line4 = serde_json::to_string(&back_call).unwrap();
    line4.push('\n');
    stdin.write_all(line4.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line4 = String::new();
    reader.read_line(&mut resp_line4).unwrap();
    let resp4: Value = serde_json::from_str(&resp_line4).unwrap();
    assert_eq!(resp4["id"], 4);
    assert!(
        resp4["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("main_fn")
    );

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_mcp_worktree_auto_copy_fast_path() {
    let temp_dir = code_kb_core::safe_tempdir();
    let main_root = temp_dir.path().join("main_repo");
    let wt_root = main_root.join(".worktrees").join("feature-y");

    std::fs::create_dir_all(main_root.join(".git")).unwrap();
    std::fs::create_dir_all(main_root.join(".code-kb")).unwrap();
    std::fs::create_dir_all(main_root.join("src")).unwrap();

    let main_file = main_root.join("src").join("main.rs");
    let main_content = "pub fn shared_fn() {}\n";
    std::fs::write(&main_file, main_content).unwrap();

    // Set up main repo DB
    let main_db = main_root.join(".code-kb").join("artifact.db");
    let conn = code_kb_core::open_read_write(&main_db).unwrap();
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
        );",
    )
    .unwrap();
    let bytes = main_content.len() as i64;
    let hash = format!("blake3:{}", blake3::hash(main_content.as_bytes()).to_hex());
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/main.rs', 'rust', ?1, ?2, 1, '2026-01-01')",
        rusqlite::params![hash, bytes],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/main.rs', 'rust', 'shared_fn', 'function',
            'pub fn shared_fn()', NULL, 'pub', NULL,
            1, 0, 1, 21, 0, ?1, 1, 0, 1, 21, 0, ?1, 'b3:hash',
            NULL, 0, 0
        )",
        rusqlite::params![bytes],
    )
    .unwrap();
    code_kb_core::db::ensure_fts_index(&conn).unwrap();
    drop(conn);

    // Set up worktree: .git file pointing to main gitdir, and identical file in src/, but NO .code-kb folder!
    std::fs::create_dir_all(wt_root.join("src")).unwrap();
    let gitdir_path = main_root.join(".git").join("worktrees").join("feature-y");
    std::fs::create_dir_all(&gitdir_path).unwrap();
    std::fs::write(
        wt_root.join(".git"),
        format!("gitdir: {}\n", gitdir_path.display()),
    )
    .unwrap();

    let wt_file = wt_root.join("src").join("main.rs");
    std::fs::write(&wt_file, main_content).unwrap();

    let wt_db = wt_root.join(".code-kb").join("artifact.db");
    assert!(!wt_db.exists(), "Worktree DB must NOT exist initially");

    // Spawn server pointing to main_root
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&main_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = std::io::BufReader::new(stdout);
    use std::io::BufRead;

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-agent", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    // 2. Query file_skeleton on worktree file -> triggers auto-copy of parent DB!
    let wt_call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": wt_file.to_string_lossy().to_string() }
        }
    });
    let mut line2 = serde_json::to_string(&wt_call).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();
    let resp2: Value = serde_json::from_str(&resp_line2).unwrap();
    assert_eq!(resp2["id"], 2);
    assert!(
        resp2["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("shared_fn"),
        "Worktree query should succeed using copied parent DB"
    );

    // Verify worktree DB was copied
    assert!(wt_db.exists(), "Worktree DB should now exist on disk");

    drop(stdin);
    let _ = child.wait();
}
