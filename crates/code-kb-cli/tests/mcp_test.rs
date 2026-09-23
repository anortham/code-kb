use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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

const PROJECT_ROOT_DESCRIPTION: &str = "Absolute path of the project or git worktree you are working in. Send the same value on every call. Change it when you move to a worktree or another project.";
const MISSING_PROJECT_ROOT: &str = "Missing required parameter: project_root. Pass the absolute path of the project or git worktree you are working in.";

fn serve_command(launch_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_code-kb"));
    command
        .env("CODE_KB_TELEMETRY_DIR", launch_root.join(".telemetry_test"))
        .arg("serve")
        .arg("--root")
        .arg(launch_root);
    command
}

struct McpSession {
    _child: ChildGuard,
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
}

impl McpSession {
    fn start(mut command: Command) -> Self {
        let mut child = ChildGuard(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("Failed to spawn code-kb serve"),
        );
        let mut stdin = child.stdin.take().unwrap();
        let mut reader = BufReader::new(child.stdout.take().unwrap());
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
                    "clientInfo": { "name": "project-root-test", "version": "1.0" }
                }
            }),
        );
        assert_eq!(initialized["id"], 1);
        Self {
            _child: child,
            stdin,
            reader,
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        mcp_request(
            &mut self.stdin,
            &mut self.reader,
            &json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}),
        )
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone()
    }

    fn finish(self) {
        let Self {
            _child: mut child,
            stdin,
            ..
        } = self;
        drop(stdin);
        child.wait().unwrap();
    }
}

fn result_text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

fn launch_log(launch_root: &Path) -> String {
    std::fs::read_dir(launch_root.join(".code-kb").join("logs"))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect()
}

fn cargo_project(source: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("src").join("lib.rs"), source).unwrap();
    project
}

