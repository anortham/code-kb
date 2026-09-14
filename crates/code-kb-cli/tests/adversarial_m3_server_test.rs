use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
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

fn setup_fixture_repo() -> (tempfile::TempDir, PathBuf, String) {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let code = "pub fn compute_sum(a: i32, b: i32) -> i32 {\n    a + b\n}\n";
    let file_path = src_dir.join("calc.rs");
    fs::write(&file_path, code).unwrap();

    let scan_out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(&root)
        .arg("scan")
        .output()
        .expect("Failed to initialize test repo via scan");
    assert!(
        scan_out.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&scan_out.stderr)
    );

    let db_path = root.join(".code-kb").join("artifact.db");
    (temp_dir, db_path, code.to_string())
}

fn toggle_drive_letter(path_str: &str) -> String {
    if path_str.len() >= 2 && path_str.as_bytes()[1] == b':' {
        let first = path_str.chars().next().unwrap();
        let toggled = if first.is_ascii_uppercase() {
            first.to_ascii_lowercase()
        } else {
            first.to_ascii_uppercase()
        };
        format!("{}{}", toggled, &path_str[1..])
    } else {
        path_str.to_string()
    }
}

// ============================================================================
// CHALLENGE 1: MCP initialize roots resolution stress matrix
// ============================================================================

