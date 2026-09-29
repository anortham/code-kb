use code_kb_core::{
    Workspace, ensure_fts_index_path, find_julie_extract_binary, fts_search_symbols_scoped,
    open_read_only, safe_tempdir, scan_workspace,
};
use std::fs;
use std::path::Path;

fn scanned_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    for (path, content) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root), &db_path, true).expect("scan failed");
    ensure_fts_index_path(&db_path).unwrap();
    (temp_dir, db_path)
}

fn assert_top(db_path: &Path, query: &str, kind: &str, name: &str) {
    let conn = open_read_only(db_path).unwrap();
    let results = fts_search_symbols_scoped(&conn, query, None, None, false, 10).unwrap();
    let ranked: Vec<String> = results
        .iter()
        .map(|r| format!("{} {} ({})", r.symbol.kind, r.symbol.name, r.symbol.path))
        .collect();
    let top = results
        .first()
        .map(|r| (r.symbol.kind.as_str(), r.symbol.name.as_str()));
    assert_eq!(
        top,
        Some((kind, name)),
        "query {query:?} ranked {ranked:#?}"
    );
}

const MULTI_LANGUAGE_CORPUS: &[(&str, &str)] = &[
    (
        "src/render.rs",
        r#"/// Output shape chosen by the caller.
pub enum RenderMode {
    /// Render a file skeleton with the implementation bodies stripped.
    Skeleton,
    /// Print the codebase outline as a directory tree.
    Outline,
}

/// Renders every signature in a file with the implementation bodies stripped.
pub fn render_symbol_skeleton(path: &str, mode: RenderMode) -> String {
    match mode {
        RenderMode::Skeleton => format!("skeleton {path}"),
        RenderMode::Outline => format!("outline {path}"),
    }
}

/// Renders the directory tree of a codebase as a nested outline.
pub fn render_codebase_outline(root: &str) -> String {
    format!("outline {root}")
}
"#,
    ),
    (
        "src/command.rs",
        r#"/// Subcommand selected on the command line.
pub enum Command {
    Serve,
    Scan,
}
"#,
    ),
    (
        "src/syntax.rs",
        r#"/// Checks that the source is well formed before an edit is written.
pub fn validate_syntax(source: &str) -> bool {
    !source.is_empty()
}

/// Reports whether the last check passed.
pub fn is_ok(status: i32) -> bool {
    status == 0
}
"#,
    ),
    (
        "src/archive.rs",
        r#"/// Verifies the archive checksum against the expected digest.
pub fn verify_checksum(archive: &[u8], expected: &str) -> bool {
    expected.len() == archive.len()
}

/// Loads the settings file from disk.
pub fn load_config(path: &str) -> String {
    path.to_string()
}
"#,
    ),
    (
        "scripts/launcher.ts",
        r#"/** Reads the checksum sidecar written next to a downloaded release archive. */
function parseSha256Sidecar(text: string): string {
  return text.trim().split(" ")[0];
}

/** Verifies the archive checksum against the expected digest. */
function verifyChecksum(archive: string, expected: string): boolean {
  return archive === expected;
}

/** Loads the settings file from disk. */
function loadConfig(path: string): string {
  return path;
}
"#,
    ),
    (
        "tools/ansi.py",
        r#"def trim_ansi(text):
    """Remove escape sequences."""
    return text


def escape_palette_table(key):
    """The escape palette table, keyed by every escape palette entry."""
    return key
"#,
    ),
    (
        "src/allocation.rs",
        r#"/// Splits the work between the callees of one request.
pub fn allocate_pack_budget(total: usize) -> usize {
    total
}

/// The token budget allocated to the context pack for one request.
pub struct Allocation {
    pub remaining: usize,
}
"#,
    ),
    (
        "src/http.ts",
        r#"/** Parses a raw HTTP response into its status line, headers, and payload. */
function parseHTTPResponse(raw: string): string[] {
  return raw.split("\r\n");
}
"#,
    ),
    (
        "tools/reconcile.py",
        r#"def reconcile_offline_edits(root):
    """Reconcile files that were edited while the server was offline."""
    return root


def größe_berechnen(pfad):
    """Berechnet die Größe einer Datei."""
    return len(pfad)


def load_config(path):
    """Loads the settings file from disk."""
    return path
"#,
    ),
    (
        "src/Options.cs",
        r#"namespace Distribution
{
    /// <summary>Settings for one run.</summary>
    public class RunOptions
    {
        /// <summary>Gets the workspace root directory.</summary>
        public string RootDirectory { get; set; }

        /// <summary>Loads the settings file from disk.</summary>
        public static RunOptions LoadConfig(string path)
        {
            return new RunOptions { RootDirectory = path };
        }
    }
}
"#,
    ),
    (
        "pkg/scan.go",
        r#"package pkg

// MaxRetryCount is how many times a failed download is tried again.
const MaxRetryCount = 5

// Scan walks every file under root and records it in the index.
func Scan(root string) error {
	return nil
}

// LoadConfig reads the settings file from disk.
func LoadConfig(path string) (string, error) {
	return path, nil
}
"#,
    ),
    (
        "tools/skills.py",
        r#"def create_skill(name):
    """Create a skill from the template registry and return the new skill."""
    return name


def _create_skill(name):
    """Create a skill."""
    return name
"#,
    ),
    (
        "README.md",
        r#"# Distribution

## Validate syntax

Run validate_syntax or ValidateSyntax for syntax validation, then parse_http_response for the http response.

## Parse the sha256 sidecar file

The launcher script reads the sha 256 sidecar, then verify checksum, scan, is_ok, and größe.

## Render a file skeleton without bodies

The codebase outline directory tree shows the root directory and the max retry count.

## Reconcile files edited while server was offline

Reconcile files that were edited while the server was offline.
"#,
    ),
];

