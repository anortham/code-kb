use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
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
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn mcp_request(
    stdin: &mut std::process::ChildStdin,
    reader: &mut BufReader<std::process::ChildStdout>,
    request: &Value,
) -> Value {
    let mut line = serde_json::to_string(request).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    serde_json::from_str(&response).unwrap()
}

#[test]
fn test_mcp_blast_radius_stdio_handshake_and_tools() {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let db_dir = root.join(".code-kb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("artifact.db");

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("other.cpp"), "int other() { return 0; }\n").unwrap();
    let content = "pub struct Workspace {\n    pub root: String,\n}\nstruct Parent { int run() { return 1; } int run(int) { return 2; } };\nint first() { Parent p; return p.run(); }\nint second() { Parent p; return p.run(2); }\n";
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
            structural_fact_id TEXT PRIMARY KEY, path TEXT, language TEXT,
            pattern_id TEXT, capture_name TEXT, node_kind TEXT,
            containing_symbol_id TEXT, start_line INTEGER, end_line INTEGER,
            confidence REAL, metadata_json TEXT
        );
        CREATE TABLE literals (
            literal_id TEXT PRIMARY KEY, path TEXT, literal_text TEXT, kind TEXT,
            carrier TEXT, start_line INTEGER, containing_symbol_id TEXT
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/workspace.rs', 'rust', ?1, ?2, 3, '2026-01-01')",
        rusqlite::params![hash, bytes],
    )
    .unwrap();
    let parent_start = content.find("struct Parent").unwrap() as i64;
    let first_run = content.find("int run() ").unwrap();
    let second_run = content.find("int run(int)").unwrap();
    for (id, start, body, line) in [
        ("overload_zero", first_run, "{ return 1; }", 4_i64),
        ("overload_one", second_run, "{ return 2; }", 4_i64),
    ] {
        let body_start = content[start..].find(body).unwrap() + start;
        conn.execute(
            "INSERT INTO symbols VALUES (?1, 'f1', 'src/workspace.rs', 'cpp', 'run', 'method', 'int run', NULL, 'pub', 'over_parent', ?2, 0, ?2, 0, ?3, ?4, ?2, 0, ?2, 0, ?3, ?4, NULL, NULL, 0, 0)",
            rusqlite::params![id, line, body_start as i64, (body_start + body.len()) as i64],
        ).unwrap();
    }
    conn.execute(
        "INSERT INTO symbols VALUES ('over_parent', 'f1', 'src/workspace.rs', 'cpp', 'Parent', 'struct', 'struct Parent', NULL, 'pub', NULL, 4, 0, 4, 0, ?1, ?2, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0)",
        rusqlite::params![parent_start, content.len() as i64],
    ).unwrap();
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
    conn.execute_batch("INSERT INTO symbols VALUES ('over_first', 'f1', 'src/workspace.rs', 'cpp', 'first', 'function', NULL, NULL, NULL, NULL, 5, 0, 5, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0), ('over_second', 'f1', 'src/workspace.rs', 'cpp', 'second', 'function', NULL, NULL, NULL, NULL, 6, 0, 6, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0); INSERT INTO relationships VALUES ('over-r1', 'over_first', 'overload_zero', 'calls', 'src/workspace.rs', 5, 0); INSERT INTO relationships VALUES ('over-r2', 'over_second', 'overload_one', 'calls', 'src/workspace.rs', 6, 0);").unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's2', 'f1', 'src/workspace.rs', 'rust', 'root', 'field',
            'pub root: String', NULL, 'pub', 's1',
            2, 4, 2, 20, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
            NULL, 0, 0
        )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's3', 'f1', 'src/workspace.rs', 'rust', 'caller', 'function',
            'fn caller()', NULL, NULL, NULL,
            1, 0, 1, 0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
            NULL, 0, 0
        )",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO relationships VALUES ('r1', 's3', 's1', 'calls', 'src/workspace.rs', 1, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO pending_relationships VALUES ('s1', 'println', 'call', 'src/workspace.rs', 2, 4)",
        [],
    )
    .unwrap();
    conn.execute_batch(
        "INSERT INTO structural_facts VALUES
            ('sf1', 'src/routes.rs', 'rust', 'axum.route.v1', 'route', 'call', 's1', 1, 1, 1.0, NULL),
            ('sf2', 'src/routes.rs', 'rust', 'axum.route.v1', 'route', 'call', 's1', 2, 2, 1.0, NULL);
         INSERT INTO literals VALUES
            ('l1', 'src/routes.rs', '/first', 'route', 'string', 1, 's1'),
            ('l2', 'src/routes.rs', '/second', 'route', 'string', 2, 's1');",
    )
    .unwrap();
    code_kb_core::db::ensure_fts_index(&conn).unwrap();
    drop(conn);

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
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
    assert!(tool_names.contains(&"lookup_symbol"));
    assert!(tool_names.contains(&"search_symbols"));
    assert!(tool_names.contains(&"get_symbol_body"));
    assert!(tool_names.contains(&"get_symbol_context"));
    assert!(tool_names.contains(&"find_references"));
    assert!(tool_names.contains(&"find_structural_facts"));
    assert!(tool_names.contains(&"blast_radius"));
    assert!(tool_names.contains(&"telemetry_summary"));
    assert!(!tool_names.contains(&"replace_symbol_body"));
    assert!(!tool_names.contains(&"edit_file"));

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

    for (id, name) in [(13, "edit_file"), (14, "replace_symbol_body")] {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": {} }
        });
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut response_line = String::new();
        reader.read_line(&mut response_line).unwrap();
        let response: Value = serde_json::from_str(&response_line).unwrap();
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert_eq!(
            response["result"]["content"][0]["text"],
            format!("Unknown tool: '{name}'")
        );
    }

    // 3. Send tools/call lookup_symbol
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "lookup_symbol",
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

    for (id, limit) in [(30, json!(u64::MAX)), (31, json!(-1)), (32, json!(1.5))] {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "lookup_symbol",
                "arguments": { "query": "Workspace", "limit": limit }
            }
        });
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut response_line = String::new();
        reader.read_line(&mut response_line).unwrap();
        let response: Value = serde_json::from_str(&response_line).unwrap();
        assert_eq!(response["id"], id);
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("Invalid limit"))
        );
    }

    let zero_limit_request = json!({
        "jsonrpc": "2.0",
        "id": 33,
        "method": "tools/call",
        "params": {
            "name": "lookup_symbol",
            "arguments": { "query": "Workspace::root", "limit": 0 }
        }
    });
    let mut zero_limit_line = serde_json::to_string(&zero_limit_request).unwrap();
    zero_limit_line.push('\n');
    stdin.write_all(zero_limit_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut zero_limit_response_line = String::new();
    reader.read_line(&mut zero_limit_response_line).unwrap();
    let zero_limit_response: Value = serde_json::from_str(&zero_limit_response_line).unwrap();
    assert_eq!(zero_limit_response["id"], 33);
    assert_ne!(zero_limit_response["result"]["isError"], true);
    assert!(
        zero_limit_response["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("No symbols found"))
    );

    let filtered_lookup_request = json!({
        "jsonrpc": "2.0",
        "id": 35,
        "method": "tools/call",
        "params": {
            "name": "lookup_symbol",
            "arguments": { "query": "Workspace::root", "kind": "function" }
        }
    });
    let mut filtered_lookup_line = serde_json::to_string(&filtered_lookup_request).unwrap();
    filtered_lookup_line.push('\n');
    stdin.write_all(filtered_lookup_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut filtered_lookup_response_line = String::new();
    reader
        .read_line(&mut filtered_lookup_response_line)
        .unwrap();
    let filtered_lookup_response: Value =
        serde_json::from_str(&filtered_lookup_response_line).unwrap();
    assert_eq!(filtered_lookup_response["id"], 35);
    assert_ne!(filtered_lookup_response["result"]["isError"], true);
    assert!(
        filtered_lookup_response["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("No symbols found"))
    );

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
    assert!(facts_text.contains("categories") || facts_text.contains("No structural facts"));

    let capped_facts_req = json!({
        "jsonrpc": "2.0",
        "id": 34,
        "method": "tools/call",
        "params": {
            "name": "find_structural_facts",
            "arguments": { "category": "route", "limit": 3 }
        }
    });
    let mut capped_facts_line = serde_json::to_string(&capped_facts_req).unwrap();
    capped_facts_line.push('\n');
    stdin.write_all(capped_facts_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut capped_facts_response_line = String::new();
    reader.read_line(&mut capped_facts_response_line).unwrap();
    let capped_facts_response: Value = serde_json::from_str(&capped_facts_response_line).unwrap();
    assert_eq!(capped_facts_response["id"], 34);
    assert_ne!(capped_facts_response["result"]["isError"], true);
    let capped_facts_text = capped_facts_response["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(capped_facts_text.contains("Structural facts for 'route' (2 found):"));
    assert!(capped_facts_text.contains("Matching literals (2 found):"));

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
                "symbol": "Workspace",
                "limit": 0
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
    assert!(blast_text.contains("Requested limit hid additional impacted symbols"));
    assert!(blast_text.contains("Downstream Impact (0 returned)"));
    assert!(!blast_text.contains("No downstream callers found within depth"));

    // 9. Test get_symbol_context via MCP with include_external: false
    let slice_req = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "tools/call",
        "params": {
            "name": "get_symbol_context",
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
    assert!(
        !slice_text.contains("println"),
        "Default get_symbol_context should not contain external callee println"
    );

    // 10. Test get_symbol_context via MCP with include_external: true
    let slice_req10 = json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {
            "name": "get_symbol_context",
            "arguments": {
                "symbol_name": "Workspace",
                "include_external": true
            }
        }
    });
    let mut line10 = serde_json::to_string(&slice_req10).unwrap();
    line10.push('\n');
    stdin.write_all(line10.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line10 = String::new();
    reader.read_line(&mut response_line10).unwrap();
    let resp10: Value =
        serde_json::from_str(&response_line10).expect("Failed to parse JSON response");
    assert_eq!(resp10["id"], 10);
    assert_ne!(resp10["result"]["isError"], true);
    let slice_text10 = resp10["result"]["content"][0]["text"].as_str().unwrap();
    assert!(slice_text10.contains("Workspace"));
    assert!(
        slice_text10.contains("println"),
        "Extended get_context_slice must contain external callee println"
    );

    let id_slice = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":55,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"symbol_id":"s1","include_external":true}}}),
    );
    assert_ne!(id_slice["result"]["isError"], true, "{id_slice}");
    assert!(
        id_slice["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("println")
    );
    let id_references_default = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":56,"method":"tools/call","params":{"name":"find_references","arguments":{"symbol_id":"s1","direction":"callees"}}}),
    );
    assert_ne!(id_references_default["result"]["isError"], true);
    assert!(
        !id_references_default["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("println")
    );
    let id_references_external = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":57,"method":"tools/call","params":{"name":"find_references","arguments":{"symbol_id":"s1","direction":"callees","include_external":true}}}),
    );
    assert_ne!(id_references_external["result"]["isError"], true);
    assert!(
        id_references_external["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("println")
    );

    let mut overload_bodies = Vec::new();
    let mut overload_contexts = Vec::new();
    let mut overload_refs = Vec::new();
    let mut overload_impacts = Vec::new();
    for (id, symbol_id) in [(40, "overload_zero"), (41, "overload_one")] {
        for (offset, tool) in [
            (0, "get_symbol_body"),
            (10, "get_symbol_context"),
            (20, "find_references"),
            (30, "blast_radius"),
        ] {
            let arguments = if tool == "blast_radius" {
                json!({"symbol_id":symbol_id,"file_path":"src/workspace.rs"})
            } else {
                json!({"symbol_id":symbol_id})
            };
            let request = json!({"jsonrpc":"2.0","id":id + offset,"method":"tools/call","params":{"name":tool,"arguments":arguments}});
            let mut line = serde_json::to_string(&request).unwrap();
            line.push('\n');
            stdin.write_all(line.as_bytes()).unwrap();
            stdin.flush().unwrap();
            let mut response = String::new();
            reader.read_line(&mut response).unwrap();
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_ne!(response["result"]["isError"], true, "{response}");
            let text = response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string();
            if tool == "get_symbol_body" {
                overload_bodies.push(text);
            } else if tool == "get_symbol_context" {
                overload_contexts.push(text);
            } else if tool == "find_references" {
                overload_refs.push(text);
            } else if tool == "blast_radius" {
                overload_impacts.push(text);
            }
        }
    }
    assert_ne!(overload_bodies[0], overload_bodies[1]);
    assert_ne!(overload_contexts[0], overload_contexts[1]);
    assert!(overload_refs[0].contains("first") && !overload_refs[0].contains("second"));
    assert!(overload_refs[1].contains("second") && !overload_refs[1].contains("first"));
    assert!(overload_impacts[0].contains("first") && !overload_impacts[0].contains("second"));
    assert!(overload_impacts[1].contains("second") && !overload_impacts[1].contains("first"));

    let mismatch = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":52,"method":"tools/call","params":{"name":"blast_radius","arguments":{"symbol_id":"overload_zero","file_path":"src/other.cpp"}}}),
    );
    assert_eq!(mismatch["result"]["isError"], true);
    assert!(
        mismatch["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("not requested file")
    );

    let exact_lookup = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":53,"method":"tools/call","params":{"name":"lookup_symbol","arguments":{"query":"Workspace"}}}),
    );
    assert!(
        exact_lookup["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("id=s1")
    );
    let exact_search = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":54,"method":"tools/call","params":{"name":"search_symbols","arguments":{"query":"Workspace"}}}),
    );
    assert!(
        exact_search["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("id=s1")
    );

    // 11. Test telemetry_summary via MCP with time_window: "month"
    let telem_req = json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "tools/call",
        "params": {
            "name": "telemetry_summary",
            "arguments": {
                "time_window": "month"
            }
        }
    });
    let mut telem_line = serde_json::to_string(&telem_req).unwrap();
    telem_line.push('\n');
    stdin.write_all(telem_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line_telem = String::new();
    reader.read_line(&mut response_line_telem).unwrap();
    let resp_telem: Value =
        serde_json::from_str(&response_line_telem).expect("Failed to parse JSON response");
    assert_eq!(resp_telem["id"], 11);
    assert_ne!(resp_telem["result"]["isError"], true);
    let telem_text = resp_telem["result"]["content"][0]["text"].as_str().unwrap();
    assert!(telem_text.contains("Telemetry Summary"));

    let notification = json!({
        "jsonrpc": "2.0",
        "method": "notifications/roots/list_changed"
    });
    let mut notification_line = serde_json::to_string(&notification).unwrap();
    notification_line.push('\n');
    stdin.write_all(notification_line.as_bytes()).unwrap();

    let ping_req = json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "ping"
    });
    let mut ping_line = serde_json::to_string(&ping_req).unwrap();
    ping_line.push('\n');
    stdin.write_all(ping_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line12 = String::new();
    reader.read_line(&mut response_line12).unwrap();
    let resp12: Value =
        serde_json::from_str(&response_line12).expect("Failed to parse JSON response");
    assert_eq!(resp12["id"], 12);

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
            .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
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
            .env("CODE_KB_TELEMETRY_DIR", main_root.join(".telemetry_test"))
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
            "name": "lookup_symbol",
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
    conn.execute_batch(
        "CREATE TABLE artifact_metadata (
            key TEXT PRIMARY KEY, value TEXT NOT NULL
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO artifact_metadata VALUES ('root_path', ?1)",
        rusqlite::params![code_kb_core::to_forward_slash(&main_root)],
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
            .env("CODE_KB_TELEMETRY_DIR", main_root.join(".telemetry_test"))
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

    // Verify artifact_metadata root_path was retargeted to worktree
    {
        let wt_conn = code_kb_core::open_read_only(&wt_db).unwrap();
        let retargeted_root: String = wt_conn
            .query_row(
                "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            code_kb_core::workspace::paths_equal(std::path::Path::new(&retargeted_root), &wt_root),
            "artifact_metadata root_path must be retargeted to worktree root: got {retargeted_root}, expected {}",
            wt_root.display()
        );
    }

    drop(stdin);
    let _ = child.wait();
}

fn setup_test_repo() -> tempfile::TempDir {
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
    conn.execute(
        "INSERT INTO pending_relationships VALUES ('s1', 'println', 'call', 'src/workspace.rs', 2, 4)",
        [],
    )
    .unwrap();
    code_kb_core::db::ensure_fts_index(&conn).unwrap();
    drop(conn);

    temp_dir
}

#[test]
fn test_mcp_initialize_roots_file_uris() {
    use std::io::BufRead;

    let repo = setup_test_repo();
    let root = repo.path();
    let root_str = root.to_string_lossy().replace('\\', "/");
    let three_slash_uri = format!("file:///{root_str}");
    let two_slash_uri = format!("file://{root_str}");

    // 1. Spawn without --root, initialize with params.roots containing file:///C:/...
    {
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_code-kb"))
                .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
                .current_dir(root)
                .arg("serve")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("Failed to spawn code-kb serve without --root"),
        );
        let mut stdin = child.stdin.take().expect("Failed to open stdin");
        let stdout = child.stdout.take().expect("Failed to open stdout");
        let mut reader = std::io::BufReader::new(stdout);

        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "uri-test", "version": "1.0" },
                "roots": [{ "uri": three_slash_uri }]
            }
        });
        let mut line = serde_json::to_string(&init).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut init_resp = String::new();
        reader.read_line(&mut init_resp).unwrap();
        let resp: Value = serde_json::from_str(&init_resp).unwrap();
        assert_eq!(resp["id"], 1);

        let call = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lookup_symbol",
                "arguments": { "query": "Workspace" }
            }
        });
        let mut call_line = serde_json::to_string(&call).unwrap();
        call_line.push('\n');
        stdin.write_all(call_line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut call_resp = String::new();
        reader.read_line(&mut call_resp).unwrap();
        let call_val: Value = serde_json::from_str(&call_resp).unwrap();
        assert_eq!(call_val["id"], 2);
        assert_ne!(call_val["result"]["isError"], true);
        assert!(
            call_val["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Workspace")
        );

        drop(stdin);
        let _ = child.wait();
    }

    // 2. Spawn without --root, initialize with params.roots containing two-slash file://C:/...
    {
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_code-kb"))
                .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
                .current_dir(root)
                .arg("serve")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("Failed to spawn code-kb serve without --root"),
        );
        let mut stdin = child.stdin.take().expect("Failed to open stdin");
        let stdout = child.stdout.take().expect("Failed to open stdout");
        let mut reader = std::io::BufReader::new(stdout);

        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "uri-test", "version": "1.0" },
                "roots": [{ "uri": two_slash_uri }]
            }
        });
        let mut line = serde_json::to_string(&init).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut init_resp = String::new();
        reader.read_line(&mut init_resp).unwrap();
        let resp: Value = serde_json::from_str(&init_resp).unwrap();
        assert_eq!(resp["id"], 1);

        let call = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lookup_symbol",
                "arguments": { "query": "Workspace" }
            }
        });
        let mut call_line = serde_json::to_string(&call).unwrap();
        call_line.push('\n');
        stdin.write_all(call_line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut call_resp = String::new();
        reader.read_line(&mut call_resp).unwrap();
        let call_val: Value = serde_json::from_str(&call_resp).unwrap();
        assert_eq!(call_val["id"], 2);
        assert_ne!(call_val["result"]["isError"], true);
        assert!(
            call_val["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Workspace")
        );

        drop(stdin);
        let _ = child.wait();
    }
}