#[test]
fn test_adversarial_mcp_initialize_roots_comprehensive_matrix() {
    let (repo, _db_path, _code) = setup_fixture_repo();
    let root = repo.path();
    let root_fwd = root.to_string_lossy().replace('\\', "/");
    let toggled_root_fwd = toggle_drive_letter(&root_fwd);

    // Matrix of initialize variations to challenge:
    let variations: Vec<(&str, Value)> = vec![
        // 1. Standard 3-slash with uppercase/default drive
        (
            "three_slash_standard",
            json!({
                "roots": [{ "uri": format!("file:///{root_fwd}") }]
            }),
        ),
        // 2. 3-slash with inverted drive casing (e.g. c: instead of C:)
        (
            "three_slash_inverted_drive",
            json!({
                "roots": [{ "uri": format!("file:///{toggled_root_fwd}") }]
            }),
        ),
        // 3. Two-slash non-standard with uppercase drive
        (
            "two_slash_standard",
            json!({
                "roots": [{ "uri": format!("file://{root_fwd}") }]
            }),
        ),
        // 4. Two-slash non-standard with inverted drive casing
        (
            "two_slash_inverted_drive",
            json!({
                "roots": [{ "uri": format!("file://{toggled_root_fwd}") }]
            }),
        ),
        // 5. 3-slash with trailing slash
        (
            "three_slash_trailing_slash",
            json!({
                "roots": [{ "uri": format!("file:///{root_fwd}/") }]
            }),
        ),
        // 6. Two-slash with trailing slash
        (
            "two_slash_trailing_slash",
            json!({
                "roots": [{ "uri": format!("file://{root_fwd}/") }]
            }),
        ),
        // 7. params.rootUri
        (
            "params_root_uri",
            json!({
                "rootUri": format!("file:///{root_fwd}")
            }),
        ),
        // 8. params.workspaceFolders
        (
            "params_workspace_folders",
            json!({
                "workspaceFolders": [{ "uri": format!("file:///{root_fwd}"), "name": "ws" }]
            }),
        ),
        // 9. params.rootPath with raw Windows path
        (
            "params_root_path_raw",
            json!({
                "rootPath": root.to_string_lossy().to_string()
            }),
        ),
        // 10. Multiple roots in params.roots (first is valid target)
        (
            "multiple_roots_first_valid",
            json!({
                "roots": [
                    { "uri": format!("file:///{root_fwd}") },
                    { "uri": "file:///C:/nonexistent_secondary_root" }
                ]
            }),
        ),
    ];

    for (name, params_payload) in variations {
        // Spawn server with ZERO --root arguments in arbitrary CWD
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_code-kb"))
                .arg("serve")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap_or_else(|e| panic!("Failed to spawn code-kb serve for {name}: {e}")),
        );

        let mut stdin = child.stdin.take().expect("Failed to open stdin");
        let stdout = child.stdout.take().expect("Failed to open stdout");
        let mut reader = BufReader::new(stdout);

        let mut init_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "matrix-tester", "version": "1.0" }
            }
        });

        if let Some(obj) = params_payload.as_object() {
            for (k, v) in obj {
                init_req["params"][k] = v.clone();
            }
        }

        let mut line = serde_json::to_string(&init_req).unwrap();
        line.push('\n');
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut init_resp = String::new();
        reader.read_line(&mut init_resp).unwrap();
        let resp: Value = serde_json::from_str(&init_resp)
            .unwrap_or_else(|e| panic!("Failed to parse JSON init response for {name}: {e}"));
        assert_eq!(resp["id"], 1, "{name}: Expected response id 1");
        assert_eq!(
            resp["result"]["serverInfo"]["name"], "code-kb",
            "{name}: Expected code-kb serverInfo"
        );

        // Verify find_symbol resolves symbol seamlessly without --root argument
        let call_find = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "find_symbol",
                "arguments": { "query": "compute_sum" }
            }
        });
        let mut call_line = serde_json::to_string(&call_find).unwrap();
        call_line.push('\n');
        stdin.write_all(call_line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut call_resp = String::new();
        reader.read_line(&mut call_resp).unwrap();
        let call_val: Value = serde_json::from_str(&call_resp)
            .unwrap_or_else(|e| panic!("Failed to parse find_symbol response for {name}: {e}"));
        assert_eq!(call_val["id"], 2);
        assert_ne!(
            call_val["result"]["isError"], true,
            "{name}: find_symbol returned error: {call_val:?}"
        );
        let text = call_val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("compute_sum"),
            "{name}: Expected compute_sum in result text, got: {text}"
        );

        // Verify file_skeleton with relative path resolves seamlessly
        let call_skel = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "file_skeleton",
                "arguments": { "file_path": "src/calc.rs" }
            }
        });
        let mut skel_line = serde_json::to_string(&call_skel).unwrap();
        skel_line.push('\n');
        stdin.write_all(skel_line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut skel_resp = String::new();
        reader.read_line(&mut skel_resp).unwrap();
        let skel_val: Value = serde_json::from_str(&skel_resp).unwrap();
        assert_eq!(skel_val["id"], 3);
        assert_ne!(
            skel_val["result"]["isError"], true,
            "{name}: file_skeleton returned error: {skel_val:?}"
        );
        let skel_text = skel_val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            skel_text.contains("compute_sum"),
            "{name}: Expected compute_sum in skeleton text, got: {skel_text}"
        );

        drop(stdin);
        let _ = child.wait();
    }
}

// ============================================================================
// CHALLENGE 2: Dynamic workspace rebinding and drive casing churn invariance
// ============================================================================

