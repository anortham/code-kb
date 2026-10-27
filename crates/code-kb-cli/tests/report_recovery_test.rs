use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct McpSession {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    _telemetry: tempfile::TempDir,
}

impl McpSession {
    fn start(root: &Path) -> Self {
        let telemetry = code_kb_core::safe_tempdir();
        let mut child = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .env("CODE_KB_TELEMETRY_DIR", telemetry.path())
            .arg("serve")
            .arg("--root")
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        let mut session = Self {
            child,
            stdin,
            reader,
            _telemetry: telemetry,
        };
        let initialized = session.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "report-recovery-test", "version": "1.0"}
            }),
        );
        assert_eq!(initialized["id"], 1);
        session
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
        let mut response = String::new();
        self.reader.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    fn call(&mut self, root: &Path, name: &str, mut arguments: Value) -> Value {
        arguments["project_root"] = json!(root);
        self.request("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone()
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn three_caller_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    for (path, source) in [
        (
            "pyproject.toml",
            "[project]\nname = 'three'\nversion = '0.1.0'\n",
        ),
        ("util.py", "def helper():\n    return 1\n"),
        (
            "app.py",
            "from util import helper\n\n\ndef a():\n    return helper()\n\n\ndef b():\n    return helper()\n\n\ndef c():\n    return helper()  # helper again\n",
        ),
        ("notes.md", "The helper function is documented here.\n"),
    ] {
        std::fs::write(repo.path().join(path), source).unwrap();
    }
    repo
}

fn many_caller_source(changed: bool) -> String {
    let mut source = if changed {
        "def helper(value):\n    return value\n\n".to_string()
    } else {
        "def helper():\n    return 1\n\n".to_string()
    };
    for index in 0..220 {
        source.push_str(&format!(
            "def caller_{index:03}():\n    return helper()\n\n"
        ));
    }
    source
}

fn many_caller_repo(with_signature_change: bool) -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    std::fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = 'many-callers'\nversion = '0.1.0'\n",
    )
    .unwrap();
    let app_path = repo.path().join("app.py");
    std::fs::write(&app_path, many_caller_source(false)).unwrap();
    if with_signature_change {
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(repo.path())
            .status()
            .unwrap();
        Command::new("git")
            .args([
                "-c",
                "user.name=Report Recovery Test",
                "-c",
                "user.email=report-recovery@example.test",
                "commit",
                "-qm",
                "base",
            ])
            .current_dir(repo.path())
            .status()
            .unwrap();
        std::fs::write(&app_path, many_caller_source(true)).unwrap();
    }
    repo
}

fn many_callee_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    std::fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = 'many-callees'\nversion = '0.1.0'\n",
    )
    .unwrap();
    let mut source = String::new();
    for index in 0..220 {
        source.push_str(&format!("def callee_{index:03}():\n    return {index}\n\n"));
    }
    source.push_str("def caller():\n");
    for index in 0..220 {
        source.push_str(&format!("    callee_{index:03}()\n"));
    }
    std::fs::write(repo.path().join("app.py"), source).unwrap();
    repo
}

fn many_qualified_callee_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    std::fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = 'many-qualified-callees'\nversion = '0.1.0'\n",
    )
    .unwrap();
    let mut source = String::new();
    for index in 0..220 {
        source.push_str(&format!("def callee_{index:03}():\n    return {index}\n\n"));
    }
    source.push_str("class Owner:\n    def caller(self):\n");
    for index in 0..220 {
        source.push_str(&format!("        callee_{index:03}()\n"));
    }
    std::fs::write(repo.path().join("app.py"), source).unwrap();
    repo
}

fn member_candidate_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    for (path, source) in [
        (
            "pyproject.toml",
            "[project]\nname = 'candidate'\nversion = '0.1.0'\n",
        ),
        (
            "runners.py",
            "class Runner:\n    def invoke(self):\n        pass\n",
        ),
        (
            "clients.py",
            "from external import invoke\n\ndef unknown(value):\n    return value.invoke\n",
        ),
    ] {
        std::fs::write(repo.path().join(path), source).unwrap();
    }
    repo
}

fn one_line_xml_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    std::fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = 'xml-fixture'\nversion = '0.1.0'\n",
    )
    .unwrap();
    std::fs::write(
        repo.path().join("Project.csproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\"><Target Name=\"anchor_xml\"><Message Text=\"41\" /></Target><Target Name=\"caller_xml\"><CallTarget Targets=\"anchor_xml\" /></Target></Project>",
    )
    .unwrap();
    repo
}

fn css_call_repo() -> tempfile::TempDir {
    css_call_repo_with_anchor_body("result: 41;")
}

fn css_call_repo_with_anchor_body(anchor_body: &str) -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    std::fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = 'css-call'\nversion = '0.1.0'\n",
    )
    .unwrap();
    std::fs::write(
        repo.path().join("sample.css"),
        format!(
            "@function --anchor_css() {{ {anchor_body} }}\n@function --caller_css() {{ result: --anchor_css(); }}\n"
        ),
    )
    .unwrap();
    repo
}