#[cfg(unix)]
fn extractor_wrapper(dir: &Path, scan_prelude: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let real = code_kb_core::find_julie_extract_binary().expect("julie-extract is installed");
    let wrapper = dir.join("julie-extract");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ \"$1\" = scan ]; then\n{scan_prelude}\nfi\nexec '{}' \"$@\"\n",
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while Command::new(&wrapper).arg("--version").output().is_err() {
        assert!(
            Instant::now() < deadline,
            "the wrapper never became runnable"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    wrapper
}

fn canonical_root(dir: &Path) -> PathBuf {
    code_kb_core::Workspace::new(dir.to_path_buf()).canonical_root
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
            .is_some_and(|instructions| instructions.starts_with("Pass project_root, the absolute path of the project or git worktree you work in, on every call except telemetry_summary. For progressive code exploration,"))
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
            "params": { "name": name, "arguments": { "project_root": root } }
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
            "arguments": { "project_root": root,
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
                "arguments": { "project_root": root, "query": "Workspace", "limit": limit }
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
            "arguments": { "project_root": root, "query": "Workspace::root", "limit": 0 }
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
            .is_some_and(|text| text.contains("limit is 0"))
    );

    let filtered_lookup_request = json!({
        "jsonrpc": "2.0",
        "id": 35,
        "method": "tools/call",
        "params": {
            "name": "lookup_symbol",
            "arguments": { "project_root": root, "query": "Workspace::root", "kind": "function" }
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
            "arguments": { "project_root": root,
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
            "arguments": { "project_root": root,
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
            "arguments": { "project_root": root }
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
            "arguments": { "project_root": root, "category": "route", "limit": 3 }
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

    for (id, name, arguments) in [
        (
            80,
            "find_structural_facts",
            json!({"project_root": root, "category": "route", "limit": 0}),
        ),
        (
            81,
            "lookup_symbol",
            json!({"project_root": root, "query": "Workspace", "limit": 0}),
        ),
        (
            82,
            "search_symbols",
            json!({"project_root": root, "query": "Workspace", "limit": 0}),
        ),
        (
            83,
            "find_references",
            json!({"project_root": root, "symbol_name": "Workspace", "limit": 0}),
        ),
    ] {
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        });
        let response = mcp_request(&mut stdin, &mut reader, &request);
        assert_eq!(response["id"], id);
        assert_ne!(response["result"]["isError"], true, "{name}: {response}");
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("limit is 0"), "{name}: {text}");
        assert!(!text.contains("No symbols found"), "{name}: {text}");
        assert!(!text.contains("No facts match"), "{name}: {text}");
        assert!(!text.contains("(none)"), "{name}: {text}");
    }

    // 7. Test file_skeleton with alias "file" instead of "file_path"
    let skeleton_req = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "project_root": root,
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
            "arguments": { "project_root": root,
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
            "arguments": { "project_root": root,
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
            "arguments": { "project_root": root,
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
        &json!({"jsonrpc":"2.0","id":55,"method":"tools/call","params":{"name":"get_symbol_context","arguments":{"project_root":root,"symbol_id":"s1","include_external":true}}}),
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
        &json!({"jsonrpc":"2.0","id":56,"method":"tools/call","params":{"name":"find_references","arguments":{"project_root":root,"symbol_id":"s1","direction":"callees"}}}),
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
        &json!({"jsonrpc":"2.0","id":57,"method":"tools/call","params":{"name":"find_references","arguments":{"project_root":root,"symbol_id":"s1","direction":"callees","include_external":true}}}),
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
                json!({"project_root":root,"symbol_id":symbol_id,"file_path":"src/workspace.rs"})
            } else {
                json!({"project_root":root,"symbol_id":symbol_id})
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
        &json!({"jsonrpc":"2.0","id":52,"method":"tools/call","params":{"name":"blast_radius","arguments":{"project_root":root,"symbol_id":"overload_zero","file_path":"src/other.cpp"}}}),
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
        &json!({"jsonrpc":"2.0","id":53,"method":"tools/call","params":{"name":"lookup_symbol","arguments":{"project_root":root,"query":"Workspace"}}}),
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
        &json!({"jsonrpc":"2.0","id":54,"method":"tools/call","params":{"name":"search_symbols","arguments":{"project_root":root,"query":"Workspace"}}}),
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
fn test_mcp_invalid_project_root_or_path_does_not_poison_later_calls() {
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
                "project_root": root,
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

    let invalid_root = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "file_skeleton",
                "arguments": {
                    "project_root": "/nonexistent_abs_path/nowhere",
                    "file": "src/lib.rs"
                }
            }
        }),
    );
    assert_eq!(invalid_root["result"]["isError"], true, "{invalid_root}");

    // 3. Subsequent call with valid relative path must STILL succeed (session not poisoned)
    let valid_call = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": {
                "project_root": root,
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
fn test_mcp_project_root_switches_between_main_checkout_and_worktree() {
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
            "arguments": { "project_root": main_root, "query": "main_fn" }
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
    let main_text = resp2["result"]["content"][0]["text"].as_str().unwrap();
    assert!(main_text.starts_with("Found 1 symbols matching \"main_fn\":"));
    assert!(main_text.contains("- function `main_fn` [src/main.rs:1-1]"));

    // 3. Query file_skeleton with absolute path to file in worktree
    let wt_call = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "project_root": wt_root, "file_path": wt_file.to_string_lossy().to_string() }
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

    let worktree_lookup = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "lookup_symbol",
                "arguments": { "project_root": wt_root, "query": "feature_fn" }
            }
        }),
    );
    assert_eq!(worktree_lookup["id"], 4);
    let worktree_text = worktree_lookup["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(worktree_text.starts_with("Found 1 symbols matching \"feature_fn\":"));
    assert!(worktree_text.contains("- function `feature_fn` [src/feature.rs:1-1]"));
    assert!(!worktree_text.contains("main_fn"));

    // 4. Query file_skeleton with absolute path back to main repo
    let back_call = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "project_root": main_root, "file": main_file.to_string_lossy().to_string() }
        }
    });
    let mut line4 = serde_json::to_string(&back_call).unwrap();
    line4.push('\n');
    stdin.write_all(line4.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line4 = String::new();
    reader.read_line(&mut resp_line4).unwrap();
    let resp4: Value = serde_json::from_str(&resp_line4).unwrap();
    assert_eq!(resp4["id"], 5);
    assert!(
        resp4["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("main_fn")
    );

    let main_lookup = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "lookup_symbol",
                "arguments": { "project_root": main_root, "query": "main_fn" }
            }
        }),
    );
    assert_eq!(main_lookup["id"], 6);
    let main_text = main_lookup["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(main_text.starts_with("Found 1 symbols matching \"main_fn\":"));
    assert!(main_text.contains("- function `main_fn` [src/main.rs:1-1]"));
    assert!(!main_text.contains("feature_fn"));

    let worktree_symbol_in_main = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "lookup_symbol",
                "arguments": { "project_root": main_root, "query": "feature_fn" }
            }
        }),
    );
    assert_ne!(
        worktree_symbol_in_main["result"]["isError"], true,
        "{worktree_symbol_in_main}"
    );
    assert!(
        !worktree_symbol_in_main["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("function `feature_fn` ["),
        "{worktree_symbol_in_main}"
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
            "arguments": { "project_root": wt_root, "file": wt_file.to_string_lossy().to_string() }
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

    let lookup = mcp_request(
        &mut stdin,
        &mut reader,
        &json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "lookup_symbol",
                "arguments": { "project_root": wt_root, "query": "shared_fn" }
            }
        }),
    );
    assert_eq!(lookup["id"], 3);
    assert_ne!(lookup["result"]["isError"], true, "{lookup}");
    let lookup_text = lookup["result"]["content"][0]["text"].as_str().unwrap();
    assert!(lookup_text.starts_with("Found 1 symbols matching \"shared_fn\":"));
    assert!(lookup_text.contains("- function `shared_fn` [src/main.rs:1-1]"));

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
    fixture_repo("Workspace")
}