#[test]
fn test_mcp_rebinding_drive_casing_insensitivity() {
    use std::io::BufRead;

    let repo = setup_test_repo();
    let root = repo.path();

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
            .arg("serve")
            .arg("--root")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );
    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = std::io::BufReader::new(stdout);

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "casing-test", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut init_resp = String::new();
    reader.read_line(&mut init_resp).unwrap();
    let resp: Value = serde_json::from_str(&init_resp).unwrap();
    assert_eq!(resp["id"], 1);

    // 2. Prepare inverted drive letter path
    let abs_file = root
        .join("src")
        .join("workspace.rs")
        .to_string_lossy()
        .to_string();
    let inverted_abs_file = if abs_file.len() >= 2 && abs_file.as_bytes()[1] == b':' {
        let first_char = abs_file.chars().next().unwrap();
        let toggled = if first_char.is_ascii_uppercase() {
            first_char.to_ascii_lowercase()
        } else {
            first_char.to_ascii_uppercase()
        };
        format!("{}{}", toggled, &abs_file[1..])
    } else {
        abs_file.clone()
    };

    // 3. Call file_skeleton with inverted absolute path
    let call1 = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": inverted_abs_file }
        }
    });
    let mut call1_line = serde_json::to_string(&call1).unwrap();
    call1_line.push('\n');
    stdin.write_all(call1_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut resp_line1 = String::new();
    reader.read_line(&mut resp_line1).unwrap();
    let resp1: Value = serde_json::from_str(&resp_line1).unwrap();
    assert_eq!(resp1["id"], 2);
    assert_ne!(
        resp1["result"]["isError"], true,
        "file_skeleton should not error on inverted drive casing: {resp1:?}"
    );
    assert!(
        resp1["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("pub struct Workspace"),
        "file_skeleton should return symbol signatures"
    );

    // 4. Call get_symbol_body with inverted absolute path in file argument
    let call2 = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "get_symbol_body",
            "arguments": {
                "symbol": "Workspace",
                "file": inverted_abs_file
            }
        }
    });
    let mut call2_line = serde_json::to_string(&call2).unwrap();
    call2_line.push('\n');
    stdin.write_all(call2_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();
    let resp2: Value = serde_json::from_str(&resp_line2).unwrap();
    assert_eq!(resp2["id"], 3);
    assert_ne!(
        resp2["result"]["isError"], true,
        "get_symbol_body should not error on inverted drive casing: {resp2:?}"
    );
    assert!(
        resp2["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("pub struct Workspace"),
        "get_symbol_body should return symbol body"
    );

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_mcp_server_start_creates_missing_index() {
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"unindexed\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "pub fn unindexed_func() {}\n").unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_code-kb"));
    cmd.env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("serve")
        .arg("--root")
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = ChildGuard(cmd.spawn().expect("Failed to spawn code-kb mcp"));
    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("Failed to open stdout"));

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "test-client", "version": "1.0" },
            "capabilities": {}
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut response_line = String::new();
    reader.read_line(&mut response_line).unwrap();

    let db = root.join(".code-kb").join("artifact.db");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !db.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        db.exists(),
        "server start must create the index without a tool call"
    );

    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "lookup_symbol",
            "arguments": { "query": "unindexed_func" }
        }
    });
    let mut call_line = serde_json::to_string(&call_req).unwrap();
    call_line.push('\n');
    stdin.write_all(call_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut call_resp_line = String::new();
    reader.read_line(&mut call_resp_line).unwrap();
    let resp: Value = serde_json::from_str(&call_resp_line).expect("Failed to parse JSON response");
    assert_eq!(resp["id"], 2);
    assert_ne!(resp["result"]["isError"], true, "{resp}");
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("unindexed_func"), "{text}");

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_mcp_telemetry_summary_scoped_errors_no_cross_workspace_leak() {
    let telem_dir = code_kb_core::safe_tempdir();
    let telem_db_path = telem_dir.path().join("telemetry.db");
    let telem_conn = code_kb_core::Connection::open(&telem_db_path).unwrap();
    telem_conn
        .execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS tool_telemetry (
                 id TEXT PRIMARY KEY,
                 timestamp TEXT NOT NULL,
                 workspace_root TEXT NOT NULL,
                 workspace_name TEXT NOT NULL,
                 tool TEXT NOT NULL,
                 duration_ms INTEGER NOT NULL,
                 outcome TEXT NOT NULL,
                 error_message TEXT,
                 result_count INTEGER NOT NULL DEFAULT 0,
                 bytes_returned INTEGER NOT NULL DEFAULT 0,
                 est_tokens INTEGER NOT NULL DEFAULT 0,
                 est_tokens_saved INTEGER NOT NULL DEFAULT 0,
                 code_kb_version TEXT NOT NULL
             );",
        )
        .unwrap();

    // 1. Record error from unrelated workspace B
    let ws_b_dir = code_kb_core::safe_tempdir();
    let ws_b_root = code_kb_core::to_forward_slash(
        &code_kb_core::Workspace::new(ws_b_dir.path().to_path_buf()).canonical_root,
    );
    let secret_error = "SECRET_PATH_EXPOSURE: failed to parse /secret/unrelated/project/token.key";
    let current_version = env!("CARGO_PKG_VERSION");
    telem_conn
        .execute(
            "INSERT INTO tool_telemetry (
                id, timestamp, workspace_root, workspace_name, tool,
                duration_ms, outcome, error_message, result_count, bytes_returned,
                est_tokens, est_tokens_saved, code_kb_version
            ) VALUES (
                'err-b-1', datetime('now'), ?1, 'unrelated-repo', 'get_symbol_body',
                10, 'error', ?2, 0, 100, 25, 0, ?3
            )",
            rusqlite::params![ws_b_root, secret_error, current_version],
        )
        .unwrap();

    // 2. Set up workspace A
    let ws_a_dir = code_kb_core::safe_tempdir();
    let root = ws_a_dir.path().to_path_buf();
    let ws_a_root =
        code_kb_core::to_forward_slash(&code_kb_core::Workspace::new(root.clone()).canonical_root);
    let local_error = "Active repo local error: symbol MissingSymbol not found";
    telem_conn
        .execute(
            "INSERT INTO tool_telemetry (
                id, timestamp, workspace_root, workspace_name, tool,
                duration_ms, outcome, error_message, result_count, bytes_returned,
                est_tokens, est_tokens_saved, code_kb_version
            ) VALUES (
                'err-a-1', datetime('now'), ?1, 'active-repo', 'get_symbol_body',
                10, 'error', ?2, 0, 100, 25, 0, ?3
            )",
            rusqlite::params![ws_a_root, local_error, current_version],
        )
        .unwrap();
    drop(telem_conn);

    let db_dir = root.join(".code-kb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("artifact.db");

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let content = "// 40 bytes line of source code text!\n".repeat(10);
    let file_bytes = content.len() as i64;
    let hash = format!("blake3:{}", blake3::hash(content.as_bytes()).to_hex());
    std::fs::write(src_dir.join("lib.rs"), &content).unwrap();

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
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/lib.rs', 'rust', ?1, ?2, 10, '2026-01-01')",
        rusqlite::params![hash, file_bytes],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/lib.rs', 'rust', 'sample_func', 'function',
            'pub fn sample_func()', NULL, 'pub', NULL,
            1, 0, 2, 1, 0, ?1, 1, 20, 2, 1, 20, ?1, 'b3:hash',
            NULL, 0, 0
        )",
        rusqlite::params![file_bytes],
    )
    .unwrap();
    drop(conn);

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_code-kb"));
    cmd.arg("serve")
        .arg("--root")
        .arg(&root)
        .env("CODE_KB_TELEMETRY_DIR", telem_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = ChildGuard(cmd.spawn().expect("Failed to spawn code-kb serve"));
    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("Failed to open stdout"));

    // Handshake
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut init_resp_line = String::new();
    reader.read_line(&mut init_resp_line).unwrap();

    // 3. Call telemetry_summary with workspace_only: false (text output)
    let call_telem_text = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "telemetry_summary",
            "arguments": {
                "workspace_only": false
            }
        }
    });
    let mut telem_line = serde_json::to_string(&call_telem_text).unwrap();
    telem_line.push('\n');
    stdin.write_all(telem_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut telem_resp_line = String::new();
    reader.read_line(&mut telem_resp_line).unwrap();
    let resp: Value = serde_json::from_str(&telem_resp_line).unwrap();
    assert_eq!(resp["id"], 2);
    let summary_text = resp["result"]["content"][0]["text"].as_str().unwrap();

    // Verify global stats reflect total across both workspaces
    assert!(summary_text.contains("Total Tool Calls: 2"));
    // Verify active workspace error is included
    assert!(summary_text.contains(local_error));
    // Verify unrelated workspace secret error is NOT leaked
    assert!(
        !summary_text.contains(secret_error),
        "Global telemetry summary must not leak error messages from other workspaces"
    );

    // 4. Call telemetry_summary with workspace_only: false and json: true
    let call_telem_json = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "telemetry_summary",
            "arguments": {
                "workspace_only": false,
                "json": true
            }
        }
    });
    let mut json_line = serde_json::to_string(&call_telem_json).unwrap();
    json_line.push('\n');
    stdin.write_all(json_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut json_resp_line = String::new();
    reader.read_line(&mut json_resp_line).unwrap();
    let resp_json: Value = serde_json::from_str(&json_resp_line).unwrap();
    assert_eq!(resp_json["id"], 3);
    let summary_json_str = resp_json["result"]["content"][0]["text"].as_str().unwrap();
    let summary_data: Value = serde_json::from_str(summary_json_str).unwrap();

    // total_calls is 3: the 2 inserted errors + the previous telemetry_summary call
    assert_eq!(summary_data["total_calls"], 3);
    let recent_errors = summary_data["recent_errors"].as_array().unwrap();
    assert_eq!(recent_errors.len(), 1);
    assert_eq!(recent_errors[0]["error_message"], local_error);

    // 5. Test grounded token savings calculation for file_skeleton
    let skeleton_req = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": {
                "file_path": "src/lib.rs"
            }
        }
    });
    let mut skel_line = serde_json::to_string(&skeleton_req).unwrap();
    skel_line.push('\n');
    stdin.write_all(skel_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut skel_resp_line = String::new();
    reader.read_line(&mut skel_resp_line).unwrap();
    let resp_skel: Value = serde_json::from_str(&skel_resp_line).unwrap();
    assert_eq!(resp_skel["id"], 4);
    assert_ne!(resp_skel["result"]["isError"], true);
    let skel_text = resp_skel["result"]["content"][0]["text"].as_str().unwrap();
    let skel_tokens = skel_text.len() / 4;
    let expected_saved = ((file_bytes as usize) / 4).saturating_sub(skel_tokens);

    // Query telemetry for workspace_only to inspect the recorded est_tokens_saved
    let stats_req = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "telemetry_summary",
            "arguments": {
                "workspace_only": true,
                "json": true
            }
        }
    });
    let mut stats_line = serde_json::to_string(&stats_req).unwrap();
    stats_line.push('\n');
    stdin.write_all(stats_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut stats_resp_line = String::new();
    reader.read_line(&mut stats_resp_line).unwrap();
    let resp_stats: Value = serde_json::from_str(&stats_resp_line).unwrap();
    let stats_json: Value =
        serde_json::from_str(resp_stats["result"]["content"][0]["text"].as_str().unwrap()).unwrap();

    let skel_stat = stats_json["tool_stats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["tool"] == "file_skeleton")
        .expect("file_skeleton stat should be present");
    assert_eq!(skel_stat["tokens_saved"], expected_saved);

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_mcp_telemetry_summary_does_not_rebind_workspace() {
    use std::io::BufRead;

    let repo1 = setup_test_repo();
    let root1 = repo1.path();

    let repo2 = setup_test_repo();
    let root2 = repo2.path();

    let telem_dir = code_kb_core::safe_tempdir();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_code-kb"));
    cmd.env("CODE_KB_TELEMETRY_DIR", telem_dir.path());
    cmd.arg("serve")
        .arg("--root")
        .arg(root1)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = ChildGuard(cmd.spawn().expect("Failed to spawn code-kb serve"));
    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = std::io::BufReader::new(stdout);

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "rebind-test", "version": "1.0" }
        }
    });
    let mut init_line = serde_json::to_string(&init_req).unwrap();
    init_line.push('\n');
    stdin.write_all(init_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut init_resp_line = String::new();
    reader.read_line(&mut init_resp_line).unwrap();

    // 2. Call telemetry_summary with unexpected path pointing to repo2
    let stats_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "telemetry_summary",
            "arguments": {
                "workspace": root2.to_str().unwrap(),
                "file_path": root2.join("src/lib.rs").to_str().unwrap()
            }
        }
    });
    let mut stats_line = serde_json::to_string(&stats_req).unwrap();
    stats_line.push('\n');
    stdin.write_all(stats_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut stats_resp_line = String::new();
    reader.read_line(&mut stats_resp_line).unwrap();
    let stats_resp: Value = serde_json::from_str(&stats_resp_line).unwrap();
    assert_ne!(stats_resp["result"]["isError"], true);

    // 3. Call codebase_outline: should still be bound to root1, NOT root2
    let outline_req = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "codebase_outline",
            "arguments": {}
        }
    });
    let mut outline_line = serde_json::to_string(&outline_req).unwrap();
    outline_line.push('\n');
    stdin.write_all(outline_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut outline_resp_line = String::new();
    reader.read_line(&mut outline_resp_line).unwrap();
    let outline_resp: Value = serde_json::from_str(&outline_resp_line).unwrap();
    assert_ne!(outline_resp["result"]["isError"], true);

    // 4. Call telemetry_summary with invalid time_window: should return error
    let invalid_req = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "telemetry_summary",
            "arguments": {
                "time_window": "invalid_window_123"
            }
        }
    });
    let mut inv_line = serde_json::to_string(&invalid_req).unwrap();
    inv_line.push('\n');
    stdin.write_all(inv_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut inv_resp_line = String::new();
    reader.read_line(&mut inv_resp_line).unwrap();
    let inv_resp: Value = serde_json::from_str(&inv_resp_line).unwrap();
    assert_eq!(inv_resp["result"]["isError"], true);

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_mcp_lookup_and_search_symbols_accept_symbol_name_and_symbol_aliases() {
    let repo = setup_test_repo();
    let root = repo.path();

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
            .current_dir(root)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    // 1. initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "1.0" },
            "rootUri": format!("file://{}", root.display())
        }
    });
    let mut init_line = serde_json::to_string(&init_req).unwrap();
    init_line.push('\n');
    stdin.write_all(init_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut init_resp_line = String::new();
    reader.read_line(&mut init_resp_line).unwrap();
    let init_resp: Value = serde_json::from_str(&init_resp_line).unwrap();
    assert_eq!(init_resp["id"], 1);

    // 2. lookup_symbol using symbol_name alias
    let lookup_symbol_name = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "lookup_symbol",
            "arguments": {
                "symbol_name": "Workspace"
            }
        }
    });
    let mut line = serde_json::to_string(&lookup_symbol_name).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();
    let resp: Value = serde_json::from_str(&resp_line).unwrap();
    assert_ne!(resp["result"]["isError"], true);

    // 3. lookup_symbol using symbol alias
    let lookup_symbol = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "lookup_symbol",
            "arguments": {
                "symbol": "Workspace"
            }
        }
    });
    let mut line = serde_json::to_string(&lookup_symbol).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();
    let resp: Value = serde_json::from_str(&resp_line).unwrap();
    assert_ne!(resp["result"]["isError"], true);

    // 4. search_symbols using symbol_name alias
    let search_symbol_name = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "search_symbols",
            "arguments": {
                "symbol_name": "Workspace"
            }
        }
    });
    let mut line = serde_json::to_string(&search_symbol_name).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();
    let resp: Value = serde_json::from_str(&resp_line).unwrap();
    assert_ne!(resp["result"]["isError"], true);

    // 5. search_symbols using symbol alias
    let search_symbol = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "search_symbols",
            "arguments": {
                "symbol": "Workspace"
            }
        }
    });
    let mut line = serde_json::to_string(&search_symbol).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();
    let resp: Value = serde_json::from_str(&resp_line).unwrap();
    assert_ne!(resp["result"]["isError"], true);

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_first_tool_call_sees_files_changed_while_no_server_ran() {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".code-kb")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn indexed_by_scan() {}\n").unwrap();
    let workspace = code_kb_core::Workspace::new(root.clone());
    let db_path = root.join(".code-kb/artifact.db");
    code_kb_core::scan_workspace(&workspace, &db_path, false).unwrap();
    std::fs::write(
        root.join("src/offline.rs"),
        "pub fn added_while_no_server_ran() {}\n",
    )
    .unwrap();

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
            .arg("serve")
            .arg("--root")
            .arg(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut rpc = |request: Value| -> Value {
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    };

    rpc(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}
    }));
    let response = rpc(json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "lookup_symbol", "arguments": {"query": "added_while_no_server_ran"}}
    }));

    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("added_while_no_server_ran"), "{text}");
}