fn css_many_call_repo(call_count: usize) -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    std::fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = 'css-call-pages'\nversion = '0.1.0'\n",
    )
    .unwrap();
    let calls = (0..call_count)
        .map(|index| format!("  step{index}: --anchor_css();\n"))
        .collect::<String>();
    std::fs::write(
        repo.path().join("sample.css"),
        format!(
            "@function --anchor_css() {{ result: 41; }}\n@function --caller_css() {{\n{calls}}}\n"
        ),
    )
    .unwrap();
    repo
}

fn css_multi_call_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    std::fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = 'css-multi-call'\nversion = '0.1.0'\n",
    )
    .unwrap();
    std::fs::write(
        repo.path().join("sample.css"),
        "@function --anchor_css() { result: 41; }\n@function --caller_css() {\n    result: --anchor_css();\nx:--anchor_css();\n}\n",
    )
    .unwrap();
    repo
}

fn changed_repo() -> tempfile::TempDir {
    let repo = three_caller_repo();
    let root = repo.path();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .status()
        .unwrap();
    Command::new("git")
        .args([
            "-c",
            "user.name=Report Recovery Test",
            "-c",
            "user.email=report-recovery@example.test",
            "commit",
            "-qm",
            "base",
        ])
        .current_dir(root)
        .status()
        .unwrap();
    std::fs::write(
        root.join("util.py"),
        "def helper(extra):\n    return 1 + extra\n",
    )
    .unwrap();
    repo
}

fn package_test_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    for (path, source) in [
        (
            "package.json",
            "{\"name\":\"broader\",\"scripts\":{\"test\":\"node --test\"}}\n",
        ),
        (
            "src/helper.js",
            "function helper() { return 1; }\nmodule.exports = helper;\n",
        ),
        (
            "test/helper.test.cjs",
            "const test = require('node:test');\nconst assert = require('node:assert/strict');\ntest('configured full suite runs', () => assert.equal(1, 1));\n",
        ),
    ] {
        let path = repo.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    repo
}

fn nested_package_test_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo.path())
        .status()
        .unwrap();
    let package = repo.path().join("packages/test suite");
    for (path, source) in [
        (
            "package.json",
            "{\"name\":\"nested\",\"scripts\":{\"test\":\"node --test\"}}\n",
        ),
        (
            "src/helper.js",
            "function helper() { return 1; }\nmodule.exports = { helper };\n",
        ),
        (
            "test/a.test.cjs",
            "const test = require('node:test');\nconst { helper } = require('../src/helper.js');\ntest('a configured test runs', () => helper());\n",
        ),
        (
            "test/b.test.cjs",
            "const test = require('node:test');\nconst { helper } = require('../src/helper.js');\ntest('b configured test runs', () => helper());\n",
        ),
    ] {
        let path = package.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    repo
}

fn unconfigured_test_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    std::fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = 'unconfigured'\nversion = '0.1.0'\n",
    )
    .unwrap();
    std::fs::write(repo.path().join("app.py"), "def helper():\n    return 1\n").unwrap();
    repo
}

fn index_import_edge(root: &Path, name: &str, kind: &str, target_path: &str, path: &str) {
    let db = rusqlite::Connection::open(root.join(".code-kb/artifact.db")).unwrap();
    let target: String = db
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = ?1 AND kind = ?2 AND path = ?3",
            rusqlite::params![name, kind, target_path],
            |row| row.get(0),
        )
        .unwrap();
    let import: String = db
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = ?1 AND kind = 'import' AND path = ?2",
            rusqlite::params![name, path],
            |row| row.get(0),
        )
        .unwrap();
    let file_id: String = db
        .query_row("SELECT file_id FROM files WHERE path = ?1", [path], |row| {
            row.get(0)
        })
        .unwrap();
    db.execute(
        "INSERT INTO reference_sites (reference_site_id, file_id, path, language, containing_symbol_id, start_line, start_column, end_line, end_column, start_byte, end_byte, is_exact, provenance) VALUES (?1, ?2, ?3, 'python', ?4, 1, 0, 1, 6, 0, 6, 1, 'target_token')",
        rusqlite::params!["report-recovery-reference-site", file_id, path, import],
    )
    .unwrap();
    db.execute(
        "INSERT INTO relationships (relationship_id, reference_site_id, from_symbol_id, to_symbol_id, file_id, path, kind, start_line, confidence) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'calls', 1, 1.0)",
        rusqlite::params![
            "report-recovery-import-edge",
            "report-recovery-reference-site",
            import,
            target,
            file_id,
            path,
        ],
    )
    .unwrap();
}

fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

fn result_text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap()
}

