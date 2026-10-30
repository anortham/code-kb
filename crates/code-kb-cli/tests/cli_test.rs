use std::process::Command;

#[cfg(unix)]
fn extractor_wrapper_failing_update(
    dir: &std::path::Path,
    active_marker: &std::path::Path,
    failure_record: &std::path::Path,
    rel_path: &str,
) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let real = code_kb_core::find_julie_extract_binary().expect("julie-extract is installed");
    let wrapper = dir.join("julie-extract-failing-update");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ -e '{}' ] && [ \"$1\" = update ]; then\n  for arg in \"$@\"; do\n    if [ \"$arg\" = '{}' ]; then : > '{}'; echo 'controlled incremental failure' >&2; exit 2; fi\n  done\nfi\nexec '{}' \"$@\"\n",
            active_marker.display(),
            rel_path,
            failure_record.display(),
            real.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let output = Command::new(&wrapper).arg("--version").output().unwrap();
    assert!(output.status.success(), "{output:?}");
    wrapper
}

#[test]
fn cli_references_and_blast_radius_handle_lambdas_and_lambda_named_functions() {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
    for (path, source) in [
        (
            "pyproject.toml",
            "[project]\nname = 'lambda-regression'\nversion = '0.1.0'\n",
        ),
        (
            "flask.py",
            "class Flask:\n    def __init__(self, name):\n        pass\n",
        ),
        (
            "app.py",
            "from flask import Flask\nmodule_app = lambda: Flask(\"module\")\ndef outer():\n    def lambda_123():\n        return Flask(\"real function\")\n    return lambda_123()\n",
        ),
    ] {
        std::fs::write(root.join(path), source).unwrap();
    }
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };

    let refs_text = String::from_utf8(run(&["refs", "Flask"])).unwrap();
    assert!(refs_text.contains("`lambda_123`"), "{refs_text}");
    assert!(!refs_text.contains("<lambda> in outer"), "{refs_text}");
    let refs_json: serde_json::Value =
        serde_json::from_slice(&run(&["--json", "refs", "Flask"])).unwrap();
    let sites = refs_json["sites"].as_array().unwrap();
    assert!(
        sites
            .iter()
            .any(|row| row["from_symbol_name"] == "lambda_123")
    );
    let module_lambda = sites
        .iter()
        .find_map(|row| {
            row["from_symbol_name"]
                .as_str()
                .filter(|name| name.starts_with("lambda_") && *name != "lambda_123")
        })
        .expect("JSON refs should preserve the module-level lambda's raw name");
    assert!(
        refs_text.contains(&format!("`{module_lambda}`")),
        "{refs_text}"
    );

    let blast_text = String::from_utf8(run(&["blast-radius", "Flask"])).unwrap();
    assert!(blast_text.contains("`lambda_123`"), "{blast_text}");
    assert!(!blast_text.contains("<lambda> in outer"), "{blast_text}");
    let blast_json: serde_json::Value =
        serde_json::from_slice(&run(&["--json", "blast-radius", "Flask"])).unwrap();
    let impacted = blast_json["impacted_symbols"].as_array().unwrap();
    assert!(impacted.iter().any(|row| row["name"] == "lambda_123"));
    let module_lambda = impacted
        .iter()
        .find_map(|row| {
            row["name"]
                .as_str()
                .filter(|name| name.starts_with("lambda_") && *name != "lambda_123")
        })
        .expect("JSON blast radius should preserve the module-level lambda's raw name");
    assert!(
        blast_text.contains(&format!("`{module_lambda}`")),
        "{blast_text}"
    );
}

