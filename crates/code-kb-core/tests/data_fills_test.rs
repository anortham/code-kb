use code_kb_core::{
    SymbolSelector, Workspace, blast_radius_selected_op, codebase_outline_op_with,
    file_skeleton_op, file_skeleton_op_with, find_julie_extract_binary, format_blast_radius,
    format_reference_report, lookup_symbols_op, open_read_only, reference_report,
    render_symbol_lookup, safe_tempdir, scan_workspace, select_reference_target,
};
use std::fs;
use std::path::PathBuf;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: tempfile::TempDir,
}

impl Repo {
    fn workspace(&self) -> Workspace {
        Workspace::new(self.root.clone())
    }

    fn skeleton(&self, path: &str, public_only: bool) -> String {
        let conn = open_read_only(&self.db).unwrap();
        file_skeleton_op_with(&self.workspace(), &self.db, &conn, path, public_only).unwrap()
    }

    fn lookup(&self, query: &str) -> (code_kb_core::SymbolLookup, String) {
        let conn = open_read_only(&self.db).unwrap();
        let lookup = lookup_symbols_op(&self.db, &conn, query, None, None, false, 20).unwrap();
        let text = render_symbol_lookup(&conn, query, &lookup, 20).unwrap();
        (lookup, text)
    }
}

fn scanned_repo(files: &[(&str, &str)]) -> Repo {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root.clone()), &db, true).expect("scan failed");
    Repo {
        root,
        db,
        _temp: temp,
    }
}

const RUST_API: &str = "pub fn open() {}\n\nfn helper() {}\n\npub(crate) fn shared() {}\n\npub struct Api {\n    pub name: u8,\n    secret: u8,\n}\n\nstruct Hidden {\n    inner: u8,\n}\n\nimpl Hidden {\n    pub fn reveal(&self) {}\n}\n";

#[test]
fn a_public_skeleton_keeps_public_and_crate_rows_and_counts_the_private_ones() {
    let repo = scanned_repo(&[("src/lib.rs", RUST_API)]);

    let public = repo.skeleton("src/lib.rs", true);
    let full = repo.skeleton("src/lib.rs", false);

    for shown in ["fn open", "fn shared", "struct Api", "name"] {
        assert!(public.contains(shown), "{shown}: {public}");
    }
    for hidden in ["helper", "secret", "Hidden", "inner", "reveal"] {
        assert!(!public.contains(hidden), "{hidden}: {public}");
    }
    assert!(
        public.contains("// Public surface: 5 private rows hidden.\n"),
        "{public}"
    );
    assert!(
        full.contains("fn helper") && full.contains("Hidden"),
        "{full}"
    );
    assert!(!full.contains("Public surface"), "{full}");
    let conn = open_read_only(&repo.db).unwrap();
    assert_eq!(
        file_skeleton_op(&repo.workspace(), &repo.db, &conn, "src/lib.rs").unwrap(),
        full
    );
}

#[test]
fn a_public_python_skeleton_hides_underscore_names() {
    let repo = scanned_repo(&[(
        "pkg/app.py",
        "def run():\n    pass\n\n\ndef _helper():\n    pass\n\n\nclass App:\n    def start(self):\n        pass\n\n    def _boot(self):\n        pass\n\n    def __init__(self):\n        pass\n",
    )]);

    let public = repo.skeleton("pkg/app.py", true);

    for shown in ["def run", "class App", "def start", "__init__"] {
        assert!(public.contains(shown), "{shown}: {public}");
    }
    assert!(!public.contains("_helper"), "{public}");
    assert!(!public.contains("_boot"), "{public}");
    assert!(
        public.contains("// Public surface: 2 private rows hidden.\n"),
        "{public}"
    );
}

#[test]
fn a_public_typescript_skeleton_keeps_members_with_no_modifier() {
    let repo = scanned_repo(&[(
        "src/service.ts",
        "export class Service {\n  send(): void {}\n  private retry(): void {}\n  protected log(): void {}\n}\n",
    )]);

    let public = repo.skeleton("src/service.ts", true);

    assert!(public.contains("send"), "{public}");
    assert!(!public.contains("retry"), "{public}");
    assert!(!public.contains("log("), "{public}");
}

#[test]
fn a_public_skeleton_of_a_file_with_only_private_rows_says_so() {
    let repo = scanned_repo(&[("src/lib.rs", "fn helper() {}\n\nfn other() {}\n")]);

    let public = repo.skeleton("src/lib.rs", true);

    assert!(
        public.contains("// No public rows: 2 private rows hidden.\n"),
        "{public}"
    );
    assert!(!public.contains("helper"), "{public}");
}