fn admission_corpus() -> String {
    let mut source = String::new();
    for i in 1..=170 {
        source.push_str(&format!(
            "/// Helper to parse sidecar file entries.\npub fn sidecar_helper_{i:03}() -> usize {{\n    {i}\n}}\n\n"
        ));
    }
    source.push_str("pub fn parseSha256Sidecar(text: &str) -> &str {\n    text\n}\n");
    source
}

#[test]
fn exact_name_wins() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "validate_syntax", "function", "validate_syntax");
}

#[test]
fn case_style_of_the_query_does_not_matter() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "ValidateSyntax", "function", "validate_syntax");
    assert_top(&db, "parse_http_response", "function", "parseHTTPResponse");
}

#[test]
fn substring_of_a_name_finds_the_symbol() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "sha256", "function", "parseSha256Sidecar");
    assert_top(
        &db,
        "parse the sha256 sidecar file",
        "function",
        "parseSha256Sidecar",
    );
}

#[test]
fn acronyms_and_digit_runs_match_split_or_joined() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "http response", "function", "parseHTTPResponse");
    assert_top(&db, "sha 256", "function", "parseSha256Sidecar");
}

#[test]
fn short_tokens_match_by_name() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "is_ok", "function", "is_ok");
}

#[test]
fn unicode_names_match() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "größe", "function", "größe_berechnen");
}

#[test]
fn stem_variants_match() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "syntax validation", "function", "validate_syntax");
}

#[test]
fn concept_queries_prefer_the_function_over_a_short_enum_variant() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(
        &db,
        "render a file skeleton without bodies",
        "function",
        "render_symbol_skeleton",
    );
    assert_top(
        &db,
        "codebase outline directory tree",
        "function",
        "render_codebase_outline",
    );
}

#[test]
fn kind_rule_prefers_functions_but_exact_names_still_win() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "scan", "function", "Scan");
    assert_top(&db, "max retry count", "constant", "MaxRetryCount");
    assert_top(&db, "root directory", "property", "RootDirectory");
}

#[test]
fn path_rule_demotes_scripts_unless_the_query_names_them() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(
        &db,
        "launcher script sha256",
        "function",
        "parseSha256Sidecar",
    );
    assert_top(&db, "verify checksum", "function", "verify_checksum");
}