#[test]
fn cli_blast_radius_lists_no_test_that_builds_the_app_and_calls_it_through_a_client() {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
    for (path, source) in [
        (
            "pyproject.toml",
            "[project]\nname = 'handler-ranking'\nversion = '0.1.0'\n",
        ),
        (
            "src/app.py",
            "class App:\n    def __call__(self, environ):\n        return self.handle_user_exception(environ)\n\n    def handle_user_exception(self, error):\n        return error\n\n    def register_error_handler(self, error, handler):\n        return None\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\nfrom app import App\n\n@pytest.fixture\ndef app():\n    return App()\n",
        ),
        (
            "tests/test_runtime.py",
            "from flask import abort\n\n\ndef test_alpha(app):\n    @app.errorhandler(ValueError)\n    def on_error(error):\n        return str(error)\n\n    @app.route('/')\n    def index():\n        return 'ok'\n\n    app.test_client().get('/')\n\n\ndef test_beta(app):\n    @app.route('/')\n    def index():\n        abort(400)\n\n    app.test_client().get('/')\n\n\ndef test_gamma(app):\n    def on_error(error):\n        return str(error)\n\n    app.register_error_handler(ValueError, on_error)\n    app.test_client().get('/')\n",
        ),
        (
            "tests/test_name_match.py",
            "def test_handle_user_exception(app):\n    app.test_client().get('/')\n",
        ),
    ] {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), source).unwrap();
    }

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args([
            "--json",
            "blast-radius",
            "App.handle_user_exception",
            "--depth",
            "3",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["likely_tests"], serde_json::json!([]), "{result:#}");

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(["blast-radius", "App.handle_user_exception", "--depth", "3"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("No test reaches it through the index. Run the full test suite."),
        "{text}"
    );
    assert!(!text.contains("possible"), "{text}");
}

#[test]
fn receiver_reference_labels_match_the_cli_contract() {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
    for (path, source) in [
        (
            "pyproject.toml",
            "[project]\nname = 'receivers'\nversion = '0.1.0'\n",
        ),
        (
            "runners.py",
            "class Runner:\n    def invoke(self):\n        pass\n\nclass Shell:\n    def invoke(self):\n        pass\n",
        ),
        (
            "client.py",
            "from runners import Runner, Shell\n\ndef supported():\n    receiver = Runner()\n    return receiver.invoke\n\ndef wrong():\n    receiver = Shell()\n    return receiver.invoke\n\ndef unknown(receiver):\n    return receiver.invoke\n",
        ),
    ] {
        std::fs::write(root.join(path), source).unwrap();
    }
    let run = |json| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_code-kb"));
        command
            .arg("--root")
            .arg(root)
            .args(["refs", "Runner.invoke"]);
        if json {
            command.arg("--json");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    };
    let text = String::from_utf8(run(false)).unwrap();
    assert!(
        text.contains(
            "Proven (1):\n- `supported` [client.py:5] (kind: member_access; builds the class)\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "Unresolved (1):\n- `unknown` [client.py:12] (kind: member_access; 2 definitions)\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "Excluded (1):\n- `wrong` [client.py:9] (kind: member_access; resolves to `Shell.invoke` (runners.py:6))\n"
        ),
        "{text}"
    );
    let report: serde_json::Value = serde_json::from_slice(&run(true)).unwrap();
    let rows = report["sites"].as_array().unwrap();
    let row = |caller: &str| {
        rows.iter()
            .find(|row| row["from_symbol_name"] == caller)
            .unwrap_or_else(|| panic!("{caller} missing from {rows:?}"))
    };
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(row("supported")["label"], "proven");
    assert_eq!(row("unknown")["label"], "unresolved");
    assert_eq!(row("unknown")["basis"], "2 definitions");
    assert_eq!(row("wrong")["label"], "excluded");
    assert_eq!(row("wrong")["kind"], "member_access");
}

#[test]
fn cli_renders_reference_sites_without_enclosing_symbols() {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
    for (path, source) in [
        (
            "pyproject.toml",
            "[project]\nname = 'callerless-reference'\nversion = '0.1.0'\n",
        ),
        (
            "runners.py",
            "class Runner:\n    def Contains(self, value):\n        return value\n",
        ),
        (
            "client.py",
            "def caller(value):\n    return value.Contains\n",
        ),
    ] {
        std::fs::write(root.join(path), source).unwrap();
    }

    let db = root.join(".code-kb/artifact.db");
    code_kb_core::scan_workspace(&code_kb_core::Workspace::new(root.to_path_buf()), &db, true)
        .unwrap();
    let conn = code_kb_core::open_read_write(&db).unwrap();
    let changed = conn
        .execute(
            "UPDATE identifiers
             SET containing_symbol_id = NULL, metadata_json = '{}'
             WHERE path = 'client.py' AND name = 'Contains' AND kind = 'member_access'",
            [],
        )
        .unwrap();
    assert_eq!(changed, 1, "expected one extracted member access");
    conn.execute(
        "DELETE FROM relationships WHERE path = 'client.py' AND start_line = 2",
        [],
    )
    .unwrap();
    conn.execute(
        "DELETE FROM pending_relationships WHERE path = 'client.py' AND start_line = 2",
        [],
    )
    .unwrap();
    drop(conn);

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(["refs", "Contains"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains(
            "Likely (1):\n- (no enclosing symbol) [client.py:2] (kind: member_access; name only)\n"
        ),
        "{text}"
    );
}

fn three_caller_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
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
        std::fs::write(root.join(path), source).unwrap();
    }
    repo
}

