use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

const PROJECT_ROOT_DESCRIPTION: &str = "Absolute path of the project or git worktree you are working in. Send the same value on every call. Change it when you move to a worktree or another project.";

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
    scanned_repo("pub fn compute_sum(a: i32, b: i32) -> i32 {\n    a + b\n}\n")
}

fn scanned_repo(code: &str) -> (tempfile::TempDir, PathBuf, String) {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("calc.rs");
    fs::write(&file_path, code).unwrap();

    let scan_out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
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

fn percent_encode_last_segment(path: &str) -> String {
    let (parent, last) = path.rsplit_once('/').unwrap();
    let encoded: String = last.bytes().map(|byte| format!("%{byte:02X}")).collect();
    format!("{parent}/{encoded}")
}

fn project_root_variants(root: &Path) -> Vec<(&'static str, String, Value)> {
    let root_raw = root.to_string_lossy().to_string();
    let root_fwd = root_raw.replace('\\', "/");
    let toggled_root_fwd = toggle_drive_letter(&root_fwd);
    let standard_uri = format!("file:///{root_fwd}");
    [
        ("three_slash_standard", standard_uri.clone()),
        (
            "three_slash_inverted_drive",
            format!("file:///{toggled_root_fwd}"),
        ),
        ("two_slash_standard", format!("file://{root_fwd}")),
        (
            "two_slash_inverted_drive",
            format!("file://{toggled_root_fwd}"),
        ),
        ("three_slash_trailing_slash", format!("file:///{root_fwd}/")),
        ("two_slash_trailing_slash", format!("file://{root_fwd}/")),
        (
            "percent_encoded_segment",
            format!("file://{}", percent_encode_last_segment(&root_fwd)),
        ),
    ]
    .into_iter()
    .map(|(name, uri)| {
        let initialize_params = json!({ "roots": [{ "uri": uri }] });
        (name, uri, initialize_params)
    })
    .chain([
        (
            "params_root_uri",
            standard_uri.clone(),
            json!({ "rootUri": standard_uri }),
        ),
        (
            "params_workspace_folders",
            standard_uri.clone(),
            json!({ "workspaceFolders": [{ "uri": standard_uri, "name": "ws" }] }),
        ),
        (
            "params_root_path_raw",
            root_raw.clone(),
            json!({ "rootPath": root_raw }),
        ),
        (
            "multiple_roots_first_valid",
            standard_uri.clone(),
            json!({
                "roots": [
                    { "uri": standard_uri },
                    { "uri": "file:///C:/nonexistent_secondary_root" }
                ]
            }),
        ),
    ])
    .collect()
}

#[test]
fn test_adversarial_mcp_project_root_variants_answer_from_the_fixture_while_initialize_roots_are_ignored()
 {
    let (repo, _db_path, _code) = setup_fixture_repo();
    let root = repo.path();
    let (other, _other_db_path, _other_code) =
        scanned_repo("pub fn compute_product(a: i32, b: i32) -> i32 {\n    a * b\n}\n");

    for ((name, project_root, _), (_, _, other_initialize_params)) in project_root_variants(root)
        .into_iter()
        .zip(project_root_variants(other.path()))
    {
        let telem_dir = code_kb_core::safe_tempdir();
        let mut child = ChildGuard(
            Command::new(env!("CARGO_BIN_EXE_code-kb"))
                .env("CODE_KB_TELEMETRY_DIR", telem_dir.path())
                .env("CODE_KB_INDEX_WAIT_MS", "60000")
                .arg("serve")
                .arg("--root")
                .arg(other.path())
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

        if let Some(obj) = other_initialize_params.as_object() {
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

        let call_find = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lookup_symbol",
                "arguments": { "project_root": project_root, "query": "compute_sum" }
            }
        });
        let mut call_line = serde_json::to_string(&call_find).unwrap();
        call_line.push('\n');
        stdin.write_all(call_line.as_bytes()).unwrap();
        stdin.flush().unwrap();

        let mut call_resp = String::new();
        reader.read_line(&mut call_resp).unwrap();
        let call_val: Value = serde_json::from_str(&call_resp)
            .unwrap_or_else(|e| panic!("Failed to parse lookup_symbol response for {name}: {e}"));
        assert_eq!(call_val["id"], 2);
        assert_ne!(
            call_val["result"]["isError"], true,
            "{name}: lookup_symbol returned error: {call_val:?}"
        );
        let text = call_val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("pub fn compute_sum(a: i32, b: i32) -> i32"),
            "{name}: Expected the fixture's compute_sum in result text, got: {text}"
        );

        let call_skel = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "file_skeleton",
                "arguments": { "project_root": project_root, "file_path": "src/calc.rs" }
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
            skel_text.contains("pub fn compute_sum(a: i32, b: i32) -> i32"),
            "{name}: Expected the fixture's compute_sum in skeleton text, got: {skel_text}"
        );

        drop(stdin);
        let _ = child.wait();
    }
}