#[test]
fn minority_language_symbol_wins_its_concept_query() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(
        &db,
        "reconcile files edited while server was offline",
        "function",
        "reconcile_offline_edits",
    );
}

#[test]
fn a_name_and_doc_covering_three_words_beat_a_name_that_repeats_two() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "trim ansi escape palette", "function", "trim_ansi");
}

#[test]
fn two_whole_token_name_words_beat_a_doc_holding_every_word() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(
        &db,
        "token budget for the context pack",
        "function",
        "allocate_pack_budget",
    );
}

#[test]
fn a_function_holding_the_word_with_context_beats_a_member_named_exactly_the_query() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "outline", "function", "render_codebase_outline");
}

#[test]
fn a_public_name_ranks_before_its_private_twin() {
    let (_dir, db) = scanned_repo(MULTI_LANGUAGE_CORPUS);
    assert_top(&db, "create skill", "function", "create_skill");
}

#[test]
fn trigram_only_target_survives_admission_past_the_word_branch_cap() {
    let source = admission_corpus();
    let (_dir, db) = scanned_repo(&[("src/sidecar.rs", source.as_str())]);
    assert_top(
        &db,
        "parse the sha256 sidecar file",
        "function",
        "parseSha256Sidecar",
    );
}

#[test]
fn multiword_search_does_not_fill_the_top_twenty_with_one_term_name_hits() {
    let mut source = String::new();
    for index in 0..15 {
        source.push_str(&format!(
            "/// Handles an exception error and creates a response.\npub fn handle_exception_{index}(error: Error, response: Response) {{}}\n\n"
        ));
    }
    source.push_str(
        "pub struct Response;\npub fn process_response() {}\npub fn make_default_options_response() {}\npub fn json_provider_response() {}\npub fn default_json_provider_response() {}\n",
    );

    let (_dir, db) = scanned_repo(&[("src/handlers.rs", source.as_str())]);
    let conn = open_read_only(&db).unwrap();
    let results = fts_search_symbols_scoped(
        &conn,
        "handle exception error response",
        None,
        None,
        false,
        20,
    )
    .unwrap();

    assert_eq!(
        results.len(),
        15,
        "single-term matches should not pad a top 20 when 15 multi-term hits exist: {:#?}",
        results
            .iter()
            .map(|result| &result.symbol.name)
            .collect::<Vec<_>>()
    );
    assert!(
        results
            .iter()
            .all(|result| result.symbol.name.starts_with("handle_exception_"))
    );
}

#[test]
fn a_budget_domain_type_ranks_above_its_competing_request_dto() {
    let source = r#"namespace SearchCorpus;

/// A capacity-one user-global lease for test execution.
/// Only one workspace may execute tests; other workspaces wait.
public sealed class ExecutionBudget
{
    public BudgetLease? Acquire(ExecutionBudgetRequest request) => null;
}

/// A user-global request for the execution lease.
public readonly record struct ExecutionBudgetRequest(string WorkspaceRoot, string Reason);
"#;
    let (_dir, db) = scanned_repo(&[("src/budget.cs", source)]);
    let conn = open_read_only(&db).unwrap();
    let results = fts_search_symbols_scoped(
        &conn,
        "workspace tests execution user global budget",
        None,
        None,
        false,
        20,
    )
    .unwrap();
    let rank = |name: &str| {
        results
            .iter()
            .position(|result| result.symbol.name == name)
            .unwrap_or(usize::MAX)
    };

    assert!(
        rank("ExecutionBudget") < rank("ExecutionBudgetRequest"),
        "the domain type should outrank its request DTO: {:#?}",
        results
            .iter()
            .map(|result| (&result.symbol.name, result.score))
            .collect::<Vec<_>>()
    );
}