#[test]
fn reference_pages_retrieve_folded_rows_with_cli_and_mcp_parity() {
    let repo = three_caller_repo();
    let root = repo.path();
    let output = cli(root, &["refs", "helper", "--limit", "1", "--offset", "1"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).unwrap();
    assert!(cli_text.contains("Sites: 3"), "{cli_text}");
    assert!(cli_text.contains("- `b` [app.py:9]"), "{cli_text}");
    assert!(!cli_text.contains("- `a` [app.py:5]"), "{cli_text}");

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "helper", "limit": 1, "offset": 1}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), cli_text);

    let default_cli = cli(root, &["refs", "helper"]);
    assert!(default_cli.status.success());
    let default_result = mcp.call(root, "find_references", json!({"symbol_name": "helper"}));
    assert_ne!(default_result["isError"], true, "{default_result}");
    assert_eq!(
        result_text(&default_result),
        String::from_utf8(default_cli.stdout).unwrap()
    );
}

#[test]
fn callee_pages_past_200_are_bounded_and_retrievable_by_name_and_id() {
    let repo = many_callee_repo();
    let root = repo.path();
    let first_page = cli(
        root,
        &[
            "refs",
            "caller",
            "--direction",
            "callees",
            "--limit",
            "1",
            "--file",
            "app.py",
        ],
    );
    assert!(
        first_page.status.success(),
        "{}",
        String::from_utf8_lossy(&first_page.stderr)
    );
    let first_page_text = String::from_utf8(first_page.stdout).unwrap();
    assert!(
        first_page_text.contains("Callees called by `caller` (220 found):"),
        "{first_page_text}"
    );
    assert!(
        first_page_text.contains("use --offset 1 to see more"),
        "{first_page_text}"
    );

    let output = cli(
        root,
        &[
            "refs",
            "caller",
            "--direction",
            "callees",
            "--limit",
            "1",
            "--offset",
            "200",
            "--file",
            "app.py",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let name_text = String::from_utf8(output.stdout).unwrap();
    assert!(
        name_text.contains("showing 1 row from 220 queried results"),
        "{name_text}"
    );
    assert!(name_text.contains("`callee_200`"), "{name_text}");

    let db = rusqlite::Connection::open(root.join(".code-kb/artifact.db")).unwrap();
    let caller_id: String = db
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'caller' AND path = 'app.py' AND kind = 'function'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(db);
    let output = cli(
        root,
        &[
            "refs",
            "--symbol-id",
            &caller_id,
            "--direction",
            "callees",
            "--limit",
            "1",
            "--offset",
            "200",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id_text = String::from_utf8(output.stdout).unwrap();
    assert_eq!(id_text, name_text);

    let output = cli(
        root,
        &[
            "refs",
            "--symbol-id",
            &caller_id,
            "--direction",
            "callees",
            "--limit",
            "1",
            "--offset",
            "300",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let past_end_text = String::from_utf8(output.stdout).unwrap();
    assert!(
        past_end_text.contains("Page offset 300: no rows within the 220 queried results."),
        "{past_end_text}"
    );

    let mut mcp = McpSession::start(root);
    let first_page = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "caller", "direction": "callees", "limit": 1, "file_path": "app.py"}),
    );
    assert_ne!(first_page["isError"], true, "{first_page}");
    assert_eq!(result_text(&first_page), first_page_text);
    let by_name = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "caller", "direction": "callees", "limit": 1, "offset": 200, "file_path": "app.py"}),
    );
    assert_ne!(by_name["isError"], true, "{by_name}");
    assert_eq!(result_text(&by_name), name_text);
    let by_id = mcp.call(
        root,
        "find_references",
        json!({"symbol_id": caller_id, "direction": "callees", "limit": 1, "offset": 200}),
    );
    assert_ne!(by_id["isError"], true, "{by_id}");
    assert_eq!(result_text(&by_id), id_text);
    let past_end = mcp.call(
        root,
        "find_references",
        json!({"symbol_id": caller_id, "direction": "callees", "limit": 1, "offset": 300}),
    );
    assert_ne!(past_end["isError"], true, "{past_end}");
    assert_eq!(result_text(&past_end), past_end_text);
}