#[test]
fn test_mcp_lookup_symbol_reports_a_broken_search_index() {
    let repo = setup_test_repo();
    let root = repo.path();

    let conn = code_kb_core::open_read_write(&root.join(".code-kb").join("artifact.db")).unwrap();
    conn.execute_batch("DROP TABLE symbols_fts_data;").unwrap();
    drop(conn);

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
            .current_dir(root)
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    let mut rpc = |request: Value| -> Value {
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    };

    rpc(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}
    }));

    let response = rpc(json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "lookup_symbol", "arguments": {"query": "ZzzAbsentSymbol"}}
    }));
    assert_eq!(response["result"]["isError"], true);
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("symbols_fts"), "{text}");
    assert!(!text.contains("No exact name match"), "{text}");

    let summary = rpc(json!({
        "jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "telemetry_summary", "arguments": {"workspace_only": true, "json": true}}
    }));
    let summary_json: Value =
        serde_json::from_str(summary["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let recent_errors = summary_json["recent_errors"].as_array().unwrap();
    assert!(
        recent_errors
            .iter()
            .any(|entry| entry["tool"] == "lookup_symbol"
                && entry["error_message"]
                    .as_str()
                    .is_some_and(|message| message.contains("symbols_fts"))),
        "{summary_json}"
    );
}

fn reference_baseline_repo() -> tempfile::TempDir {
    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"baseline\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let padding = "x".repeat(80);
    let mut source = String::from("pub struct Alpha {\n    pub n: u32,\n}\n");
    for index in 0..40 {
        source.push_str(&format!(
            "\npub fn make_{index}(seed: u32) -> Alpha {{\n    let note = \"{padding}\";\n    let _ = note;\n    Alpha {{ n: seed + {index} }}\n}}\n"
        ));
    }
    std::fs::write(root.join("src").join("lib.rs"), &source).unwrap();
    temp_dir
}

#[test]
fn test_mcp_records_a_known_baseline_for_references_and_none_for_an_outline() {
    let repo = reference_baseline_repo();
    let root = repo.path();

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
            .arg("serve")
            .arg("--root")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());

    let mut call = |id: u64, request: Value| -> Value {
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();
        let mut response = String::new();
        reader.read_line(&mut response).unwrap();
        let parsed: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["id"], id, "{parsed}");
        parsed
    };

    call(
        1,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0" }
            }
        }),
    );

    let refs = call(
        2,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "find_references", "arguments": { "symbol_name": "Alpha" } }
        }),
    );
    assert_ne!(refs["result"]["isError"], true, "{refs}");

    let outline = call(
        3,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "codebase_outline", "arguments": {} }
        }),
    );
    assert_ne!(outline["result"]["isError"], true, "{outline}");

    let stats = call(
        4,
        json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "telemetry_summary",
                "arguments": { "workspace_only": true, "json": true }
            }
        }),
    );
    let summary: Value =
        serde_json::from_str(stats["result"]["content"][0]["text"].as_str().unwrap()).unwrap();

    let refs_stat = summary["tool_stats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["tool"] == "find_references")
        .expect("find_references stat should be present");
    assert_eq!(refs_stat["saved_known_count"], 1, "{refs_stat}");
    assert!(
        refs_stat["tokens_saved"].as_u64().unwrap() > 0,
        "{refs_stat}"
    );

    let outline_stat = summary["tool_stats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["tool"] == "codebase_outline")
        .expect("codebase_outline stat should be present");
    assert_eq!(outline_stat["saved_known_count"], 0, "{outline_stat}");
    assert_eq!(outline_stat["tokens_saved"], 0, "{outline_stat}");
    assert_eq!(summary["saved_known_calls"], 1, "{summary}");

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn mcp_read_tool_handlers_validate_selectors_and_preserve_git_discovery() {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path().to_path_buf();
    let src = root.join("src");
    let db_dir = root.join(".code-kb");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::write(
        src.join("workspace.rs"),
        "pub fn run_task() { helper(); }\nfn helper() {}\n",
    )
    .unwrap();
    let workspace = code_kb_core::Workspace::new(root.clone());
    code_kb_core::scan_workspace(&workspace, &db_dir.join("artifact.db"), true).unwrap();
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
            .arg("serve")
            .arg("--root")
            .arg(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);
    let initialized = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "selector-test", "version": "1.0" },
                "rootUri": format!("file://{}", root.display())
            }
        }),
    );
    assert_eq!(initialized["id"], 1);

    let lookup = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lookup_symbol","arguments":{"query":"run_task"}}}),
    );
    assert_ne!(lookup["result"]["isError"], true, "{lookup}");
    let lookup_text = lookup["result"]["content"][0]["text"].as_str().unwrap();
    let selected_id = lookup_text
        .lines()
        .find_map(|line| line.split_once("id=").map(|(_, id)| id))
        .unwrap_or_else(|| panic!("{lookup_text}"));
    assert!(!selected_id.is_empty());
    assert!(
        lookup_text
            .lines()
            .any(|line| { line.starts_with("- ") && line.contains(&format!("id={selected_id}")) })
    );
    let search = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"search_symbols","arguments":{"query":"run_task"}}}),
    );
    assert_ne!(search["result"]["isError"], true, "{search}");
    assert!(
        search["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(&format!("id={selected_id}"))
    );
    let selected_body = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"get_symbol_body","arguments":{"symbol_id":selected_id}}}),
    );
    assert_ne!(selected_body["result"]["isError"], true, "{selected_body}");
    for arguments in [
        json!({"symbol_name":"run_task","symbol_id":null}),
        json!({"symbol_name":"run_task","symbol":"run_task"}),
        json!({"symbol_id":selected_id,"symbol_name":null}),
    ] {
        let response = mcp_request(
            &mut stdin,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":17,"method":"tools/call","params":{"name":"get_symbol_body","arguments":arguments}}),
        );
        assert_ne!(response["result"]["isError"], true, "{response}");
    }

    let invalid_selectors = [
        (json!({}), "exactly one non-empty"),
        (json!({"symbol_name":""}), "must not be empty"),
        (json!({"symbol_id":""}), "must not be empty"),
        (json!({"symbol_name":7}), "must be a string"),
        (
            json!({"symbol_name":"run_task","symbol_id":"s2"}),
            "exactly one of",
        ),
        (
            json!({"symbol":"run_task","symbol_id":"s2"}),
            "exactly one of",
        ),
    ];
    let mut request_id = 3;
    for tool in ["get_symbol_body", "get_symbol_context", "find_references"] {
        for (arguments, error_text) in &invalid_selectors {
            let response = mcp_request(
                &mut stdin,
                &mut reader,
                &json!({"jsonrpc":"2.0","id":request_id,"method":"tools/call","params":{"name":tool,"arguments":arguments.clone()}}),
            );
            assert_eq!(response["result"]["isError"], true, "{tool}: {response}");
            assert!(
                response["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains(error_text)
            );
            request_id += 1;
        }
    }

    for (arguments, error_text) in [
        (json!({"symbol_id":""}), "must not be empty"),
        (json!({"symbol_id":7}), "must be a string"),
        (
            json!({"symbol":"run_task","symbol_id":"s2"}),
            "exactly one of",
        ),
    ] {
        let response = mcp_request(
            &mut stdin,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":request_id,"method":"tools/call","params":{"name":"blast_radius","arguments":arguments}}),
        );
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains(error_text)
        );
        request_id += 1;
    }

    std::fs::write(root.join(".code-kb/.gitignore"), "*\n").unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["add", "-A"])
            .current_dir(&root)
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
            .current_dir(&root)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(
        root.join("src/workspace.rs"),
        "pub struct Workspace { pub root: String }\npub fn changed() {}\n",
    )
    .unwrap();

    let discovery = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc":"2.0","id":request_id,"method":"tools/call","params":{"name":"blast_radius","arguments":{}}}),
    );
    assert_ne!(discovery["result"]["isError"], true, "{discovery}");
    assert!(
        discovery["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("src/workspace.rs")
    );
    for arguments in [
        json!({"symbol":null}),
        json!({"symbol_id":null}),
        json!({"symbol":null,"file":"src/workspace.rs"}),
    ] {
        let response = mcp_request(
            &mut stdin,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":request_id + 1,"method":"tools/call","params":{"name":"blast_radius","arguments":arguments}}),
        );
        assert_ne!(response["result"]["isError"], true, "{response}");
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("src/workspace.rs")
        );
    }

    drop(stdin);
    let _ = child.wait();
}