#[test]
fn execution_budget_ranks_above_hooks_record_at_wide_limits() {
    let budget = r#"namespace Miller.Testing;

/// Capacity-1 user-global lease modeled on a scan governor. Held only while tests execute.
/// A second workspace reports paused while the first executes; idle daemons starve nobody.
public sealed class CtExecutionBudget
{
    public CtExecutionBudgetLease? TryAcquire(CtExecutionBudgetRequest request) => null;
}

/// One execution-scoped request for the user-global CT run lease.
public readonly record struct CtExecutionBudgetRequest(string WorkspaceRoot, string Reason);
"#;
    let hooks = r#"namespace Miller.Server.Tools;

/// Seams for the CT verbs. Budget overrides the user-global execution budget a foreground
/// run takes, so tests bind their own miller home instead of contending on the caller's one.
public sealed record TestsCoreHooks(CtExecutionBudget? Budget, TestsForegroundRunRequest? Run);
public sealed record TestsForegroundRunRequest(string WorkspaceRoot);
"#;
    let (_dir, db) = scanned_repo(&[
        ("src/CtExecutionBudget.cs", budget),
        ("src/TestsCore.cs", hooks),
    ]);
    let conn = open_read_only(&db).unwrap();

    for limit in [20, 200] {
        let results = fts_search_symbols_scoped(
            &conn,
            "one workspace executes tests at a time under a user-global budget",
            None,
            None,
            false,
            limit,
        )
        .unwrap();
        let rank = |name: &str| {
            results
                .iter()
                .position(|result| result.symbol.name == name)
                .unwrap_or(usize::MAX)
        };
        assert!(
            rank("CtExecutionBudget") < rank("TestsCoreHooks"),
            "the execution budget should outrank its test hooks at limit {limit}: {:#?}",
            results
                .iter()
                .map(|result| (&result.symbol.name, result.score))
                .collect::<Vec<_>>()
        );
        assert!(
            rank("CtExecutionBudget") < rank("CtExecutionBudgetRequest"),
            "the execution budget should outrank its request DTO at limit {limit}: {:#?}",
            results
                .iter()
                .map(|result| (&result.symbol.name, result.score))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn specific_backend_type_outranks_generic_terminal_placeholder_helper() {
    let backend = r#"/// The Vercel sandbox terminal backend.
pub struct VercelSandboxEnvironment;
"#;
    let helper = r#"/// Resolve local terminal cwd and provide the sandbox backend default.
pub fn resolve_placeholder_terminal_cwd(terminal_backend: &str, docker_mount: bool) {}
"#;
    let (_dir, db) = scanned_repo(&[("src/backend.rs", backend), ("src/cwd.rs", helper)]);
    let conn = open_read_only(&db).unwrap();
    let results = fts_search_symbols_scoped(
        &conn,
        "seven terminal backends local docker ssh singularity modal daytona and vercel sandbox",
        None,
        None,
        false,
        20,
    )
    .unwrap();
    let rank = |name: &str| {
        results
            .iter()
            .position(|result| result.symbol.name == name)
            .unwrap_or(usize::MAX)
    };

    assert!(
        rank("VercelSandboxEnvironment") < rank("resolve_placeholder_terminal_cwd"),
        "the specific backend type should outrank a generic placeholder helper: {:#?}",
        results
            .iter()
            .map(|result| (&result.symbol.name, result.score))
            .collect::<Vec<_>>()
    );
}

#[test]
fn multiword_search_keeps_distinctive_named_tail_after_many_full_matches() {
    let mut source = String::new();
    for index in 0..15 {
        let migration_term = if index < 7 { "migrate " } else { "" };
        source.push_str(&format!(
            "/// {migration_term}settings memories skills api keys openclaw.\npub fn import_workspace_{index}() {{}}\n\n"
        ));
    }
    source.push_str("pub fn _cmd_migrate() {}\npub fn generic_settings() {}\n");
    let (_dir, db) = scanned_repo(&[("src/claw.rs", source.as_str())]);
    let conn = open_read_only(&db).unwrap();
    let results = fts_search_symbols_scoped(
        &conn,
        "migrate settings memories skills api keys from openclaw",
        None,
        None,
        false,
        20,
    )
    .unwrap();
    let names: Vec<_> = results
        .iter()
        .map(|result| result.symbol.name.as_str())
        .collect();
    assert!(
        names.contains(&"_cmd_migrate"),
        "the distinctive migration entry point should survive the multi-term cutoff: {names:#?}"
    );
    assert!(
        !names.contains(&"generic_settings"),
        "a common one-term name should still be pruned: {names:#?}"
    );
}

#[test]
fn multiword_wsgi_query_ranks_dispatch_methods_above_logging_helper() {
    let mut app = r#"class Flask:
    def full_dispatch_request(self, ctx: AppContext) -> Response:
        """Dispatches the request and performs request preprocessing, response
        creation, HTTP exception catching, and error handling.
        """
        return self.finalize_request(ctx)

    def wsgi_app(
        self, environ: WSGIEnvironment, start_response: StartResponse
    ) -> Iterable[bytes]:
        """The actual WSGI application. This is not implemented in __call__ so
        that middlewares can be applied without losing a reference to the app.
        DOC_PADDING

        Teardown events for the request and app contexts are called even if an
        unhandled error occurs. Other events may not be called during dispatch.
        """
        ctx = self.request_context(environ)
        return self.full_dispatch_request(ctx)
"#
    .to_string();
    app = app.replace(
        "DOC_PADDING",
        &"Middleware wrappers retain the app object. ".repeat(7),
    );
    let logging = r#"def wsgi_errors_stream() -> TextIO:
    """Find the most appropriate error stream for the application. If a request
    is active, log to wsgi.errors, otherwise use sys.stderr.
    """
    return sys.stderr
"#;
    let (_dir, db) = scanned_repo(&[
        ("src/flask/app.py", app.as_str()),
        ("src/flask/logging.py", logging),
    ]);
    let conn = open_read_only(&db).unwrap();
    let results = fts_search_symbols_scoped(
        &conn,
        "incoming WSGI request response error handling",
        None,
        Some("src/flask"),
        false,
        20,
    )
    .unwrap();
    let rank = |name: &str| {
        results
            .iter()
            .position(|result| result.symbol.name == name)
            .unwrap_or(usize::MAX)
    };
    assert!(
        rank("wsgi_app") < rank("wsgi_errors_stream"),
        "the WSGI app should outrank the logging helper: {:#?}",
        results
            .iter()
            .map(|result| (&result.symbol.name, result.score))
            .collect::<Vec<_>>()
    );
    assert!(
        rank("full_dispatch_request") < rank("wsgi_errors_stream"),
        "the request dispatcher should outrank the logging helper: {:#?}",
        results
            .iter()
            .map(|result| (&result.symbol.name, result.score))
            .collect::<Vec<_>>()
    );
}

#[test]
fn multiword_search_keeps_single_term_fallback_for_sparse_matches() {
    let source = r#"pub fn handle_exception() {}

/// An unrelated payload.
pub struct ResponseNoise;
"#;
    let (_dir, db) = scanned_repo(&[("src/handlers.rs", source)]);
    let conn = open_read_only(&db).unwrap();
    let results = fts_search_symbols_scoped(
        &conn,
        "handle exception error response",
        None,
        None,
        false,
        20,
    )
    .unwrap();

    assert!(
        results
            .iter()
            .any(|result| result.symbol.name == "handle_exception")
    );
    assert!(
        results
            .iter()
            .any(|result| result.symbol.name == "ResponseNoise")
    );
}

#[test]
fn multiword_search_keeps_an_exact_name_hit_ahead_of_strong_fallbacks() {
    let mut source = String::from("pub fn fuse() {}\n\n");
    for index in 0..15 {
        source.push_str(&format!(
            "/// rrf fuse\npub const unrelated_{index}: u8 = 0;\n"
        ));
    }

    let (_dir, db) = scanned_repo(&[("src/search.rs", source.as_str())]);
    let conn = open_read_only(&db).unwrap();
    let results = fts_search_symbols_scoped(&conn, "rrf fuse", None, None, false, 20).unwrap();

    assert!(results.iter().any(|result| result.symbol.name == "fuse"));
}