#[test]
#[cfg(windows)]
fn test_adversarial_mcp_project_root_casing_slash_and_uri_churn_answers_from_one_index() {
    let (repo, db_path, _code) = setup_fixture_repo();
    let root = repo.path();
    let root_str = root.to_string_lossy().to_string();
    let inverted_root = toggle_drive_letter(&root_str);
    let abs_file = root.join("src").join("calc.rs");
    let abs_file_str = abs_file.to_string_lossy().to_string();
    let inverted_abs_file = toggle_drive_letter(&abs_file_str);

    assert_ne!(
        abs_file_str, inverted_abs_file,
        "Test requires a drive-letter path on Windows (e.g. C: vs c:)"
    );

    let project_roots = [
        root_str.clone(),
        inverted_root.clone(),
        root_str.replace('\\', "/"),
        format!("{inverted_root}\\"),
        format!("file:///{}", inverted_root.replace('\\', "/")),
        format!("file://{}/", root_str.replace('\\', "/")),
    ];
    let telemetry_dir = root.join(".telemetry_test");

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", &telemetry_dir)
            .env("CODE_KB_INDEX_WAIT_MS", "60000")
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

    for i in 2..=11 {
        let use_inverted = i % 2 == 0;
        let file_arg = if use_inverted {
            &inverted_abs_file
        } else {
            &abs_file_str
        };
        let project_root = &project_roots[(i - 2) % project_roots.len()];

        let call = json!({
            "jsonrpc": "2.0",
            "id": i,
            "method": "tools/call",
            "params": {
                "name": "get_symbol_body",
                "arguments": {
                    "project_root": project_root,
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
            "Iteration {i} (inverted={use_inverted}, project_root={project_root}) failed with error: {resp_val:?}"
        );
        let text = resp_val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("compute_sum"),
            "Iteration {i} (project_root={project_root}) failed to return body"
        );
    }

    let fwd_inverted = inverted_abs_file.replace('\\', "/");
    let uri_arg = format!("file:///{fwd_inverted}");
    let call_uri = json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "project_root": &project_roots[4], "file": uri_arg }
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

    let verbatim_arg = format!(r"\\?\{inverted_abs_file}");
    let call_verb = json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "project_root": &inverted_root, "file": verbatim_arg }
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

    drop(stdin);
    let _ = child.wait();

    let conn = code_kb_core::Connection::open(telemetry_dir.join("telemetry.db")).unwrap();
    let recorded_roots = conn
        .prepare(
            "SELECT workspace_root FROM tool_telemetry
             WHERE tool IN ('get_symbol_body', 'file_skeleton') ORDER BY rowid",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let fixture_root = code_kb_core::to_forward_slash(
        &code_kb_core::Workspace::new(root.to_path_buf()).canonical_root,
    );
    assert_eq!(
        recorded_roots,
        vec![fixture_root; 12],
        "every call must answer from the fixture index"
    );

    let mut dirs = vec![root.to_path_buf()];
    let mut indexes = Vec::new();
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if entry.file_name() == "artifact.db" {
                indexes.push(path);
            }
        }
    }
    assert_eq!(
        indexes,
        vec![db_path],
        "the churn must not create a second index"
    );
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
fn test_adversarial_mcp_schemas_have_no_blacklisted_workspace_parameter_and_require_project_root() {
    let (repo, _db_path, _code) = setup_fixture_repo();
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
        10,
        "MCP server must advertise exactly 10 tools"
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

        let required: Vec<&str> = schema["required"]
            .as_array()
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if name == "telemetry_summary" {
            assert!(
                schema["properties"].get("project_root").is_none(),
                "Tool '{name}' must not list project_root"
            );
            assert!(
                !required.contains(&"project_root"),
                "Tool '{name}' must not require project_root"
            );
        } else {
            assert_eq!(
                schema["properties"]["project_root"]["description"], PROJECT_ROOT_DESCRIPTION,
                "Tool '{name}' must list project_root with its contract description"
            );
            assert!(
                required.contains(&"project_root"),
                "Tool '{name}' must require project_root"
            );
        }

        let property_descriptions = schema["properties"]
            .as_object()
            .into_iter()
            .flat_map(|props| props.values())
            .filter_map(|prop| prop["description"].as_str());
        for desc in tool["description"]
            .as_str()
            .into_iter()
            .chain(property_descriptions)
        {
            let desc_lower = desc.to_lowercase();
            assert!(
                !desc_lower.contains("pass workspace")
                    && !desc_lower.contains("pass the workspace")
                    && !desc_lower.contains("provide workspace root"),
                "Tool '{name}' description prompts for workspace: {desc}"
            );
        }
    }

    let unadvertised_call = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "lookup_symbol",
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
        "Unadvertised workspace parameter should act as the project_root alias: {unadv_val:?}"
    );
    let text = unadv_val["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("pub fn compute_sum(a: i32, b: i32) -> i32"),
        "Unadvertised call must execute successfully"
    );

    drop(stdin);
    let _ = child.wait();
}