#[test]
fn qualified_callee_pages_resolve_by_path_and_current_index_id() {
    let repo = many_qualified_callee_repo();
    let root = repo.path();
    let output = cli(
        root,
        &[
            "refs",
            "Owner.caller",
            "--direction",
            "callees",
            "--limit",
            "1",
            "--offset",
            "200",
            "--file",
            "app.py",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let name_text = String::from_utf8(output.stdout).unwrap();
    assert!(
        name_text.contains("Callees called by `Owner.caller` (220 found)"),
        "{name_text}"
    );
    let name_row = name_text
        .lines()
        .find(|line| line.contains("`callee_200`"))
        .unwrap();

    let db = rusqlite::Connection::open(root.join(".code-kb/artifact.db")).unwrap();
    let caller_id: String = db
        .query_row(
            "SELECT method.symbol_id FROM symbols method JOIN symbols owner ON owner.symbol_id = method.parent_symbol_id WHERE method.name = 'caller' AND owner.name = 'Owner' AND method.path = 'app.py' AND method.kind = 'method'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(db);
    let output = cli(
        root,
        &[
            "refs",
            "--symbol-id",
            &caller_id,
            "--direction",
            "callees",
            "--limit",
            "1",
            "--offset",
            "200",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let id_text = String::from_utf8(output.stdout).unwrap();
    let id_row = id_text
        .lines()
        .find(|line| line.contains("`callee_200`"))
        .unwrap();
    assert_eq!(id_row, name_row);

    let mut mcp = McpSession::start(root);
    let by_name = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "Owner.caller", "direction": "callees", "limit": 1, "offset": 200, "file_path": "app.py"}),
    );
    assert_ne!(by_name["isError"], true, "{by_name}");
    assert_eq!(
        result_text(&by_name)
            .lines()
            .find(|line| line.contains("`callee_200`")),
        Some(name_row)
    );
    let by_id = mcp.call(
        root,
        "find_references",
        json!({"symbol_id": caller_id, "direction": "callees", "limit": 1, "offset": 200}),
    );
    assert_ne!(by_id["isError"], true, "{by_id}");
    assert_eq!(
        result_text(&by_id)
            .lines()
            .find(|line| line.contains("`callee_200`")),
        Some(name_row)
    );
}

#[test]
fn caller_reference_pages_past_200_are_retrievable_over_cli_and_mcp() {
    let repo = many_caller_repo(false);
    let root = repo.path();
    let output = cli(root, &["refs", "helper", "--limit", "1", "--offset", "200"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).unwrap();
    assert!(
        cli_text.contains("Page offset 200: rows 201-201 of 220."),
        "{cli_text}"
    );
    assert!(cli_text.contains("`caller_200`"), "{cli_text}");

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "helper", "limit": 1, "offset": 200}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), cli_text);
}

#[test]
fn edit_check_pages_retrieve_folded_sites_with_cli_and_mcp_parity() {
    let repo = changed_repo();
    let root = repo.path();
    let default_output = cli(root, &["check", "--base", "HEAD"]);
    assert!(
        default_output.status.success(),
        "{}",
        String::from_utf8_lossy(&default_output.stderr)
    );
    let default_text = String::from_utf8(default_output.stdout).unwrap();
    assert!(default_text.contains("`a`"), "{default_text}");
    assert!(default_text.contains("`b`"), "{default_text}");

    let output = cli(root, &["check", "--base", "HEAD", "--offset", "1"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).unwrap();
    assert!(cli_text.contains("`b`"), "{cli_text}");
    assert!(!cli_text.contains("app.py:4"), "{cli_text}");

    let mut mcp = McpSession::start(root);
    let default_result = mcp.call(root, "check_edit", json!({"base": "HEAD"}));
    assert_ne!(default_result["isError"], true, "{default_result}");
    assert_eq!(result_text(&default_result), default_text);
    let result = mcp.call(root, "check_edit", json!({"base": "HEAD", "offset": 1}));
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), cli_text);
}

#[test]
fn edit_check_pages_past_200_are_retrievable_over_cli_and_mcp() {
    let repo = many_caller_repo(true);
    let root = repo.path();
    let output = cli(
        root,
        &["check", "--base", "HEAD", "--offset", "200", "--limit", "1"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).unwrap();
    assert!(
        cli_text.contains("Page offset 200: rows 201-201 of 220."),
        "{cli_text}"
    );
    assert!(cli_text.contains("caller_200"), "{cli_text}");

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "check_edit",
        json!({"base": "HEAD", "offset": 200, "limit": 1}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), cli_text);
}

#[test]
fn impact_pages_retrieve_folded_symbols_with_cli_and_mcp_parity() {
    let repo = three_caller_repo();
    let root = repo.path();
    let output = cli(
        root,
        &["blast-radius", "helper", "--limit", "1", "--offset", "1"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).unwrap();
    assert!(cli_text.contains("`b`"), "{cli_text}");
    assert!(!cli_text.contains("[line 4]"), "{cli_text}");

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "blast_radius",
        json!({"symbol": "helper", "limit": 1, "offset": 1}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), cli_text);
}

#[test]
fn low_signal_impact_rows_are_retrievable_over_cli_and_mcp() {
    let repo = three_caller_repo();
    let root = repo.path();
    let indexed = cli(root, &["blast-radius", "helper"]);
    assert!(
        indexed.status.success(),
        "{}",
        String::from_utf8_lossy(&indexed.stderr)
    );
    index_import_edge(root, "helper", "function", "util.py", "app.py");

    let output = cli(root, &["blast-radius", "helper", "--include-low-signal"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).unwrap();
    assert!(cli_text.contains("import `helper`"), "{cli_text}");

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "blast_radius",
        json!({"symbol": "helper", "include_low_signal": true}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), cli_text);
}