#[test]
fn a_public_folder_outline_names_public_definitions_and_counts_private_ones() {
    let repo = scanned_repo(&[
        ("src/lib.rs", RUST_API),
        ("src/inner.rs", "fn only_private() {}\n"),
    ]);
    let conn = open_read_only(&repo.db).unwrap();

    let public = codebase_outline_op_with(&repo.workspace(), &conn, 2, Some("src"), true).unwrap();
    let full = codebase_outline_op_with(&repo.workspace(), &conn, 2, Some("src"), false).unwrap();

    assert!(
        public.contains("lib.rs [function open, function shared, struct Api, 2 private]"),
        "{public}"
    );
    assert!(public.contains("inner.rs [1 private]"), "{public}");
    assert!(!public.contains("helper"), "{public}");
    assert!(full.contains("function helper"), "{full}");
    assert!(!full.contains(" private]"), "{full}");
}

const FLASK_APP: &str = "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.get(\"/users/<int:user_id>\")\ndef show_user(user_id):\n    return str(user_id)\n\n\n@app.post(\"/users\")\ndef create_user():\n    return \"\"\n";

#[test]
fn a_lookup_with_a_route_template_lists_the_route_and_its_handler() {
    let repo = scanned_repo(&[("app.py", FLASK_APP)]);

    let (lookup, text) = repo.lookup("/users/{id}");
    let (_, with_verb) = repo.lookup("GET /users/{id}");
    let (_, wrong_verb) = repo.lookup("POST /users/{id}");

    assert_eq!(lookup.facts.len(), 1, "{text}");
    assert!(
        text.contains("- route `GET /users/<int:user_id>` (app.py:6) -> show_user\n"),
        "{text}"
    );
    assert!(with_verb.contains("-> show_user"), "{with_verb}");
    assert!(!wrong_verb.contains("-> show_user"), "{wrong_verb}");
    assert!(!text.starts_with("No symbols found"), "{text}");
}

#[test]
fn a_lookup_with_a_dotted_key_lists_config_keys_in_toml_json_and_yaml() {
    let repo = scanned_repo(&[
        (
            "config.toml",
            "[mcp_servers.code-kb]\ncommand = \"code-kb\"\nargs = [\"serve\"]\n",
        ),
        (
            ".mcp.json",
            "{\n  \"mcpServers\": {\n    \"code-kb\": {\n      \"command\": \"code-kb\"\n    }\n  }\n}\n",
        ),
        ("ci.yml", "jobs:\n  test:\n    runs-on: ubuntu-latest\n"),
        ("src/lib.rs", "pub fn command() {}\n"),
    ]);

    let (toml, toml_text) = repo.lookup("mcp_servers.code-kb.command");
    let (_, suffix_text) = repo.lookup("code-kb.command");
    let (_, json_text) = repo.lookup("mcpServers.code-kb.command");
    let (_, yaml_text) = repo.lookup("jobs.test.runs-on");
    let (plain, plain_text) = repo.lookup("command");

    assert_eq!(toml.facts.len(), 1, "{toml_text}");
    let (json, _) = repo.lookup("mcpServers.code-kb.command");
    assert!(json.names.is_empty(), "{:?}", json.names);
    assert!(
        toml_text.contains("- config `mcp_servers.code-kb.command` (config.toml:2)\n"),
        "{toml_text}"
    );
    assert!(
        suffix_text.contains("config `mcp_servers.code-kb.command` (config.toml:2)")
            && suffix_text.contains("config `mcpServers.code-kb.command` (.mcp.json:4)"),
        "{suffix_text}"
    );
    assert!(
        json_text.contains("- config `mcpServers.code-kb.command` (.mcp.json:4)\n"),
        "{json_text}"
    );
    assert!(
        yaml_text.contains("- config `jobs.test.runs-on` (ci.yml:3)\n"),
        "{yaml_text}"
    );
    assert!(plain.facts.is_empty(), "{plain_text}");
    assert!(!plain_text.contains("config `"), "{plain_text}");
}

#[test]
fn a_lookup_fact_follows_the_path_and_kind_filters() {
    let repo = scanned_repo(&[
        ("a/config.toml", "[server]\nport = 1\n"),
        ("b/config.toml", "[server]\nport = 2\n"),
    ]);
    let conn = open_read_only(&repo.db).unwrap();

    let scoped =
        lookup_symbols_op(&repo.db, &conn, "server.port", None, Some("b"), false, 20).unwrap();
    let functions = lookup_symbols_op(
        &repo.db,
        &conn,
        "server.port",
        Some("function"),
        Some("a"),
        false,
        20,
    )
    .unwrap();
    let configs = lookup_symbols_op(
        &repo.db,
        &conn,
        "server.port",
        Some("config"),
        None,
        false,
        20,
    )
    .unwrap();

    let paths: Vec<_> = scoped.facts.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["b/config.toml"]);
    assert!(functions.facts.is_empty());
    assert_eq!(configs.facts.len(), 2);
    assert_eq!(configs.names.len(), 0);
    let both = lookup_symbols_op(&repo.db, &conn, "server.port", None, None, false, 20).unwrap();
    assert_eq!(both.facts.len(), 2);
    assert!(both.names.is_empty(), "{:?}", both.names);
}