fn run_refs(root: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn cli_refs_text_lists_up_to_the_limit_and_folds_the_rest_with_complete_counts() {
    let repo = three_caller_repo();

    let text = run_refs(repo.path(), &["refs", "helper", "--limit", "1"]);

    assert_eq!(
        text,
        "References to function `helper` (util.py:1)\n\
         Sites: 3 · proven 3 · likely 0 · unresolved 0 · excluded 0\n\
         Text: `helper` is on 6 lines: 3 sites, 1 definition, 1 import, 1 text only.\n\
         \n\
         Proven (3):\n\
         - `a` [app.py:5] (kind: calls; import; 0 args)\n\
         … 2 more proven sites in 1 file: app.py (2)\n\
         Text only (1): docs, comments, strings, or code the index did not record\n\
         \x20 notes.md: 1\n\
         Not visible to this index: calls through getattr() or names built at runtime.\n\
         Imported 1 time in 1 file: app.py:1 (lookup_symbol with kind=\"import\" lists them)\n"
    );
}

#[test]
fn cli_refs_json_prints_the_whole_labeled_report_whatever_the_limit() {
    let repo = three_caller_repo();

    let raw = run_refs(repo.path(), &["--json", "refs", "helper", "--limit", "1"]);

    let report: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(report["target"]["name"], "helper", "{raw}");
    assert_eq!(report["target"]["path"], "util.py", "{raw}");
    assert_eq!(report["counts"]["sites"], 3, "{raw}");
    assert_eq!(report["counts"]["proven"], 3, "{raw}");
    assert_eq!(report["counts"]["text_lines"], 6, "{raw}");
    assert_eq!(report["counts"]["text_only_lines"], 1, "{raw}");
    let sites = report["sites"].as_array().unwrap();
    assert_eq!(sites.len(), 3, "{raw}");
    for (site, (caller, line)) in sites.iter().zip([("a", 5), ("b", 9), ("c", 13)]) {
        assert_eq!(site["from_symbol_name"], caller, "{raw}");
        assert_eq!(site["start_line"], line, "{raw}");
        assert_eq!(site["path"], "app.py", "{raw}");
        assert_eq!(site["label"], "proven", "{raw}");
        assert_eq!(site["basis"], "import", "{raw}");
    }
    assert_eq!(
        report["text_only"],
        serde_json::json!([{ "path": "notes.md", "lines": [1] }]),
        "{raw}"
    );
    assert_eq!(report["text_scan"]["complete"], true, "{raw}");
    assert!(report["text_scan"]["files_scanned"].as_u64().unwrap() >= 4);
    assert!(!raw.contains("\\\\"), "{raw}");
}

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

    conn.execute_batch(
        "ALTER TABLE pending_relationships ADD COLUMN pending_relationship_id TEXT;
         ALTER TABLE pending_relationships ADD COLUMN caller_scope_symbol_id TEXT;
         ALTER TABLE pending_relationships ADD COLUMN file_id TEXT;
         ALTER TABLE pending_relationships ADD COLUMN target_display_name TEXT;
         ALTER TABLE pending_relationships ADD COLUMN target_receiver TEXT;
         ALTER TABLE pending_relationships ADD COLUMN target_namespace_json TEXT;
         ALTER TABLE pending_relationships ADD COLUMN target_import_context TEXT;
         ALTER TABLE pending_relationships ADD COLUMN end_line INTEGER;
         ALTER TABLE pending_relationships ADD COLUMN metadata_json TEXT;
         ALTER TABLE relationships ADD COLUMN file_id TEXT;
         ALTER TABLE relationships ADD COLUMN end_line INTEGER;
         ALTER TABLE relationships ADD COLUMN metadata_json TEXT;
         ALTER TABLE symbols ADD COLUMN metadata_json TEXT;
         CREATE TABLE identifiers (
            identifier_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT,
            kind TEXT, containing_symbol_id TEXT, start_line INTEGER, start_column INTEGER,
            end_line INTEGER, end_column INTEGER, metadata_json TEXT
        );",
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
fn test_cli_json_skeleton_of_a_folder_lists_its_files() {
    let repo = setup_test_repo();
    let root = repo.path();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("--json")
        .arg("skeleton")
        .arg("src")
        .output()
        .expect("Failed to execute skeleton");

    assert!(output.status.success());
    let files: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let paths: Vec<&str> = files
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|file| file["path"].as_str())
        .collect();
    assert!(paths.contains(&"src/workspace.rs"), "{paths:?}");
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
    assert_eq!(json_val["names"].as_array().unwrap().len(), 1);
    assert_eq!(json_val["names"][0]["name"], "Workspace");
    assert!(json_val["ranked"].as_array().unwrap().is_empty());

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

fn mcp_lookup_texts(root: &std::path::Path, queries: &[&str]) -> Vec<String> {
    use std::io::{BufRead, Write};
    let telemetry = code_kb_core::safe_tempdir();
    let mut child = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("CODE_KB_TELEMETRY_DIR", telemetry.path())
        .arg("serve")
        .arg("--root")
        .arg(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("Failed to spawn code-kb serve");
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut requests = vec![
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                   "clientInfo": {"name": "parity", "version": "1.0"}}}),
    ];
    for (id, query) in queries.iter().enumerate() {
        requests.push(serde_json::json!({"jsonrpc": "2.0", "id": id + 2, "method": "tools/call",
            "params": {"name": "lookup_symbol", "arguments": {"query": query, "project_root": root}}}));
    }
    let mut texts = Vec::new();
    for request in requests {
        stdin.write_all(format!("{request}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: serde_json::Value = serde_json::from_str(&line).unwrap();
        if response["id"] != 1 {
            texts.push(
                response["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
    }
    drop(stdin);
    let _ = child.wait();
    texts
}

fn mcp_lookup_names(root: &std::path::Path, query: &str) -> Vec<String> {
    mcp_lookup_texts(root, &[query])
        .remove(0)
        .lines()
        .filter(|line| line.starts_with("- "))
        .map(|line| line.split('`').nth(1).unwrap().to_string())
        .collect()
}

#[test]
fn test_cli_lookup_prints_the_text_of_the_mcp_lookup_symbol_for_a_concept_an_exact_name_and_a_miss()
{
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/net.rs"),
        "pub fn retry_count() -> u32 {\n    3\n}\n\n/// Send the request again; retry with backoff.\npub fn send_again() {}\n\npub fn backoff() {}\n",
    )
    .unwrap();
    let queries = [
        "retry",
        "send_again",
        "bakoff",
        "fn send_again()",
        "function that retries",
        "send_agian",
    ];

    let cli_texts: Vec<String> = queries
        .iter()
        .map(|query| {
            let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
                .arg("--root")
                .arg(root)
                .args(["lookup", query])
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).to_string()
        })
        .collect();
    let mcp_texts = mcp_lookup_texts(root, &queries);

    assert_eq!(cli_texts.len(), 6);
    for (query, (cli, mcp)) in queries.iter().zip(cli_texts.iter().zip(&mcp_texts)) {
        assert_eq!(cli, mcp, "query {query}");
    }
    assert!(
        mcp_texts[0].contains("No exact name match; 1 full-text matches for \"retry\":"),
        "{}",
        mcp_texts[0]
    );
    assert!(mcp_texts[0].contains("`retry_count`") && mcp_texts[0].contains("`send_again`"));
    assert!(
        !mcp_texts[1].contains("No exact name match"),
        "{}",
        mcp_texts[1]
    );
    assert!(
        mcp_texts[2].contains("Did you mean one of:"),
        "{}",
        mcp_texts[2]
    );
    assert!(mcp_texts[2].contains("`backoff`"), "{}", mcp_texts[2]);
    assert_eq!(mcp_texts[3], mcp_texts[1]);
    assert!(
        mcp_texts[4].contains("full-text matches for \"function that retries\":"),
        "{}",
        mcp_texts[4]
    );

    let json = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(["--json", "lookup", "retry"])
        .output()
        .unwrap();
    assert!(json.status.success());
    let lookup: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(lookup["names"][0]["name"], "retry_count");
    assert_eq!(lookup["ranked"][0]["symbol"]["name"], "send_again");
    assert!(lookup.get("suggestions").is_none(), "{lookup}");

    let hint = mcp_texts[5]
        .find("Did you mean one of:\n  - function `send_again` (src/net.rs:6)")
        .unwrap_or_else(|| panic!("{}", mcp_texts[5]));
    let header = mcp_texts[5]
        .find("full-text matches for \"send_agian\":")
        .unwrap_or_else(|| panic!("{}", mcp_texts[5]));
    assert!(hint < header, "{}", mcp_texts[5]);
    let json = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(["--json", "lookup", "send_agian"])
        .output()
        .unwrap();
    let typo: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(typo["suggestions"][0]["name"], "send_again");
    assert!(typo.get("miss").is_none(), "{typo}");
}

#[test]
fn test_cli_search_order_matches_the_mcp_lookup_symbol_fallback() {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"lookup_parity\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub struct Workspace;\npub fn run_task() { helper(); }\nfn helper() {}\n",
    )
    .unwrap();
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
    assert_eq!(cli_names, mcp_lookup_names(root, query));
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
    let id = serde_json::from_slice::<serde_json::Value>(&lookup.stdout).unwrap()["names"][0]["symbol_id"]
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
    std::fs::remove_dir_all(root.join(".code-kb")).unwrap();
    let scan = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("scan")
        .output()
        .unwrap();
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
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
    let lookup: serde_json::Value = serde_json::from_slice(&lookup.stdout).unwrap();
    let rows = lookup["names"].as_array().unwrap();
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
    for refs in &outputs[1] {
        assert!(
            refs.contains("- `first` [src/overloads.cpp:2] (kind: calls;"),
            "{refs}"
        );
        assert!(
            refs.contains("- `second` [src/overloads.cpp:3] (kind: calls;"),
            "{refs}"
        );
    }
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
    assert!(
        stdout
            .contains("Proven (1):\n- `run_task` [src/workspace.rs:6] (kind: calls; same file)\n"),
        "{stdout}"
    );

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
    assert!(
        stdout
            .contains("Proven (1):\n- `run_task` [src/workspace.rs:6] (kind: calls; same file)\n"),
        "{stdout}"
    );

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
    assert_eq!(short_stdout, stdout);
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
    assert!(result.get("test_file_ceiling_reached").is_none());
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

#[cfg(unix)]
#[test]
fn test_cli_never_runs_the_julie_extract_of_the_directory_it_starts_in() {
    use std::os::unix::fs::PermissionsExt;

    let repo = tempfile::TempDir::new_in(std::env::temp_dir()).unwrap();
    let root = repo.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn own_fn() {}\n").unwrap();
    let foreign_tools = root.join(".tools");
    std::fs::create_dir_all(&foreign_tools).unwrap();
    let marker = foreign_tools.join("ran");
    let foreign_extractor = foreign_tools.join("julie-extract");
    std::fs::write(
        &foreign_extractor,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'julie-extract 0.0.1'; exit 0; fi\ntouch '{}'\nexit 1\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&foreign_extractor, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .current_dir(root)
        .env_remove("JULIE_EXTRACT_BIN")
        .env("PATH", "/usr/bin:/bin")
        .env("CODE_KB_TELEMETRY_DIR", root.join(".telemetry_test"))
        .arg("--root")
        .arg(root)
        .arg("scan")
        .output()
        .unwrap();

    assert!(
        !marker.exists(),
        "code-kb ran the extractor found in its working directory"
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(root.join(".code-kb/artifact.db").is_file());
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
    assert!(stdout_cat.contains("`toml.key_value.v1` (1 fact)"));
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
fn cli_facts_orders_same_line_flask_routes_by_template_and_method() {
    let repo = setup_test_repo();
    let root = repo.path();

    let db_path = root.join(".code-kb/artifact.db");
    let conn = code_kb_core::open_read_write(&db_path).unwrap();
    conn.execute_batch(
        "INSERT INTO structural_facts VALUES
            ('sf_post_create', 'f1', 'examples/tutorial/routes.py', 'python', 'flask.route.v1', 'create', 'decorated_definition', NULL, 10, 10, 1.0, '{\"verb\":\"POST\",\"route_template\":\"/create\",\"normalized_route_template\":\"/create\"}'),
            ('sf_get_create', 'f1', 'examples/tutorial/routes.py', 'python', 'flask.route.v1', 'create', 'decorated_definition', NULL, 10, 10, 1.0, '{\"verb\":\"GET\",\"route_template\":\"/create\",\"normalized_route_template\":\"/create\"}'),
            ('sf_get_update', 'f1', 'examples/tutorial/routes.py', 'python', 'flask.route.v1', 'update', 'decorated_definition', NULL, 10, 10, 1.0, '{\"verb\":\"GET\",\"route_template\":\"/<int:id>/update\",\"normalized_route_template\":\"/:id/update\"}'),
            ('sf_post_update', 'f1', 'examples/tutorial/routes.py', 'python', 'flask.route.v1', 'update', 'decorated_definition', NULL, 10, 10, 1.0, '{\"verb\":\"POST\",\"route_template\":\"/<int:id>/update\",\"normalized_route_template\":\"/:id/update\"}');",
    )
    .unwrap();
    drop(conn);

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .args(["facts", "route", "--path", "examples/tutorial"])
        .output()
        .expect("Failed to execute route facts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let route_order = [
        "GET /<int:id>/update",
        "POST /<int:id>/update",
        "GET /create",
        "POST /create",
    ]
    .map(|route| {
        stdout
            .find(route)
            .unwrap_or_else(|| panic!("missing {route}: {stdout}"))
    });
    assert!(
        route_order.windows(2).all(|pair| pair[0] < pair[1]),
        "{stdout}"
    );
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
    assert!(ctx.contains("Code navigation: use the `code-kb` MCP tools first"));
    assert!(ctx.contains("`project_root`"));
    assert!(ctx.contains("on every code-kb call."));
    assert!(ctx.contains(
        "- **To find code by concept**, call `lookup_symbol` with a few words; it adds ranked matches when no name matches."
    ));
    assert!(
        ctx.contains("- **After you edit**, call `check_edit` before you say the change is done.")
    );
    assert!(!ctx.contains("telemetry_summary"));
    assert!(!ctx.contains("search_symbols"));
    assert!(!ctx.contains("EnterWorktree"));
    assert!(!ctx.contains("Binding is server-wide"));
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
    assert!(ctx.contains("Code navigation: use the `code-kb` MCP tools first"));
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
    assert!(ctx.contains("Code navigation: use the `code-kb` MCP tools first"));

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
    assert!(msg.contains("Code navigation: use the `code-kb` MCP tools first"));
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
fn test_skills_follow_agent_skill_frontmatter_rules() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let mut skills: Vec<_> = std::fs::read_dir(root.join("skills"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    skills.sort();
    assert_eq!(skills, ["code-kb", "code-kb-telemetry", "report-issue"]);
    for skill in skills {
        let path = format!("skills/{skill}/SKILL.md");
        let text = std::fs::read_to_string(root.join(&path))
            .unwrap()
            .replace("\r\n", "\n");
        let (frontmatter, body) = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .unwrap_or_else(|| panic!("{path} has no frontmatter"));
        let field = |key: &str| {
            frontmatter
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{key}: ")))
                .unwrap_or_else(|| panic!("{path} has no {key}"))
        };
        let name = field("name");
        assert_eq!(name, skill, "{path} name must match its directory");
        assert!(
            name.len() <= 64
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "{path} name must be at most 64 lowercase letters, digits, or hyphens"
        );
        let description = field("description");
        assert!(
            !description.is_empty() && description.len() <= 1024,
            "{path} description must hold 1 to 1024 characters"
        );
        assert!(
            !description.contains(": ") && !description.contains('<'),
            "{path} description must stay a plain YAML scalar with no XML tag"
        );
        assert!(
            body.lines().count() < 500,
            "{path} body must stay under 500 lines"
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
        if cmd_args == ["--json", "refs", "helper"] {
            let sites = val["sites"].as_array().expect("refs JSON is a report");
            assert_eq!(sites.len(), 1, "{val}");
            assert_eq!(sites[0]["from_symbol_name"], "run_task", "{val}");
            assert_eq!(sites[0]["path"], "src/workspace.rs", "{val}");
            assert_eq!(sites[0]["label"], "proven", "{val}");
            assert!(found > 0, "{val}");
        }
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
fn test_cli_scan_rebuilds_an_index_the_extractor_must_migrate() {
    use std::os::unix::fs::PermissionsExt;
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
    let db = root.join(".code-kb").join("artifact.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    std::fs::write(&db, "schema 7").unwrap();
    let args_log = root.join("extractor-args.txt");
    let fake = root.join("fake-julie-extract");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version) echo 'julie-extract {}'; exit 0;; esac\nfor arg in \"$@\"; do [ \"$arg\" = --level ] && {{ printf '%s\\n' \"$@\" > \"$EXTRACTOR_ARGS_LOG\"; exit 0; }}; done\necho 'schema_migration_required: artifact schema migration is required' >&2\nexit 3\n",
            code_kb_core::PINNED_JULIE_VERSION
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("JULIE_EXTRACT_BIN", &fake)
        .env("EXTRACTOR_ARGS_LOG", &args_log)
        .current_dir(root)
        .arg("scan")
        .output()
        .expect("Failed to run scan");

    let args = std::fs::read_to_string(&args_log).expect("scan must rebuild the index");
    assert!(args.lines().any(|arg| arg == "--level"), "{args}");
    let side_file = args
        .lines()
        .skip_while(|arg| *arg != "--db")
        .nth(1)
        .unwrap_or_default();
    assert!(side_file.contains("artifact.db.rescan-"), "{args}");
    assert!(!std::path::Path::new(side_file).exists());
    assert_eq!(
        std::fs::read(&db).unwrap(),
        b"schema 7",
        "a rebuild that writes no index must keep the old file"
    );
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
fn test_cli_scan_requests_the_references_extraction_level() {
    let (_repo, args, _) = scan_with_fake_extractor();
    let idx = args
        .iter()
        .position(|a| a == "--level")
        .expect("scan must pass --level");
    assert_eq!(args[idx + 1], "references");
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

#[cfg(unix)]
#[test]
fn test_cli_offline_reconcile_failure_does_not_return_stale_results() {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn old_symbol() {}\n").unwrap();

    let workspace = code_kb_core::Workspace::new(root.to_path_buf());
    let db_path = root.join(".code-kb/artifact.db");
    code_kb_core::scan_workspace(&workspace, &db_path, true).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn current_symbol() {}\n").unwrap();

    let tools = tempfile::tempdir().unwrap();
    let active_marker = tools.path().join("fail-refresh");
    let failure_record = tools.path().join("failed-path");
    std::fs::write(&active_marker, "fail").unwrap();
    let wrapper = extractor_wrapper_failing_update(
        tools.path(),
        &active_marker,
        &failure_record,
        "src/lib.rs",
    );

    let failed = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("JULIE_EXTRACT_BIN", &wrapper)
        .arg("--root")
        .arg(root)
        .arg("lookup")
        .arg("old_symbol")
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&failed.stderr);
    assert!(
        !failed.status.success(),
        "{}",
        String::from_utf8_lossy(&failed.stdout)
    );
    assert!(
        failed.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&failed.stdout)
    );
    assert!(error.contains(&root.display().to_string()), "{error}");
    assert!(error.contains("controlled incremental failure"), "{error}");
    assert!(error.contains("code-kb scan --root"), "{error}");

    let conn = code_kb_core::open_read_only(&db_path).unwrap();
    assert!(
        code_kb_core::get_symbol_by_name(&conn, "old_symbol", Some("src/lib.rs"))
            .unwrap()
            .is_some()
    );
    assert!(
        code_kb_core::get_symbol_by_name(&conn, "current_symbol", Some("src/lib.rs"))
            .unwrap()
            .is_none()
    );
    drop(conn);

    std::fs::remove_file(active_marker).unwrap();
    let recovered = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("JULIE_EXTRACT_BIN", &wrapper)
        .arg("--root")
        .arg(root)
        .arg("lookup")
        .arg("current_symbol")
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    assert!(String::from_utf8_lossy(&recovered.stdout).contains("current_symbol"));
}

#[test]
fn test_cli_query_in_a_markerless_folder_under_a_dotfiles_home_does_not_index_the_home() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".git")).unwrap();
    let notes = home.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join("todo.py"), "def remember():\n    pass\n").unwrap();
    let telemetry = tempfile::tempdir().unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .current_dir(&notes)
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("CODE_KB_TELEMETRY_DIR", telemetry.path())
        .args(["lookup", "remember"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains(HOME_REFUSAL), "{stderr}");
    assert!(!home.path().join(".code-kb").join("artifact.db").exists());
}

const HOME_REFUSAL: &str = "is refused: it is the home directory. Run the command inside a project or pass --root <project path>.";

#[test]
fn test_cli_query_refuses_a_leftover_home_index_and_leaves_it_unchanged() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".git")).unwrap();
    std::fs::write(
        home.path().join("dotfiles.py"),
        "def remember():\n    pass\n",
    )
    .unwrap();
    let notes = home.path().join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    let telemetry = tempfile::tempdir().unwrap();
    let code_kb = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_code-kb"));
        command
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("CODE_KB_TELEMETRY_DIR", telemetry.path());
        command
    };
    let scan = code_kb()
        .arg("--root")
        .arg(home.path())
        .arg("scan")
        .output()
        .unwrap();
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let index = home.path().join(".code-kb").join("artifact.db");
    let before = std::fs::read(&index).unwrap();

    let out = code_kb()
        .arg("--root")
        .arg(&notes)
        .args(["lookup", "remember"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.starts_with("Error: project_root '"), "{stderr}");
    assert!(stderr.contains(HOME_REFUSAL), "{stderr}");
    assert!(std::fs::read(&index).unwrap() == before);
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

fn collect_guidance_files(path: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        files.push(path.to_path_buf());
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let skipped = matches!(
            entry.file_name().to_str(),
            Some("__pycache__" | "reach" | "results")
        );
        if !skipped {
            collect_guidance_files(&entry.path(), files);
        }
    }
}

fn names_removed_mcp_tool(line: &str) -> Option<&'static str> {
    const REMOVED: [&str; 4] = [
        "search_symbols",
        "codebase_outline",
        "find_structural_facts",
        "telemetry_summary",
    ];
    let bytes = line.as_bytes();
    REMOVED.into_iter().find(|tool| {
        line.match_indices(tool).any(|(start, _)| {
            let continues_after = bytes
                .get(start + tool.len())
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_');
            let continues_before = start > 0
                && (bytes[start - 1].is_ascii_alphanumeric()
                    || (bytes[start - 1] == b'_' && !line[..start].ends_with("__")));
            !continues_after && !continues_before
        })
    })
}

#[test]
fn test_guidance_names_no_removed_mcp_tool_outside_a_removal_note() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let mut files = Vec::new();
    for entry in [
        "README.md",
        "CLAUDE.md",
        "AGENTS.md",
        "hooks",
        "skills",
        "evals",
        "scripts",
        "docs/site",
        "docs/decisions",
        "docs/toolkit-assessment.md",
    ] {
        collect_guidance_files(&root.join(entry), &mut files);
    }
    assert!(
        files.len() > 20,
        "guidance walk found {} files",
        files.len()
    );
    let mut offenders = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if let Some(tool) = names_removed_mcp_tool(line)
                && !line.contains("removed")
                && !line.contains("pub fn ")
            {
                offenders.push(format!(
                    "{}:{} names {tool}",
                    file.strip_prefix(root).unwrap().display(),
                    index + 1
                ));
            }
        }
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

#[test]
fn test_readme_and_site_match_the_contract_memory_latency_and_cli_only_commands() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let read = |doc: &str| std::fs::read_to_string(root.join(doc)).unwrap();
    assert!(read("AGENTS.md").contains("Retained memory is about 25–40 MB"));
    for doc in ["README.md", "docs/site/index.html"] {
        let text = read(doc);
        assert!(
            text.contains("25–40 MB"),
            "{doc} disagrees with the retained memory in AGENTS.md"
        );
        assert!(!text.contains("about 25 MB"), "{doc} claims 25 MB");
        for claim in ["Sub-5ms", "0–5 ms", "0 to 5 milliseconds"] {
            assert!(!text.contains(claim), "{doc} claims `{claim}`");
        }
        let line = text
            .lines()
            .find(|line| line.contains("have no MCP tool"))
            .unwrap_or_else(|| panic!("{doc} omits the CLI-only commands"));
        assert!(line.contains("Four query commands"), "{doc}: {line}");
        for command in ["outline", "search", "facts", "stats"] {
            assert!(
                line.contains(&format!("`{command}`"))
                    || line.contains(&format!("code-kb {command}")),
                "{doc} omits `{command}` from the CLI-only commands"
            );
        }
    }
}

#[test]
fn test_guidance_documents_the_edit_check() {
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let read = |doc: &str| std::fs::read_to_string(root.join(doc)).unwrap();
    for doc in [
        "hooks/code-kb-routing-block.md",
        "skills/code-kb/SKILL.md",
        "README.md",
        "CLAUDE.md",
        "AGENTS.md",
        "docs/site/index.html",
        "docs/edit-check.md",
    ] {
        assert!(read(doc).contains("check_edit"), "{doc} omits check_edit");
    }
    for doc in [
        "skills/code-kb/SKILL.md",
        "README.md",
        "docs/site/index.html",
        "docs/edit-check.md",
    ] {
        assert!(
            read(doc).contains("code-kb check"),
            "{doc} omits code-kb check"
        );
    }
    for doc in ["README.md", "docs/site/index.html", "docs/edit-check.md"] {
        let text = read(doc);
        assert!(text.contains("hook Stop"), "{doc} omits the Stop hook");
        assert!(
            text.contains("SubagentStop"),
            "{doc} omits the SubagentStop hook"
        );
    }
    for doc in ["CLAUDE.md", "AGENTS.md"] {
        assert!(
            read(doc).contains("docs/edit-check.md"),
            "{doc} omits the path of the edit check rules"
        );
    }
    assert!(read("docs/release-notes/v2.7.0.md").contains("check_edit"));
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

fn project_with_junk_index(source: &str) -> tempfile::TempDir {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"junk_index\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), source).unwrap();
    let db = root.join(".code-kb").join("artifact.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    std::fs::write(&db, b"not a database ".repeat(4096)).unwrap();
    std::fs::write(root.join(".code-kb/artifact.db-wal"), b"stale").unwrap();
    temp_dir
}

#[test]
fn test_cli_lookup_rebuilds_an_index_that_is_not_a_database() {
    code_kb_core::find_julie_extract_binary().expect("julie-extract must be present");
    let project = project_with_junk_index("pub fn survives_corruption() {}\n");

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env(
            "CODE_KB_TELEMETRY_DIR",
            project.path().join(".telemetry_test"),
        )
        .arg("--root")
        .arg(project.path())
        .args(["lookup", "survives_corruption"])
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("function `survives_corruption` ["),
        "{stdout}"
    );
}

#[cfg(unix)]
#[test]
fn test_cli_names_the_scan_remedy_when_an_unreadable_index_cannot_be_rebuilt() {
    use std::os::unix::fs::PermissionsExt;
    let project = project_with_junk_index("pub fn never_indexed() {}\n");
    let tools = tempfile::tempdir().unwrap();
    let fake = tools.path().join("julie-extract");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version) echo 'julie-extract {}'; exit 0;; esac\necho 'simulated scan failure' >&2\nexit 2\n",
            code_kb_core::PINNED_JULIE_VERSION
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .env("JULIE_EXTRACT_BIN", &fake)
        .env(
            "CODE_KB_TELEMETRY_DIR",
            project.path().join(".telemetry_test"),
        )
        .arg("--root")
        .arg(project.path())
        .args(["lookup", "never_indexed"])
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    let root = code_kb_core::Workspace::new(project.path().to_path_buf()).canonical_root;
    assert!(!output.status.success(), "{stderr}");
    assert!(stderr.contains("simulated scan failure"), "{stderr}");
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_code-kb"));
    assert!(stderr.contains("Run `"), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "{} scan --root {}` to rebuild the index.",
            exe.file_name().unwrap().to_string_lossy(),
            root.display()
        )),
        "{stderr}"
    );
}