#[test]
fn low_signal_recovery_does_not_turn_candidate_identifiers_into_impact_edges() {
    let repo = member_candidate_repo();
    let root = repo.path();
    let refs = cli(root, &["refs", "Runner.invoke"]);
    assert!(
        refs.status.success(),
        "{}",
        String::from_utf8_lossy(&refs.stderr)
    );
    let refs_text = String::from_utf8(refs.stdout).unwrap();
    assert!(refs_text.contains("Likely (1):"), "{refs_text}");
    assert!(refs_text.contains("receiver not typed"), "{refs_text}");
    assert!(
        refs_text.contains("`unknown` [clients.py:4]"),
        "{refs_text}"
    );
    index_import_edge(root, "invoke", "method", "runners.py", "clients.py");

    let output = cli(
        root,
        &["blast-radius", "Runner.invoke", "--include-low-signal"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).unwrap();
    assert!(cli_text.contains("import `invoke`"), "{cli_text}");
    assert!(!cli_text.contains("`unknown`"), "{cli_text}");

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "blast_radius",
        json!({"symbol": "Runner.invoke", "include_low_signal": true}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), cli_text);
}

#[test]
fn one_line_xml_targets_are_visible_through_cli_and_mcp_skeletons() {
    let repo = one_line_xml_repo();
    let root = repo.path();
    let output = cli(root, &["skeleton", "Project.csproj"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_text = String::from_utf8(output.stdout).unwrap();
    assert!(cli_text.contains("anchor_xml"), "{cli_text}");
    assert!(cli_text.contains("caller_xml"), "{cli_text}");

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "file_skeleton",
        json!({"file_path": "Project.csproj"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), cli_text);
}

#[test]
fn css_callee_references_include_proven_call_identifiers_over_cli_and_mcp() {
    let repo = css_call_repo();
    let root = repo.path();
    let output = cli(
        root,
        &[
            "refs",
            "--direction",
            "callees",
            "--file",
            "sample.css",
            "--",
            "--caller_css",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--anchor_css"), "{text}");

    let json_output = cli(
        root,
        &[
            "--json",
            "refs",
            "--direction",
            "callees",
            "--file",
            "sample.css",
            "--",
            "--caller_css",
        ],
    );
    assert!(
        json_output.status.success(),
        "{}",
        String::from_utf8_lossy(&json_output.stderr)
    );
    let rows: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|row| { row["to_symbol_name"] == "--anchor_css" && row["kind"] == "calls" }),
        "{rows}"
    );

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "--caller_css", "file_path": "sample.css", "direction": "callees"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), text);
}

#[test]
fn css_callee_identifier_pages_past_200_work_by_path_and_symbol_id() {
    let repo = css_many_call_repo(220);
    let root = repo.path();
    let first_page = cli(
        root,
        &[
            "refs",
            "--direction",
            "callees",
            "--limit",
            "1",
            "--file",
            "sample.css",
            "--",
            "--caller_css",
        ],
    );
    assert!(
        first_page.status.success(),
        "{}",
        String::from_utf8_lossy(&first_page.stderr)
    );
    let first_text = String::from_utf8(first_page.stdout).unwrap();
    assert!(first_text.contains("(220 found)"), "{first_text}");

    let page = cli(
        root,
        &[
            "refs",
            "--direction",
            "callees",
            "--limit",
            "1",
            "--offset",
            "200",
            "--file",
            "sample.css",
            "--",
            "--caller_css",
        ],
    );
    assert!(
        page.status.success(),
        "{}",
        String::from_utf8_lossy(&page.stderr)
    );
    let name_text = String::from_utf8(page.stdout).unwrap();
    assert!(
        name_text.contains("showing 1 row from 220 queried results"),
        "{name_text}"
    );
    assert!(name_text.contains("--anchor_css"), "{name_text}");

    let db = rusqlite::Connection::open(root.join(".code-kb/artifact.db")).unwrap();
    let caller_id: String = db
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = '--caller_css' AND path = 'sample.css'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(db);
    let id_page = cli(
        root,
        &[
            "refs",
            "--symbol-id",
            &caller_id,
            "--direction",
            "callees",
            "--limit",
            "1",
            "--offset",
            "200",
        ],
    );
    assert!(
        id_page.status.success(),
        "{}",
        String::from_utf8_lossy(&id_page.stderr)
    );
    let id_text = String::from_utf8(id_page.stdout).unwrap();
    assert_eq!(id_text, name_text);

    let mut mcp = McpSession::start(root);
    let by_name = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "--caller_css", "file_path": "sample.css", "direction": "callees", "limit": 1, "offset": 200}),
    );
    assert_ne!(by_name["isError"], true, "{by_name}");
    assert_eq!(result_text(&by_name), name_text);
    let by_id = mcp.call(
        root,
        "find_references",
        json!({"symbol_id": caller_id, "direction": "callees", "limit": 1, "offset": 200}),
    );
    assert_ne!(by_id["isError"], true, "{by_id}");
    assert_eq!(result_text(&by_id), id_text);
}