#[test]
#[cfg(windows)]
fn test_adversarial_mcp_dynamic_rebinding_drive_casing_and_interleaved_churn() {
    let (repo, _db_path, _code) = setup_fixture_repo();
    let root = repo.path();
    let abs_file = root.join("src").join("calc.rs");
    let abs_file_str = abs_file.to_string_lossy().to_string();
    let inverted_abs_file = toggle_drive_letter(&abs_file_str);

    assert_ne!(
        abs_file_str, inverted_abs_file,
        "Test requires a drive-letter path on Windows (e.g. C: vs c:)"
    );

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
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
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "casing-churn-tester", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut init_resp = String::new();
    reader.read_line(&mut init_resp).unwrap();

    // 1. Stress: Rapidly interleave 10 tool calls alternating between original casing and inverted drive casing
    for i in 2..=11 {
        let use_inverted = i % 2 == 0;
        let file_arg = if use_inverted {
            &inverted_abs_file
        } else {
            &abs_file_str
        };

        let call = json!({
            "jsonrpc": "2.0",
            "id": i,
            "method": "tools/call",
            "params": {
                "name": "get_symbol_body",
                "arguments": {
                    "symbol": "compute_sum",
                    "file": file_arg
                }
            }
        });
        let mut call_line = serde_json::to_string(&call).unwrap();
        call_line.push('\n');
        stdin.write_all(call_line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut resp_str = String::new();
        reader.read_line(&mut resp_str).unwrap();
        let resp_val: Value = serde_json::from_str(&resp_str).unwrap();

        assert_eq!(resp_val["id"], i);
        assert_ne!(
            resp_val["result"]["isError"], true,
            "Iteration {i} (inverted={use_inverted}) failed with error: {resp_val:?}"
        );
        let text = resp_val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("compute_sum"),
            "Iteration {i} failed to return body"
        );
    }

    // 2. Challenge with file:// URI tool arguments (both standard and inverted casing)
    let fwd_inverted = inverted_abs_file.replace('\\', "/");
    let uri_arg = format!("file:///{fwd_inverted}");
    let call_uri = json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": uri_arg }
        }
    });
    let mut uri_line = serde_json::to_string(&call_uri).unwrap();
    uri_line.push('\n');
    stdin.write_all(uri_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut uri_resp = String::new();
    reader.read_line(&mut uri_resp).unwrap();
    let uri_val: Value = serde_json::from_str(&uri_resp).unwrap();
    assert_eq!(uri_val["id"], 12);
    assert_ne!(
        uri_val["result"]["isError"], true,
        "file:// URI argument with inverted casing failed: {uri_val:?}"
    );

    // 3. Challenge with verbatim prefix: \\?\c:\...
    let verbatim_arg = format!(r"\\?\{inverted_abs_file}");
    let call_verb = json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": verbatim_arg }
        }
    });
    let mut verb_line = serde_json::to_string(&call_verb).unwrap();
    verb_line.push('\n');
    stdin.write_all(verb_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut verb_resp = String::new();
    reader.read_line(&mut verb_resp).unwrap();
    let verb_val: Value = serde_json::from_str(&verb_resp).unwrap();
    assert_eq!(verb_val["id"], 13);
    assert_ne!(
        verb_val["result"]["isError"], true,
        "Verbatim prefix argument with inverted casing failed: {verb_val:?}"
    );

    // 4. Verify atomic edit works seamlessly with inverted drive casing
    let new_body = "{\n    let x = a + b;\n    x\n}";
    let edit_call = json!({
        "jsonrpc": "2.0",
        "id": 14,
        "method": "tools/call",
        "params": {
            "name": "replace_symbol_body",
            "arguments": {
                "symbol_name": "compute_sum",
                "file_path": inverted_abs_file,
                "new_body": new_body
            }
        }
    });
    let mut edit_line = serde_json::to_string(&edit_call).unwrap();
    edit_line.push('\n');
    stdin.write_all(edit_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut edit_resp = String::new();
    reader.read_line(&mut edit_resp).unwrap();
    let edit_val: Value = serde_json::from_str(&edit_resp).unwrap();
    assert_eq!(edit_val["id"], 14);
    assert_ne!(
        edit_val["result"]["isError"], true,
        "replace_symbol_body with inverted drive casing failed: {edit_val:?}"
    );

    // 5. Subsequent query with relative path reflects edited body immediately
    let verify_call = json!({
        "jsonrpc": "2.0",
        "id": 15,
        "method": "tools/call",
        "params": {
            "name": "get_symbol_body",
            "arguments": {
                "symbol": "compute_sum",
                "file": "src/calc.rs"
            }
        }
    });
    let mut verify_line = serde_json::to_string(&verify_call).unwrap();
    verify_line.push('\n');
    stdin.write_all(verify_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut verify_resp = String::new();
    reader.read_line(&mut verify_resp).unwrap();
    let verify_val: Value = serde_json::from_str(&verify_resp).unwrap();
    assert_eq!(verify_val["id"], 15);
    assert_ne!(verify_val["result"]["isError"], true);
    let updated_text = verify_val["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        updated_text.contains("let x = a + b;"),
        "Updated body not reflected: {updated_text}"
    );

    drop(stdin);
    let _ = child.wait();
}

// ============================================================================
// CHALLENGE 3: Reconcile offline edits _seen NOCASE collation stress test
// ============================================================================

#[test]
fn test_adversarial_reconcile_offline_edits_seen_table_nocase_collation() {
    let temp = code_kb_core::safe_tempdir();
    let root = temp.path().to_path_buf();
    let db_path = root.join("test.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();

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

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    // Create 3 files on disk:
    // 1. main.rs (lowercase on disk)
    // 2. UTIL.RS (uppercase on disk)
    // 3. MixedCase.rs (mixed on disk)
    let f1_path = src_dir.join("main.rs");
    let f1_content = "fn main() {}\n";
    fs::write(&f1_path, f1_content).unwrap();
    let f1_hash = format!("blake3:{}", blake3::hash(f1_content.as_bytes()).to_hex());

    let f2_path = src_dir.join("UTIL.RS");
    let f2_content = "pub fn util() {}\n";
    fs::write(&f2_path, f2_content).unwrap();
    let f2_hash = format!("blake3:{}", blake3::hash(f2_content.as_bytes()).to_hex());

    let f3_path = src_dir.join("MixedCase.rs");
    let f3_content = "pub fn mixed() {}\n";
    fs::write(&f3_path, f3_content).unwrap();
    let f3_hash = format!("blake3:{}", blake3::hash(f3_content.as_bytes()).to_hex());

    // Insert into SQLite `files` with OPPOSITE casing:
    // 1. src/MAIN.RS in DB vs src/main.rs on disk
    // 2. src/util.rs in DB vs src/UTIL.RS on disk
    // 3. src/mixedcase.rs in DB vs src/MixedCase.rs on disk
    // 4. src/deleted_file.rs in DB but NOT on disk (must be deleted)
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/MAIN.RS', 'rust', ?1, ?2, 1, '2026-09-14')",
        rusqlite::params![f1_hash, f1_content.len() as i64],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO files VALUES ('f2', 'src/util.rs', 'rust', ?1, ?2, 1, '2026-09-14')",
        rusqlite::params![f2_hash, f2_content.len() as i64],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO files VALUES ('f3', 'src/mixedcase.rs', 'rust', ?1, ?2, 1, '2026-09-14')",
        rusqlite::params![f3_hash, f3_content.len() as i64],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO files VALUES ('f4', 'src/deleted_file.rs', 'rust', 'hash_del', 100, 1, '2026-09-14')",
        [],
    )
    .unwrap();

    // Create a new untracked file on disk
    let f_new_path = src_dir.join("new_feature.rs");
    fs::write(&f_new_path, "pub fn new_fn() {}\n").unwrap();

    let ws = code_kb_core::Workspace::new(root);

    // Run reconcile_offline_edits
    let report = code_kb_core::sync::reconcile_offline_edits(&ws, &db_path, &conn)
        .expect("reconcile_offline_edits must succeed");

    // EMPIRICAL ASSERTIONS:
    // 1. Files with casing differences (src/MAIN.RS, src/util.rs, src/mixedcase.rs)
    //    MUST NOT be marked deleted due to COLLATE NOCASE!
    assert!(
        !report.deleted.contains(&"src/MAIN.RS".to_string()),
        "src/MAIN.RS was erroneously marked deleted despite main.rs existing on disk!"
    );
    assert!(
        !report.deleted.contains(&"src/util.rs".to_string()),
        "src/util.rs was erroneously marked deleted despite UTIL.RS existing on disk!"
    );
    assert!(
        !report.deleted.contains(&"src/mixedcase.rs".to_string()),
        "src/mixedcase.rs was erroneously marked deleted despite MixedCase.rs existing on disk!"
    );

    // 2. The genuine deleted file MUST be detected
    assert!(
        report.deleted.contains(&"src/deleted_file.rs".to_string()),
        "src/deleted_file.rs should be detected as deleted: {:?}",
        report.deleted
    );
    assert_eq!(
        report.deleted.len(),
        1,
        "ONLY the genuinely deleted file should be in report.deleted: {:?}",
        report.deleted
    );

    // 3. The new file on disk MUST be detected as added
    assert!(
        report.added.iter().any(|p| p.ends_with("new_feature.rs")),
        "new_feature.rs must be detected as added: {:?}",
        report.added
    );
}

// ============================================================================
// CHALLENGE 4: Core Invariant #1 MCP tool schema strict verification
// ============================================================================

#[test]
fn test_adversarial_mcp_core_invariant_1_exhaustive_blacklist() {
    let (repo, _db_path, _code) = setup_fixture_repo();
    let root = repo.path();

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
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
    let mut reader = BufReader::new(stdout);

    // 1. Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "schema-invariants", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut init_resp = String::new();
    reader.read_line(&mut init_resp).unwrap();

    // 2. Request tools/list
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
    let mut list_resp = String::new();
    reader.read_line(&mut list_resp).unwrap();

    let resp_val: Value = serde_json::from_str(&list_resp).unwrap();
    assert_eq!(resp_val["id"], 2);

    let tools = resp_val["result"]["tools"]
        .as_array()
        .expect("Tools array in response");

    assert_eq!(
        tools.len(),
        11,
        "MCP server must advertise exactly 11 tools"
    );

    let forbidden_param_blacklist = [
        "workspace",
        "workspace_id",
        "workspace_root",
        "workspace_dir",
        "workspace_path",
        "repo",
        "repo_path",
        "repo_root",
        "repo_dir",
        "root",
        "root_dir",
        "root_path",
        "cwd",
        "workdir",
        "work_dir",
        "working_dir",
        "repository",
        "repository_path",
    ];

    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];

        // Check properties
        if let Some(props) = schema["properties"].as_object() {
            for key in props.keys() {
                for forbidden in &forbidden_param_blacklist {
                    assert_ne!(
                        key.as_str(),
                        *forbidden,
                        "CRITICAL INVARIANT VIOLATION: Tool '{name}' exposes forbidden parameter '{key}'"
                    );
                }
            }
        }

        // Check required array
        if let Some(req_arr) = schema["required"].as_array() {
            for req in req_arr {
                let req_str = req.as_str().unwrap();
                for forbidden in &forbidden_param_blacklist {
                    assert_ne!(
                        req_str, *forbidden,
                        "CRITICAL INVARIANT VIOLATION: Tool '{name}' marks forbidden parameter '{req_str}' as required"
                    );
                }
            }
        }

        // Check descriptions: ensure no tool description instructs models to supply workspace paths
        if let Some(desc) = tool["description"].as_str() {
            let desc_lower = desc.to_lowercase();
            assert!(
                !desc_lower.contains("pass workspace")
                    && !desc_lower.contains("pass the workspace")
                    && !desc_lower.contains("provide workspace root"),
                "Tool '{name}' description prompts for workspace: {desc}"
            );
        }
    }

    // 3. Test Core Invariant #1 Internal Compatibility:
    // If an unadvertised `workspace` argument is provided internally, the backend accepts it silently.
    let unadvertised_call = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "find_symbol",
            "arguments": {
                "query": "compute_sum",
                "workspace": root.to_string_lossy().to_string()
            }
        }
    });
    let mut unadv_line = serde_json::to_string(&unadvertised_call).unwrap();
    unadv_line.push('\n');
    stdin.write_all(unadv_line.as_bytes()).unwrap();
    stdin.flush().unwrap();

    let mut unadv_resp = String::new();
    reader.read_line(&mut unadv_resp).unwrap();
    let unadv_val: Value = serde_json::from_str(&unadv_resp).unwrap();
    assert_eq!(unadv_val["id"], 3);
    assert_ne!(
        unadv_val["result"]["isError"], true,
        "Unadvertised workspace parameter should be accepted silently: {unadv_val:?}"
    );
    let text = unadv_val["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("compute_sum"),
        "Unadvertised call must execute successfully"
    );

    drop(stdin);
    let _ = child.wait();
}