fn fixture_repo(struct_name: &str) -> tempfile::TempDir {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let db_dir = root.join(".code-kb");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("artifact.db");

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let content = format!("pub struct {struct_name} {{\n    pub root: String,\n}}\n");
    std::fs::write(src_dir.join("workspace.rs"), &content).unwrap();
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
            's1', 'f1', 'src/workspace.rs', 'rust', ?2, 'struct',
            ?3, ?4, 'pub', NULL,
            1, 0, 3, 1, 0, ?1, 1, 21, 3, 1, 21, ?1, 'b3:hash',
            NULL, 0, 0
        )",
        rusqlite::params![
            bytes,
            struct_name,
            format!("pub struct {struct_name}"),
            format!("{struct_name} representation for code-kb workspace discovery root")
        ],
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
fn test_mcp_initialize_roots_are_ignored_and_file_uri_project_roots_resolve() {
    let repo = fixture_repo("Alpha");
    let root = repo.path();
    let bait = cargo_project("pub fn bait() {}\n");
    let root_str = root.to_string_lossy().replace('\\', "/");
    let three_slash_uri = format!("file:///{root_str}");
    let two_slash_uri = format!("file://{root_str}");

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
    let mut reader = BufReader::new(child.stdout.take().expect("Failed to open stdout"));

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
                "clientInfo": { "name": "uri-test", "version": "1.0" },
                "roots": [{ "uri": file_uri(bait.path()) }]
            }
        }),
    );
    assert_eq!(initialized["id"], 1);

    for (id, project_root) in [(2, three_slash_uri), (3, two_slash_uri)] {
        let call_val = mcp_request(
            &mut stdin,
            &mut reader,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "lookup_symbol",
                    "arguments": { "project_root": project_root, "query": "Alpha" }
                }
            }),
        );
        assert_eq!(call_val["id"], id);
        assert_ne!(call_val["result"]["isError"], true, "{call_val}");
        assert!(
            result_text(&call_val["result"]).contains("struct `Alpha` ["),
            "{call_val}"
        );
    }
    assert!(!bait.path().join(".code-kb").exists());

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_mcp_project_root_drive_casing_and_trailing_slash_variants_reach_the_same_index() {
    let repo = setup_test_repo();
    let root = repo.path();
    let launch = fixture_repo("Alpha");
    let mut session = McpSession::start(serve_command(launch.path()));

    let toggle_drive_letter = |path: String| -> String {
        if path.len() >= 2 && path.as_bytes()[1] == b':' {
            let first_char = path.chars().next().unwrap();
            let toggled = if first_char.is_ascii_uppercase() {
                first_char.to_ascii_lowercase()
            } else {
                first_char.to_ascii_uppercase()
            };
            format!("{}{}", toggled, &path[1..])
        } else {
            path
        }
    };
    let inverted_root = toggle_drive_letter(root.to_string_lossy().to_string());
    let trailing_slash_root = format!("{}/", root.display());
    let inverted_abs_file = toggle_drive_letter(
        root.join("src")
            .join("workspace.rs")
            .to_string_lossy()
            .to_string(),
    );

    let skeleton = session.call(
        "file_skeleton",
        json!({ "project_root": inverted_root, "file": inverted_abs_file }),
    );
    assert_ne!(
        skeleton["isError"], true,
        "file_skeleton should not error on inverted drive casing: {skeleton:?}"
    );
    assert!(
        result_text(&skeleton).contains("pub struct Workspace"),
        "file_skeleton should return symbol signatures"
    );

    let body = session.call(
        "get_symbol_body",
        json!({
            "project_root": trailing_slash_root,
            "symbol": "Workspace",
            "file": inverted_abs_file
        }),
    );
    assert_ne!(
        body["isError"], true,
        "get_symbol_body should not error on a trailing slash or inverted drive casing: {body:?}"
    );
    assert!(
        result_text(&body).contains("pub struct Workspace"),
        "get_symbol_body should return symbol body"
    );
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
        .env("CODE_KB_INDEX_WAIT_MS", "60000")
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
            "arguments": { "project_root": root, "query": "unindexed_func" }
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
                "project_root": root,
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
fn test_mcp_telemetry_summary_does_not_switch_the_active_root_or_create_an_index() {
    let repo1 = setup_test_repo();
    let root1 = repo1.path();

    let repo2 = tempfile::tempdir().unwrap();
    let root2 = repo2.path();
    std::fs::write(
        root2.join("Cargo.toml"),
        "[package]\nname = \"unindexed\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root2.join("src")).unwrap();
    std::fs::write(root2.join("src").join("lib.rs"), "pub fn other() {}\n").unwrap();

    let telem_dir = code_kb_core::safe_tempdir();
    let mut command = serve_command(root1);
    command.env("CODE_KB_TELEMETRY_DIR", telem_dir.path());
    let mut session = McpSession::start(command);

    let lookup = session.call(
        "lookup_symbol",
        json!({ "project_root": root1, "query": "Workspace" }),
    );
    assert_ne!(lookup["isError"], true, "{lookup}");

    let stats = session.call(
        "telemetry_summary",
        json!({
            "project_root": root2,
            "workspace": root2,
            "file_path": root2.join("src").join("lib.rs")
        }),
    );
    assert_ne!(stats["isError"], true, "{stats}");

    let scoped = session.call(
        "telemetry_summary",
        json!({ "workspace_only": true, "json": true }),
    );
    let scoped_json: Value = serde_json::from_str(result_text(&scoped)).unwrap();
    assert!(
        scoped_json["tool_stats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stat| stat["tool"] == "lookup_symbol"),
        "{scoped_json}"
    );
    assert!(!root2.join(".code-kb").exists());

    let outline = session.call("codebase_outline", json!({ "project_root": root1 }));
    assert_ne!(outline["isError"], true, "{outline}");

    let invalid = session.call(
        "telemetry_summary",
        json!({ "time_window": "invalid_window_123" }),
    );
    assert_eq!(invalid["isError"], true, "{invalid}");
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
            "arguments": { "project_root": root,
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
            "arguments": { "project_root": root,
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
            "arguments": { "project_root": root,
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
            "arguments": { "project_root": root,
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
            .env("CODE_KB_INDEX_WAIT_MS", "60000")
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
        "params": {"name": "lookup_symbol", "arguments": {"project_root": root, "query": "added_while_no_server_ran"}}
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
        "params": {"name": "lookup_symbol", "arguments": {"project_root": root, "query": "ZzzAbsentSymbol"}}
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
            .env("CODE_KB_INDEX_WAIT_MS", "60000")
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
            "params": { "name": "find_references", "arguments": { "project_root": root, "symbol_name": "Alpha" } }
        }),
    );
    assert_ne!(refs["result"]["isError"], true, "{refs}");

    let outline = call(
        3,
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": { "name": "codebase_outline", "arguments": { "project_root": root } }
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
        &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lookup_symbol","arguments":{"project_root":root,"query":"run_task"}}}),
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
        &json!({"jsonrpc":"2.0","id":15,"method":"tools/call","params":{"name":"search_symbols","arguments":{"project_root":root,"query":"run_task"}}}),
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
        &json!({"jsonrpc":"2.0","id":16,"method":"tools/call","params":{"name":"get_symbol_body","arguments":{"project_root":root,"symbol_id":selected_id}}}),
    );
    assert_ne!(selected_body["result"]["isError"], true, "{selected_body}");
    for arguments in [
        json!({"project_root":root,"symbol_name":"run_task","symbol_id":null}),
        json!({"project_root":root,"symbol_name":"run_task","symbol":"run_task"}),
        json!({"project_root":root,"symbol_id":selected_id,"symbol_name":null}),
    ] {
        let response = mcp_request(
            &mut stdin,
            &mut reader,
            &json!({"jsonrpc":"2.0","id":17,"method":"tools/call","params":{"name":"get_symbol_body","arguments":arguments}}),
        );
        assert_ne!(response["result"]["isError"], true, "{response}");
    }

    let invalid_selectors = [
        (json!({"project_root":root}), "exactly one non-empty"),
        (
            json!({"project_root":root,"symbol_name":""}),
            "must not be empty",
        ),
        (
            json!({"project_root":root,"symbol_id":""}),
            "must not be empty",
        ),
        (
            json!({"project_root":root,"symbol_name":7}),
            "must be a string",
        ),
        (
            json!({"project_root":root,"symbol_name":"run_task","symbol_id":"s2"}),
            "exactly one of",
        ),
        (
            json!({"project_root":root,"symbol":"run_task","symbol_id":"s2"}),
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
        (
            json!({"project_root":root,"symbol_id":""}),
            "must not be empty",
        ),
        (
            json!({"project_root":root,"symbol_id":7}),
            "must be a string",
        ),
        (
            json!({"project_root":root,"symbol":"run_task","symbol_id":"s2"}),
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
        &json!({"jsonrpc":"2.0","id":request_id,"method":"tools/call","params":{"name":"blast_radius","arguments":{"project_root":root}}}),
    );
    assert_ne!(discovery["result"]["isError"], true, "{discovery}");
    assert!(
        discovery["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("src/workspace.rs")
    );
    for arguments in [
        json!({"project_root":root,"symbol":null}),
        json!({"project_root":root,"symbol_id":null}),
        json!({"project_root":root,"symbol":null,"file":"src/workspace.rs"}),
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

fn file_uri(dir: &Path) -> String {
    let path = dir.to_string_lossy().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

#[test]
fn test_mcp_tools_list_requires_project_root_on_every_tool_except_telemetry_summary() {
    let repo = setup_test_repo();
    let mut session = McpSession::start(serve_command(repo.path()));

    let listed = session.request("tools/list", json!({}));
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 10);
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if name == "telemetry_summary" {
            assert!(schema["properties"].get("project_root").is_none(), "{tool}");
            assert!(!required.contains(&"project_root"), "{tool}");
        } else {
            assert_eq!(
                schema["properties"]["project_root"]["type"], "string",
                "{tool}"
            );
            assert_eq!(
                schema["properties"]["project_root"]["description"], PROJECT_ROOT_DESCRIPTION,
                "{tool}"
            );
            assert!(required.contains(&"project_root"), "{tool}");
        }
        for forbidden in ["workspace", "workspace_id", "repo_path", "root_dir"] {
            assert!(
                schema["properties"].get(forbidden).is_none(),
                "{name} exposes {forbidden}"
            );
        }
    }
}

#[test]
fn test_mcp_call_without_project_root_asks_for_the_absolute_project_path() {
    let repo = setup_test_repo();
    let mut session = McpSession::start(serve_command(repo.path()));

    for arguments in [
        json!({"query": "Workspace"}),
        json!({"query": "Workspace", "project_root": "  "}),
    ] {
        let result = session.call("lookup_symbol", arguments);
        assert_eq!(result["isError"], true, "{result}");
        assert_eq!(result_text(&result), MISSING_PROJECT_ROOT);
    }
}

#[test]
fn test_mcp_relative_project_root_is_an_error() {
    let repo = setup_test_repo();
    let mut session = McpSession::start(serve_command(repo.path()));

    let result = session.call(
        "lookup_symbol",
        json!({"query": "Workspace", "project_root": "src"}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(
        result_text(&result),
        "project_root 'src' is a relative path. Pass the absolute path of the project or git worktree you are working in as project_root."
    );
}

#[test]
fn test_mcp_project_root_subfolder_answers_from_the_enclosing_project() {
    let launch = fixture_repo("Alpha");
    let target = fixture_repo("Beta");
    let mut session = McpSession::start(serve_command(launch.path()));

    let result = session.call(
        "lookup_symbol",
        json!({"query": "Beta", "project_root": target.path().join("src")}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert!(result_text(&result).contains("struct `Beta` ["), "{result}");
}

#[test]
fn test_mcp_file_uri_project_root_answers_from_that_project() {
    let launch = fixture_repo("Alpha");
    let target = fixture_repo("Beta");
    let mut session = McpSession::start(serve_command(launch.path()));

    let result = session.call(
        "lookup_symbol",
        json!({"query": "Beta", "project_root": file_uri(target.path())}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert!(result_text(&result).contains("struct `Beta` ["), "{result}");
}

#[test]
fn test_mcp_unindexed_project_root_starts_indexing_and_answers_later() {
    let launch = setup_test_repo();
    let target = tempfile::tempdir().unwrap();
    std::fs::write(
        target.path().join("Cargo.toml"),
        "[package]\nname = \"fresh\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(target.path().join("src")).unwrap();
    std::fs::write(
        target.path().join("src").join("lib.rs"),
        "pub fn freshly_indexed() {}\n",
    )
    .unwrap();
    let mut command = serve_command(launch.path());
    command.env("CODE_KB_INDEX_WAIT_MS", "0");
    let mut session = McpSession::start(command);
    let arguments = json!({"query": "freshly_indexed", "project_root": target.path()});

    let first = session.call("lookup_symbol", arguments.clone());
    assert_ne!(first["isError"], true, "{first}");
    assert_eq!(
        result_text(&first),
        format!(
            "Indexing {} started; call again in a few seconds.",
            canonical_root(target.path()).display()
        )
    );

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let later = session.call("lookup_symbol", arguments.clone());
        if result_text(&later).contains("function `freshly_indexed` [") {
            break;
        }
        assert!(Instant::now() < deadline, "{later}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn test_mcp_filesystem_root_project_root_is_refused() {
    let launch = setup_test_repo();
    let filesystem_root = launch.path().ancestors().last().unwrap().to_path_buf();
    let mut session = McpSession::start(serve_command(launch.path()));

    let result = session.call("codebase_outline", json!({"project_root": filesystem_root}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result_text(&result).contains("is refused: it is a filesystem root"),
        "{result}"
    );
}

#[test]
fn test_mcp_home_directory_project_root_is_refused_without_creating_an_index() {
    let launch = setup_test_repo();
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("package.json"), "{}\n").unwrap();
    let mut command = serve_command(launch.path());
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path());
    let mut session = McpSession::start(command);

    let result = session.call("codebase_outline", json!({"project_root": home.path()}));
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result_text(&result).contains("is refused: it is the home directory"),
        "{result}"
    );
    assert!(!home.path().join(".code-kb").exists());
}

#[test]
fn test_mcp_serve_launched_in_the_home_directory_does_not_index_it() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("package.json"), "{}\n").unwrap();
    let project = setup_test_repo();
    let telemetry = tempfile::tempdir().unwrap();
    let mut command = serve_command(home.path());
    command
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CODE_KB_TELEMETRY_DIR", telemetry.path())
        .env_remove("RUST_LOG");
    let mut session = McpSession::start(command);

    let result = session.call(
        "lookup_symbol",
        json!({"query": "Workspace", "project_root": project.path()}),
    );
    assert!(
        result_text(&result).contains("struct `Workspace` ["),
        "{result}"
    );
    session.finish();

    let log = launch_log(home.path());
    assert!(log.contains("Startup index skipped"), "{log}");
    assert!(!log.contains("running automatic initial scan"), "{log}");
    assert!(!home.path().join(".code-kb").join("artifact.db").exists());
}

#[test]
fn test_mcp_absolute_path_outside_project_root_is_an_error_that_changes_nothing() {
    let launch = fixture_repo("Alpha");
    let other = fixture_repo("Beta");
    let mut session = McpSession::start(serve_command(launch.path()));
    let outside = other
        .path()
        .join("src")
        .join("workspace.rs")
        .to_string_lossy()
        .to_string();

    let refused = session.call(
        "file_skeleton",
        json!({"file_path": outside, "project_root": launch.path()}),
    );
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(
        result_text(&refused),
        format!(
            "Path '{outside}' is outside project_root '{}'. Pass a path inside project_root, or change project_root to the project that holds the path.",
            canonical_root(launch.path()).display()
        )
    );

    let next = session.call(
        "lookup_symbol",
        json!({"query": "Alpha", "project_root": launch.path()}),
    );
    assert!(result_text(&next).contains("struct `Alpha` ["), "{next}");
}

#[test]
fn test_mcp_pinned_db_serves_only_the_launch_root() {
    let launch = fixture_repo("Alpha");
    let other = fixture_repo("Beta");
    let pin_dir = tempfile::tempdir().unwrap();
    let pinned = pin_dir.path().join("pinned.db");
    std::fs::copy(launch.path().join(".code-kb").join("artifact.db"), &pinned).unwrap();
    let mut command = serve_command(launch.path());
    command.arg("--db").arg(&pinned);
    let mut session = McpSession::start(command);

    let launch_answer = session.call(
        "lookup_symbol",
        json!({"query": "Alpha", "project_root": launch.path()}),
    );
    assert!(
        result_text(&launch_answer).contains("struct `Alpha` ["),
        "{launch_answer}"
    );
    let pinned_bytes = std::fs::read(&pinned).unwrap();

    let other_answer = session.call(
        "lookup_symbol",
        json!({"query": "Beta", "project_root": other.path()}),
    );
    assert!(
        result_text(&other_answer).contains("struct `Beta` ["),
        "{other_answer}"
    );
    assert_eq!(std::fs::read(&pinned).unwrap(), pinned_bytes);
}

#[test]
fn test_mcp_symbol_id_from_lookup_resolves_in_get_symbol_body_for_the_same_project_root() {
    let launch = fixture_repo("Alpha");
    let target = fixture_repo("Beta");
    let mut session = McpSession::start(serve_command(launch.path()));

    let lookup = session.call(
        "lookup_symbol",
        json!({"query": "Beta", "project_root": target.path()}),
    );
    let lookup_text = result_text(&lookup);
    let symbol_id = lookup_text
        .lines()
        .find_map(|line| line.split_once("id=").map(|(_, id)| id.trim().to_string()))
        .unwrap_or_else(|| panic!("{lookup_text}"));

    let body = session.call(
        "get_symbol_body",
        json!({"symbol_id": symbol_id, "project_root": target.path()}),
    );
    assert_ne!(body["isError"], true, "{body}");
    assert!(result_text(&body).contains("pub struct Beta"), "{body}");
}

#[test]
fn test_mcp_telemetry_summary_needs_no_project_root() {
    let repo = setup_test_repo();
    let mut session = McpSession::start(serve_command(repo.path()));

    let result = session.call("telemetry_summary", json!({}));
    assert_ne!(result["isError"], true, "{result}");
    assert!(
        result_text(&result).contains("Telemetry Summary"),
        "{result}"
    );
}

#[test]
fn test_mcp_telemetry_records_the_launch_root_for_a_refused_call_and_the_resolved_root_otherwise() {
    let launch = fixture_repo("Alpha");
    let target = fixture_repo("Beta");
    let telemetry = tempfile::tempdir().unwrap();
    let mut command = serve_command(launch.path());
    command.env("CODE_KB_TELEMETRY_DIR", telemetry.path());
    let mut session = McpSession::start(command);

    session.call(
        "lookup_symbol",
        json!({"query": "Beta", "project_root": target.path()}),
    );
    session.call("lookup_symbol", json!({"query": "Beta"}));

    let conn = code_kb_core::Connection::open(telemetry.path().join("telemetry.db")).unwrap();
    let rows = conn
        .prepare(
            "SELECT outcome, workspace_root FROM tool_telemetry
             WHERE tool = 'lookup_symbol' ORDER BY rowid",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![
            (
                "ok".to_string(),
                code_kb_core::to_forward_slash(&canonical_root(target.path()))
            ),
            (
                "error".to_string(),
                code_kb_core::to_forward_slash(&canonical_root(launch.path()))
            ),
        ]
    );
}

fn markerless_folder(source: &str) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(folder.path().join("src")).unwrap();
    std::fs::write(folder.path().join("src").join("lib.rs"), source).unwrap();
    folder
}

fn cli_scan(root: &Path, db: Option<&Path>, telemetry_dir: &Path) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_code-kb"));
    command
        .env("CODE_KB_TELEMETRY_DIR", telemetry_dir)
        .arg("--root")
        .arg(root);
    if let Some(db) = db {
        command.arg("--db").arg(db);
    }
    let scanned = command.arg("scan").output().unwrap();
    assert!(scanned.status.success(), "{scanned:?}");
}

#[test]
fn test_mcp_pinned_db_is_the_index_of_a_markerless_launch_root() {
    let folder = markerless_folder("pub fn pinned_only() {}\n");
    let telemetry = tempfile::tempdir().unwrap();
    let pin_dir = tempfile::tempdir().unwrap();
    let pinned = pin_dir.path().join("pinned.db");
    cli_scan(folder.path(), Some(&pinned), telemetry.path());
    let mut command = serve_command(folder.path());
    command
        .env("CODE_KB_TELEMETRY_DIR", telemetry.path())
        .arg("--db")
        .arg(&pinned);
    let mut session = McpSession::start(command);

    let result = session.call(
        "lookup_symbol",
        json!({"query": "pinned_only", "project_root": folder.path()}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert!(
        result_text(&result).contains("function `pinned_only` ["),
        "{result}"
    );
    assert!(!folder.path().join(".code-kb").join("artifact.db").exists());
}

#[test]
fn test_mcp_markerless_launch_root_without_a_pinned_db_is_refused() {
    let folder = markerless_folder("pub fn unpinned() {}\n");
    let telemetry = tempfile::tempdir().unwrap();
    let mut command = serve_command(folder.path());
    command.env("CODE_KB_TELEMETRY_DIR", telemetry.path());
    let mut session = McpSession::start(command);

    let result = session.call(
        "lookup_symbol",
        json!({"query": "unpinned", "project_root": folder.path()}),
    );
    assert_eq!(result["isError"], true, "{result}");
    assert!(
        result_text(&result).contains(code_kb_core::workspace::NO_PROJECT_MARKER_REASON),
        "{result}"
    );
}

#[test]
fn test_mcp_launch_root_indexed_during_the_session_is_reconciled_on_first_use() {
    let folder = markerless_folder("pub fn indexed_by_cli() {}\n");
    let telemetry = tempfile::tempdir().unwrap();
    let mut command = serve_command(folder.path());
    command
        .env("CODE_KB_TELEMETRY_DIR", telemetry.path())
        .env("CODE_KB_INDEX_WAIT_MS", "60000");
    let mut session = McpSession::start(command);
    cli_scan(folder.path(), None, telemetry.path());
    std::fs::write(
        folder.path().join("src").join("offline.rs"),
        "pub fn added_after_the_scan() {}\n",
    )
    .unwrap();

    let result = session.call(
        "lookup_symbol",
        json!({"query": "added_after_the_scan", "project_root": folder.path()}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert!(
        result_text(&result).contains("function `added_after_the_scan` ["),
        "{result}"
    );
}

#[test]
fn test_mcp_absolute_path_in_a_nested_worktree_is_refused_without_indexing_it() {
    let main = fixture_repo("Alpha");
    let worktree = main.path().join(".claude").join("worktrees").join("x");
    std::fs::create_dir_all(worktree.join("src")).unwrap();
    std::fs::write(
        worktree.join(".git"),
        "gitdir: /nonexistent/main/.git/worktrees/x\n",
    )
    .unwrap();
    std::fs::write(
        worktree.join("src").join("a.rs"),
        "pub fn worktree_only() {}\n",
    )
    .unwrap();
    let mut session = McpSession::start(serve_command(main.path()));
    let nested = canonical_root(&worktree);
    let root = canonical_root(main.path());
    let worktree_file = worktree
        .join("src")
        .join("a.rs")
        .to_string_lossy()
        .to_string();
    let worktree_src = worktree.join("src").to_string_lossy().to_string();

    for (tool, path, arguments) in [
        (
            "file_skeleton",
            &worktree_file,
            json!({"project_root": main.path(), "file_path": worktree_file}),
        ),
        (
            "lookup_symbol",
            &worktree_src,
            json!({"project_root": main.path(), "query": "worktree_only", "path": worktree_src}),
        ),
    ] {
        let result = session.call(tool, arguments);
        assert_eq!(result["isError"], true, "{tool}: {result}");
        assert_eq!(
            result_text(&result),
            format!(
                "Path '{path}' belongs to the nested project '{}', not to project_root '{}'. Pass '{}' as project_root.",
                nested.display(),
                root.display(),
                nested.display()
            ),
            "{tool}"
        );
    }

    let conn =
        code_kb_core::open_read_only(&main.path().join(".code-kb").join("artifact.db")).unwrap();
    let nested_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM files WHERE path LIKE '.claude/%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(nested_rows, 0);
}

#[test]
fn test_mcp_file_written_after_an_unwaited_prepare_reaches_the_index() {
    let launch = setup_test_repo();
    let target = cargo_project("pub fn first_indexed() {}\n");
    let mut command = serve_command(launch.path());
    command.env("CODE_KB_INDEX_WAIT_MS", "0");
    let mut session = McpSession::start(command);
    let arguments = json!({"query": "written_later", "project_root": target.path()});

    let first = session.call("lookup_symbol", arguments.clone());
    assert!(result_text(&first).starts_with("Indexing "), "{first}");
    let db = target.path().join(".code-kb").join("artifact.db");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !db.exists() {
        assert!(Instant::now() < deadline, "the index was never created");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_secs(3));
    std::fs::write(
        target.path().join("src").join("later.rs"),
        "pub fn written_later() {}\n",
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let later = session.call("lookup_symbol", arguments.clone());
        if result_text(&later).contains("function `written_later` [") {
            break;
        }
        assert!(Instant::now() < deadline, "{later}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[test]
fn test_mcp_indexing_answer_records_no_token_savings() {
    let launch = setup_test_repo();
    let target =
        cargo_project(&"// a long source line that makes the file a large baseline\n".repeat(200));
    let telemetry = tempfile::tempdir().unwrap();
    let mut command = serve_command(launch.path());
    command
        .env("CODE_KB_TELEMETRY_DIR", telemetry.path())
        .env("CODE_KB_INDEX_WAIT_MS", "0");
    let mut session = McpSession::start(command);

    let result = session.call(
        "file_skeleton",
        json!({"project_root": target.path(), "file_path": "src/lib.rs"}),
    );
    assert!(result_text(&result).starts_with("Indexing "), "{result}");

    let conn = code_kb_core::Connection::open(telemetry.path().join("telemetry.db")).unwrap();
    let savings = conn
        .query_row(
            "SELECT est_tokens_saved, est_tokens_saved_known FROM tool_telemetry
             WHERE tool = 'file_skeleton'",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap();
    assert_eq!(savings, (0, 0));
}

#[cfg(unix)]
#[test]
fn test_mcp_failed_initial_scan_is_retried_on_the_next_call() {
    let launch = setup_test_repo();
    let target = cargo_project("pub fn scanned_on_retry() {}\n");
    let tools = tempfile::tempdir().unwrap();
    let marker = tools.path().join("failed-once");
    let wrapper = extractor_wrapper(
        tools.path(),
        &format!(
            "if [ ! -e '{marker}' ]; then : > '{marker}'; echo 'simulated scan failure' >&2; exit 2; fi",
            marker = marker.display()
        ),
    );
    let mut command = serve_command(launch.path());
    command
        .env("JULIE_EXTRACT_BIN", &wrapper)
        .env("CODE_KB_INDEX_WAIT_MS", "60000");
    let mut session = McpSession::start(command);
    let arguments = json!({"query": "scanned_on_retry", "project_root": target.path()});

    let first = session.call("lookup_symbol", arguments.clone());
    assert_eq!(first["isError"], true, "{first}");
    let first_text = result_text(&first);
    assert!(
        first_text.starts_with(&format!(
            "Initial scan of '{}' failed: ",
            canonical_root(target.path()).display()
        )),
        "{first_text}"
    );
    assert!(
        first_text.contains("simulated scan failure"),
        "{first_text}"
    );
    assert!(
        first_text.ends_with("; it will be retried on the next tool call."),
        "{first_text}"
    );

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let later = session.call("lookup_symbol", arguments.clone());
        if result_text(&later).contains("function `scanned_on_retry` [") {
            break;
        }
        assert!(Instant::now() < deadline, "{later}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[cfg(unix)]
#[test]
fn test_mcp_switching_back_reuses_the_running_prepare() {
    let launch = setup_test_repo();
    let target = cargo_project("pub fn slow_scan() {}\n");
    let tools = tempfile::tempdir().unwrap();
    let wrapper = extractor_wrapper(tools.path(), "sleep 1");
    let mut command = serve_command(launch.path());
    command
        .env("JULIE_EXTRACT_BIN", &wrapper)
        .env("CODE_KB_INDEX_WAIT_MS", "0")
        .env_remove("RUST_LOG");
    let mut session = McpSession::start(command);
    let arguments = json!({"query": "slow_scan", "project_root": target.path()});

    let first = session.call("lookup_symbol", arguments.clone());
    assert!(result_text(&first).starts_with("Indexing "), "{first}");
    session.call(
        "lookup_symbol",
        json!({"query": "Workspace", "project_root": launch.path()}),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let later = session.call("lookup_symbol", arguments.clone());
        if result_text(&later).contains("function `slow_scan` [") {
            break;
        }
        assert!(Instant::now() < deadline, "{later}");
        std::thread::sleep(Duration::from_millis(100));
    }
    session.finish();

    let target_root = canonical_root(target.path()).display().to_string();
    let log = launch_log(launch.path());
    let initial_scans = log
        .lines()
        .filter(|line| {
            line.contains("running automatic initial scan") && line.contains(&target_root)
        })
        .count();
    assert_eq!(initial_scans, 1, "{log}");
}