#[test]
fn css_context_with_a_large_callee_body_keeps_only_the_signature_prefix() {
    let large_value = "x".repeat(1_000_000);
    let anchor_body = format!("result: {large_value};");
    let repo = css_call_repo_with_anchor_body(&anchor_body);
    let root = repo.path();
    let output = cli(
        root,
        &["context", "--file", "sample.css", "--", "--caller_css"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("@function --anchor_css()"), "{text}");
    assert!(!text.contains(&large_value));

    let json_output = cli(
        root,
        &[
            "--json",
            "context",
            "--file",
            "sample.css",
            "--",
            "--caller_css",
        ],
    );
    assert!(
        json_output.status.success(),
        "{}",
        String::from_utf8_lossy(&json_output.stderr)
    );
    let json: Value = serde_json::from_slice(&json_output.stdout).unwrap();
    let json_text = json.to_string();
    assert!(json_text.len() < 16_384, "{} bytes", json_text.len());
    assert!(
        json_text.contains("@function --anchor_css()"),
        "{json_text}"
    );
    assert!(!json_text.contains(&large_value));

    let db = rusqlite::Connection::open(root.join(".code-kb/artifact.db")).unwrap();
    let (stored_signature, start_line, start_byte, end_byte, body_start_byte, body_end_byte): (
        String,
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = db
        .query_row(
            "SELECT signature, start_line, start_byte, end_byte, body_start_byte, body_end_byte
             FROM symbols WHERE name = '--anchor_css' AND path = 'sample.css'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert!(stored_signature.contains(&large_value));
    assert!(stored_signature.len() > 1_000_000);
    assert_eq!(start_line, 1);
    assert!(start_byte < body_start_byte && body_start_byte < body_end_byte);
    assert!(body_end_byte <= end_byte);
    drop(db);

    let mut mcp = McpSession::start(root);
    let result = mcp.call(
        root,
        "get_symbol_context",
        json!({"symbol_name": "--caller_css", "file_path": "sample.css"}),
    );
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), text);
}

#[test]
fn css_call_identifiers_supply_context_and_impact_through_cli_and_mcp() {
    let repo = css_call_repo();
    let root = repo.path();
    let refs = cli(root, &["refs", "--", "--anchor_css"]);
    assert!(
        refs.status.success(),
        "{}",
        String::from_utf8_lossy(&refs.stderr)
    );
    let refs_text = String::from_utf8(refs.stdout).unwrap();
    assert!(refs_text.contains("Proven (1):"), "{refs_text}");
    assert!(refs_text.contains("`--caller_css`"), "{refs_text}");

    let context = cli(
        root,
        &["context", "--file", "sample.css", "--", "--caller_css"],
    );
    assert!(
        context.status.success(),
        "{}",
        String::from_utf8_lossy(&context.stderr)
    );
    let context_text = String::from_utf8(context.stdout).unwrap();
    assert!(
        context_text.contains("@function --anchor_css()"),
        "{context_text}"
    );
    assert!(!context_text.contains("result: 41"), "{context_text}");
    let db = rusqlite::Connection::open(root.join(".code-kb/artifact.db")).unwrap();
    let stored_signature: String = db
        .query_row(
            "SELECT signature FROM symbols WHERE name = '--anchor_css' AND path = 'sample.css'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        stored_signature.contains("result: 41"),
        "{stored_signature}"
    );
    drop(db);

    let context_json = cli(
        root,
        &[
            "--json",
            "context",
            "--file",
            "sample.css",
            "--",
            "--caller_css",
        ],
    );
    assert!(
        context_json.status.success(),
        "{}",
        String::from_utf8_lossy(&context_json.stderr)
    );
    let context_value: Value = serde_json::from_slice(&context_json.stdout).unwrap();
    let context_json_text = context_value.to_string();
    assert!(
        context_json_text.contains("@function --anchor_css()"),
        "{context_json_text}"
    );
    assert!(
        !context_json_text.contains("result: 41"),
        "{context_json_text}"
    );

    let impact = cli(root, &["blast-radius", "--", "--anchor_css"]);
    assert!(
        impact.status.success(),
        "{}",
        String::from_utf8_lossy(&impact.stderr)
    );
    let impact_text = String::from_utf8(impact.stdout).unwrap();
    assert!(impact_text.contains("`--caller_css`"), "{impact_text}");

    let mut mcp = McpSession::start(root);
    let mcp_context = mcp.call(
        root,
        "get_symbol_context",
        json!({"symbol_name": "--caller_css", "file_path": "sample.css"}),
    );
    assert_ne!(mcp_context["isError"], true, "{mcp_context}");
    assert_eq!(result_text(&mcp_context), context_text);
    let mcp_impact = mcp.call(root, "blast_radius", json!({"symbol": "--anchor_css"}));
    assert_ne!(mcp_impact["isError"], true, "{mcp_impact}");
    assert_eq!(result_text(&mcp_impact), impact_text);
}

#[test]
fn css_context_uses_one_real_site_when_same_callee_has_multiple_columns() {
    let repo = css_multi_call_repo();
    let root = repo.path();
    let output = cli(
        root,
        &["context", "--file", "sample.css", "--", "--caller_css"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let context_text = String::from_utf8(output.stdout).unwrap();
    assert!(
        context_text.contains("@function --anchor_css()"),
        "{context_text}"
    );
    assert!(!context_text.contains("result: 41"), "{context_text}");

    let db = rusqlite::Connection::open(root.join(".code-kb/artifact.db")).unwrap();
    let caller_id: String = db
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = '--caller_css' AND path = 'sample.css'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let sites = db
        .prepare(
            "SELECT start_line, start_column FROM identifiers
             WHERE containing_symbol_id = ?1 AND name = '--anchor_css' AND kind = 'call'
             ORDER BY start_line, start_column",
        )
        .unwrap()
        .query_map([caller_id], |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(sites.len(), 2, "{sites:?}");
    assert!(
        sites[0].0 < sites[1].0 && sites[0].1 > sites[1].1,
        "{sites:?}"
    );
    drop(db);

    let mut mcp = McpSession::start(root);
    let context = mcp.call(
        root,
        "get_symbol_context",
        json!({"symbol_name": "--caller_css", "file_path": "sample.css"}),
    );
    assert_ne!(context["isError"], true, "{context}");
    assert_eq!(result_text(&context), context_text);
}

#[test]
fn blast_radius_recommends_and_runs_a_configured_broader_test_command() {
    let repo = package_test_repo();
    let root = repo.path();
    let output = cli(root, &["blast-radius", "helper"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Broader test command: `npm test`"), "{text}");

    let run = Command::new("npm")
        .arg("test")
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains("configured full suite runs"));

    let mut mcp = McpSession::start(root);
    let result = mcp.call(root, "blast_radius", json!({"symbol": "helper"}));
    assert_ne!(result["isError"], true, "{result}");
    assert_eq!(result_text(&result), text);
}

#[test]
fn broader_commands_quote_nested_runner_paths_and_keep_the_runner_working_directory() {
    let repo = nested_package_test_repo();
    let root = repo.path();
    let output = cli(root, &["blast-radius", "helper", "--limit", "1"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("Broader test command: `cd 'packages/test suite' && npm test`"),
        "{text}"
    );

    #[cfg(unix)]
    {
        let command = text
            .lines()
            .find_map(|line| {
                line.strip_prefix("Broader test command: `")
                    .and_then(|line| line.strip_suffix('`'))
            })
            .unwrap();
        let run = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let stdout = String::from_utf8_lossy(&run.stdout);
        assert!(stdout.contains("a configured test runs"), "{stdout}");
        assert!(stdout.contains("b configured test runs"), "{stdout}");
    }
}

#[test]
fn blast_radius_names_missing_runner_evidence_when_no_broader_command_is_known() {
    let repo = unconfigured_test_repo();
    let output = cli(repo.path(), &["blast-radius", "helper"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("Broader scope: full project test suite"),
        "{text}"
    );
    assert!(
        text.contains("No supported runner configuration was found"),
        "{text}"
    );
}

#[test]
fn report_tools_describe_bounded_pages_and_keep_project_root_required() {
    let repo = three_caller_repo();
    let mut mcp = McpSession::start(repo.path());
    let listed = mcp.request("tools/list", json!({}))["result"]["tools"]
        .as_array()
        .unwrap()
        .clone();

    for name in ["find_references", "check_edit", "blast_radius"] {
        let tool = listed.iter().find(|tool| tool["name"] == name).unwrap();
        let schema = &tool["inputSchema"];
        let properties = schema["properties"].as_object().unwrap();
        assert!(
            properties.contains_key("offset"),
            "{name} has no offset control"
        );
        assert_eq!(properties["offset"]["type"], "integer", "{name}");
        assert_eq!(properties["offset"]["minimum"], 0, "{name}");
        assert!(
            properties["offset"]["description"]
                .as_str()
                .unwrap()
                .contains("restart at offset 0"),
            "{name}"
        );
        assert!(
            schema["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|required| required == "project_root"),
            "{name}"
        );
        assert!(
            !["workspace", "workspace_id", "repo_path", "root_dir"]
                .iter()
                .any(|banned| properties.contains_key(*banned))
        );
    }

    let check = listed
        .iter()
        .find(|tool| tool["name"] == "check_edit")
        .unwrap();
    assert_eq!(check["inputSchema"]["properties"]["limit"]["maximum"], 200);
    let blast = listed
        .iter()
        .find(|tool| tool["name"] == "blast_radius")
        .unwrap();
    assert_eq!(
        blast["inputSchema"]["properties"]["include_low_signal"]["type"],
        "boolean"
    );
    for name in ["find_references", "check_edit", "blast_radius"] {
        let tool = listed.iter().find(|tool| tool["name"] == name).unwrap();
        assert!(
            tool["inputSchema"]["properties"]["offset"]
                .get("maximum")
                .is_none()
        );
    }
}

#[test]
fn report_page_controls_allow_large_offsets_and_reject_overflow_over_cli_and_mcp() {
    let repo = changed_repo();
    let root = repo.path();

    let output = cli(root, &["refs", "helper", "--limit", "0", "--offset", "999"]);
    assert!(output.status.success());
    let cli_zero_refs = String::from_utf8(output.stdout).unwrap();
    assert!(cli_zero_refs.contains("Result limit is 0"));

    let output = cli(
        root,
        &["check", "--base", "HEAD", "--limit", "0", "--offset", "999"],
    );
    assert!(output.status.success());
    let cli_zero_check = String::from_utf8(output.stdout).unwrap();

    let output = cli(
        root,
        &["blast-radius", "helper", "--limit", "0", "--offset", "999"],
    );
    assert!(output.status.success());
    let cli_zero_blast = String::from_utf8(output.stdout).unwrap();

    let output = cli(
        root,
        &["refs", "helper", "--limit", "1", "--offset", "1000"],
    );
    assert!(output.status.success());
    let cli_past_refs = String::from_utf8(output.stdout).unwrap();
    assert!(
        cli_past_refs.contains("Page offset 1000"),
        "{cli_past_refs}"
    );

    let output = cli(
        root,
        &[
            "check", "--base", "HEAD", "--limit", "1", "--offset", "1000",
        ],
    );
    assert!(output.status.success());
    let cli_past_check = String::from_utf8(output.stdout).unwrap();
    assert!(
        cli_past_check.contains("Page offset 1000"),
        "{cli_past_check}"
    );

    let output = cli(
        root,
        &["blast-radius", "helper", "--limit", "1", "--offset", "1000"],
    );
    assert!(output.status.success());
    let cli_past_blast = String::from_utf8(output.stdout).unwrap();
    assert!(cli_past_blast.contains("offset 1000"), "{cli_past_blast}");

    let max_offset = u64::MAX.to_string();
    let output = cli(
        root,
        &["refs", "helper", "--limit", "1", "--offset", &max_offset],
    );
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("overflows"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_overflow_refs = String::from_utf8_lossy(&output.stderr).to_string();

    let mut cli_overflow_reports = Vec::new();
    for (cli_args, tool, arguments) in [
        (
            vec![
                "check",
                "--base",
                "HEAD",
                "--limit",
                "1",
                "--offset",
                max_offset.as_str(),
            ],
            "check_edit",
            json!({"base": "HEAD", "limit": 1, "offset": u64::MAX}),
        ),
        (
            vec![
                "blast-radius",
                "helper",
                "--limit",
                "1",
                "--offset",
                max_offset.as_str(),
            ],
            "blast_radius",
            json!({"symbol": "helper", "limit": 1, "offset": u64::MAX}),
        ),
    ] {
        let output = cli(root, &cli_args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("overflows"));
        cli_overflow_reports.push((
            tool,
            arguments,
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }

    let mut mcp = McpSession::start(root);
    for (tool, arguments, cli_text) in [
        (
            "find_references",
            json!({"symbol_name": "helper", "limit": 0, "offset": 999}),
            cli_zero_refs.as_str(),
        ),
        (
            "check_edit",
            json!({"base": "HEAD", "limit": 0, "offset": 999}),
            cli_zero_check.as_str(),
        ),
        (
            "blast_radius",
            json!({"symbol": "helper", "limit": 0, "offset": 999}),
            cli_zero_blast.as_str(),
        ),
        (
            "find_references",
            json!({"symbol_name": "helper", "limit": 1, "offset": 1000}),
            cli_past_refs.as_str(),
        ),
        (
            "check_edit",
            json!({"base": "HEAD", "limit": 1, "offset": 1000}),
            cli_past_check.as_str(),
        ),
        (
            "blast_radius",
            json!({"symbol": "helper", "limit": 1, "offset": 1000}),
            cli_past_blast.as_str(),
        ),
    ] {
        let result = mcp.call(root, tool, arguments);
        assert_ne!(result["isError"], true, "{result}");
        assert_eq!(result_text(&result), cli_text);
    }
    let invalid_window = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "helper", "limit": 1, "offset": u64::MAX}),
    );
    assert_eq!(invalid_window["isError"], true, "{invalid_window}");
    assert!(result_text(&invalid_window).contains("overflows"));
    assert!(cli_overflow_refs.contains("overflows"));
    for (tool, arguments, cli_text) in cli_overflow_reports {
        let result = mcp.call(root, tool, arguments);
        assert_eq!(result["isError"], true, "{result}");
        assert!(result_text(&result).contains("overflows"));
        assert!(cli_text.contains("overflows"));
    }

    let invalid_offset = mcp.call(
        root,
        "find_references",
        json!({"symbol_name": "helper", "offset": "1"}),
    );
    assert_eq!(invalid_offset["isError"], true, "{invalid_offset}");
    assert!(result_text(&invalid_offset).contains("Invalid offset"));
}