fn oversized_repo() -> tempfile::TempDir {
    let repo = code_kb_core::safe_tempdir();
    let root = repo.path();
    let methods: String = (0..1500)
        .map(|i| format!("    def method_{i}(self, value):\n        return value + {i}\n\n"))
        .collect();
    let statements: String = (0..3000)
        .map(|i| format!("    total += {i}  # step {i} of the long loop body\n"))
        .collect();
    let records: Vec<String> = (0..3000)
        .map(|i| format!("{{\"id\": {i}, \"tags\": {{\"name\": \"item{i}\", \"rank\": {i}}}}}"))
        .collect();
    for (path, source) in [
        (
            "pyproject.toml",
            "[project]\nname = 'big'\nversion = '0.1.0'\n".to_string(),
        ),
        ("big.py", format!("class Big:\n{methods}")),
        (
            "long.py",
            format!("def long_function():\n    total = 0\n{statements}    return total\n"),
        ),
        (
            "data.json",
            format!("{{\"records\": [\n{}\n]}}\n", records.join(",\n")),
        ),
    ] {
        std::fs::write(root.join(path), source).unwrap();
    }
    repo
}

#[test]
fn text_answers_for_oversized_files_and_symbols_stay_within_the_output_budget() {
    let repo = oversized_repo();
    let root = repo.path();
    let budget = code_kb_core::OUTPUT_BUDGET_CHARS;

    for file in ["big.py", "data.json"] {
        let skeleton = run_refs(root, &["skeleton", file]);
        assert!(skeleton.len() <= budget, "{file}: {}", skeleton.len());
        assert!(skeleton.contains("// Cut to about 8K tokens"), "{skeleton}");
        assert!(skeleton.contains(" more rows: L"), "{skeleton}");
    }
    let python = run_refs(root, &["skeleton", "big.py"]);
    assert!(python.contains("class Big {"), "{python}");
    assert!(python.contains("def method_0(self, value)"), "{python}");
    assert!(!python.contains("def method_1499("), "{python}");

    let class_body = run_refs(root, &["body", "Big"]);
    assert!(class_body.len() <= budget, "{}", class_body.len());
    assert!(
        class_body.contains("`Big` has 4500 lines, more than about 8K tokens, so its members follow with their bodies hidden. Call get_symbol_body on a member, or Read lines 1-4500."),
        "{class_body}"
    );
    assert!(
        class_body.contains("def method_0(self, value)"),
        "{class_body}"
    );
    assert!(!class_body.contains("return value + 0"), "{class_body}");

    let class_context = run_refs(root, &["context", "Big"]);
    assert!(class_context.len() <= budget, "{}", class_context.len());
    assert!(
        class_context.contains("its members follow"),
        "{class_context}"
    );

    let function_body = run_refs(root, &["body", "long_function"]);
    assert!(function_body.len() <= budget, "{}", function_body.len());
    assert!(
        function_body.contains("    total += 0  # step 0"),
        "{function_body}"
    );
    let cut_line = function_body.lines().last().unwrap();
    assert!(
        cut_line.starts_with("// Cut to about 8K tokens: Read lines ")
            && cut_line.ends_with("-3003 for the rest."),
        "{cut_line}"
    );

    let json_body = run_refs(root, &["--json", "body", "long_function"]);
    assert!(json_body.contains("step 2999 of the long loop body"));
}