#[test]
fn a_callers_report_of_a_json_key_says_the_index_records_no_sites_in_json() {
    let repo = scanned_repo(&[
        ("settings.json", "{\n  \"retries\": 3\n}\n"),
        ("src/app.py", "def retries():\n    return 3\n"),
    ]);
    let conn = open_read_only(&repo.db).unwrap();

    let json_target = select_reference_target(&conn, "retries", Some("settings.json")).unwrap();
    let json_report = reference_report(&conn, &repo.root, &json_target, false).unwrap();
    let json_text = format_reference_report(&json_report, 20);
    let py_target = select_reference_target(&conn, "retries", Some("src/app.py")).unwrap();
    let py_text = format_reference_report(
        &reference_report(&conn, &repo.root, &py_target, false).unwrap(),
        20,
    );

    let line = "Coverage: the index records no reference sites in json files (julie: identifiers exception), so this answer lists no site inside them.\n";
    assert!(json_text.contains(line), "{json_text}");
    let sites = json_text.find("Sites:").unwrap();
    assert!(json_text.find(line).unwrap() > sites, "{json_text}");
    assert_eq!(
        json_report.coverage_gap.as_ref().unwrap().languages,
        ["json"]
    );
    assert!(!py_text.contains("Coverage:"), "{py_text}");
}

#[test]
fn a_blast_radius_of_a_json_key_says_the_index_records_no_sites_in_json() {
    let repo = scanned_repo(&[
        ("settings.json", "{\n  \"retries\": 3\n}\n"),
        ("src/app.py", "def retries():\n    return 3\n"),
    ]);
    let conn = open_read_only(&repo.db).unwrap();
    let id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'retries' AND path = 'settings.json'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let blast = |selector, file: Option<&str>| {
        format_blast_radius(
            &blast_radius_selected_op(
                &repo.workspace(),
                Some(&repo.db),
                &conn,
                selector,
                file,
                2,
                20,
            )
            .unwrap(),
        )
    };

    let by_id = blast(Some(SymbolSelector::Id(id)), None);
    let by_file = blast(None, Some("settings.json"));
    let python = blast(None, Some("src/app.py"));

    let line = "Coverage: the index records no reference sites in json files (julie: identifiers exception), so this answer lists no site inside them.\n";
    for text in [&by_id, &by_file] {
        let header = text.find("## Blast Radius").unwrap();
        let tests = text.find("### Likely Tests").unwrap();
        let at = text.find(line).unwrap_or_else(|| panic!("{text}"));
        assert!(header < at && at < tests, "{text}");
    }
    assert!(!python.contains("Coverage:"), "{python}");
}

#[test]
fn a_qualified_lookup_lists_ambiguous_definitions_beside_the_keys_it_names() {
    let repo = scanned_repo(&[
        ("config.toml", "[server]\nport = 1\n"),
        ("src/a.rs", "pub struct Server {\n    pub port: u16,\n}\n"),
        ("src/b.rs", "pub struct Server {\n    pub port: u16,\n}\n"),
    ]);

    let (lookup, text) = repo.lookup("Server.port");

    let paths: Vec<_> = lookup.names.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "src/b.rs"], "{text}");
    assert!(
        text.contains("- config `server.port` (config.toml:2)\n"),
        "{text}"
    );
}

#[test]
fn a_lookup_json_answer_counts_the_keys_that_limit_cut() {
    let repo = scanned_repo(&[
        ("a/config.toml", "[server]\nport = 1\n"),
        ("b/config.toml", "[server]\nport = 2\n"),
        ("c/config.toml", "[server]\nport = 3\n"),
    ]);
    let conn = open_read_only(&repo.db).unwrap();

    let cut = lookup_symbols_op(&repo.db, &conn, "server.port", None, None, false, 2).unwrap();
    let whole = lookup_symbols_op(&repo.db, &conn, "server.port", None, None, false, 20).unwrap();

    let cut_json = serde_json::to_value(&cut).unwrap();
    assert_eq!(cut_json["facts"].as_array().unwrap().len(), 2, "{cut_json}");
    assert_eq!(cut_json["facts_found"], 3, "{cut_json}");
    let whole_json = serde_json::to_value(&whole).unwrap();
    assert!(whole_json.get("facts_found").is_none(), "{whole_json}");
}

#[test]
fn a_coverage_line_lists_three_languages_as_a_series() {
    let gap = code_kb_core::CoverageGap {
        languages: vec!["json".into(), "markdown".into(), "toml".into()],
        capabilities: vec!["identifiers".into()],
    };

    let line = code_kb_core::formatters::coverage_gap_line(Some(&gap));

    assert!(
        line.starts_with("Coverage: the index records no reference sites in json, markdown, and toml files (julie: identifiers exception)"),
        "{line}"
    );
}
