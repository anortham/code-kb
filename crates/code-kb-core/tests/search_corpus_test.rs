use code_kb_core::{
    Workspace, ensure_fts_index_path, find_julie_extract_binary, fts_search_symbols_explained,
    fts_search_symbols_scoped, open_read_only, safe_tempdir, scan_workspace,
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
fn multiword_wsgi_query_ranks_application_methods_ahead_of_real_stream_handler() {
    // Keep the query-bearing signatures and documentation aligned with Flask's app.py and
    // logging.py so the stream handler competes on real documentation, not a weakened stand-in.
    let app = r#"class Flask:
    def full_dispatch_request(self, ctx: AppContext) -> Response:
        """Dispatches the request and on top of that performs request
        pre and postprocessing as well as HTTP exception catching and
        error handling.

        .. versionadded:: 0.7
        """
        return self.finalize_request(ctx, None)

    def finalize_request(
        self,
        ctx: AppContext,
        rv: ft.ResponseReturnValue | HTTPException,
        from_error_handler: bool = False,
    ) -> Response:
        """Given the return value from a view function this finalizes
        the request by converting it into a response and invoking the
        postprocessing functions. This is invoked for both normal
        request dispatching as well as error handlers.

        Because this means that it might be called as a result of a
        failure a special safe mode is available which can be enabled
        with the `from_error_handler` flag. If enabled, failures in
        response processing will be logged and otherwise ignored.

        :internal:
        """
        return self.make_response(rv)

    def wsgi_app(
        self, environ: WSGIEnvironment, start_response: StartResponse
    ) -> cabc.Iterable[bytes]:
        """The actual WSGI application. This is not implemented in
        :meth:`__call__` so that middlewares can be applied without
        losing a reference to the app object. Instead of doing this::

            app = MyMiddleware(app)

        It's a better idea to do this instead::

            app.wsgi_app = MyMiddleware(app.wsgi_app)

        Then you still have the original application object around and
        can continue to call methods on it.

        .. versionchanged:: 0.7
            Teardown events for the request and app contexts are called
            even if an unhandled error occurs. Other events may not be
            called depending on when an error occurs during dispatch.

        :param environ: A WSGI environment.
        :param start_response: A callable accepting a status code,
            a list of headers, and an optional exception context to
            start the response.
        """
        ctx = self.request_context(environ)
        return self.full_dispatch_request(ctx)
"#;
    let logging = r#"@LocalProxy
def wsgi_errors_stream() -> t.TextIO:
    """Find the most appropriate error stream for the application. If a request
    is active, log to ``wsgi.errors``, otherwise use ``sys.stderr``.

    If you configure your own :class:`logging.StreamHandler`, you may want to
    use this for the stream. If you are using file or dict configuration and
    can't import this directly, you can refer to it as
    ``ext://flask.logging.wsgi_errors_stream``.
    """
    return sys.stderr
"#;
    let (_dir, db) = scanned_repo(&[("src/flask/app.py", app), ("src/flask/logging.py", logging)]);
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
    let top_two: Vec<_> = results
        .iter()
        .take(2)
        .map(|result| result.symbol.name.as_str())
        .collect();

    assert_eq!(
        top_two,
        ["wsgi_app", "full_dispatch_request"],
        "Flask's application entry point and dispatcher should lead its actual stream-handler competitor: {:#?}",
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

const DIGIT_CORPUS: &[(&str, &str)] = &[(
    "src/limits.rs",
    "/// Upper bound on retries.\npub const MAX_RETRIES: u32 = 1;\n\n/// Slots in the ring.\npub const SLOT_COUNT: u32 = 7;\n\n/// Parses one thing from the input.\npub fn parse_thing(input: &str) -> usize {\n    input.len()\n}\n",
)];

fn searched_names(db_path: &Path, query: &str) -> Vec<String> {
    let conn = open_read_only(db_path).unwrap();
    fts_search_symbols_scoped(&conn, query, None, None, false, 20)
        .unwrap()
        .into_iter()
        .map(|result| result.symbol.name)
        .collect()
}

#[test]
fn a_row_that_matches_only_a_digit_of_a_wordy_query_is_no_match() {
    let (_dir, db) = scanned_repo(DIGIT_CORPUS);

    assert!(searched_names(&db, "zzq1xyq7blorp").is_empty());
    assert_eq!(searched_names(&db, "zzq1_thing"), ["parse_thing"]);
}

#[test]
fn a_query_of_digits_still_finds_the_rows_that_hold_them() {
    let (_dir, db) = scanned_repo(DIGIT_CORPUS);

    assert!(searched_names(&db, "1").contains(&"MAX_RETRIES".to_string()));
    assert!(searched_names(&db, "7").contains(&"SLOT_COUNT".to_string()));
}

#[test]
fn a_word_past_the_doc_head_still_finds_its_row() {
    let mut notes = String::from("# Release notes\n\n");
    while notes.len() < 900 {
        notes.push_str("The archive layout stays the same in this release. ");
    }
    notes.push_str("\n\nThe glimmerstone codec is new.\n");
    let (_dir, db) = scanned_repo(&[("docs/notes.md", notes.as_str())]);

    assert_eq!(searched_names(&db, "glimmerstone"), ["Release notes"]);
    assert_eq!(searched_names(&db, "glimmerstone 2"), ["Release notes"]);
}

#[test]
fn a_class_name_query_still_lists_the_members_of_that_class() {
    let (_dir, db) = scanned_repo(&[(
        "src/glimmer.py",
        "class Glimmer:\n    def start(self):\n        return 1\n\n    def stop(self):\n        return 2\n",
    )]);

    let names = searched_names(&db, "Glimmer");
    assert!(names.contains(&"start".to_string()), "{names:?}");
    assert!(names.contains(&"stop".to_string()), "{names:?}");
}

#[test]
fn a_digit_in_a_wordy_query_does_not_crowd_out_the_word_matches() {
    let mut limits = String::new();
    for i in 0..200 {
        limits.push_str(&format!("pub const LIMIT_{i}: u32 = 1;\n"));
    }
    let mut files: Vec<(String, String)> = vec![("src/limits.rs".into(), limits)];
    for i in 0..5 {
        files.push((
            format!("docs/guide_{i}.md"),
            format!("# Guide {i}\n\nThis page covers one thing about the release.\n"),
        ));
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_str()))
        .collect();
    let (_dir, db) = scanned_repo(&files);

    let plain = searched_names(&db, "thing");
    let with_digit = searched_names(&db, "zzq1_thing");
    assert_eq!(plain.len(), 5, "{plain:?}");
    assert!(
        plain.iter().all(|name| with_digit.contains(name)),
        "{plain:?} vs {with_digit:?}"
    );
}

#[test]
fn a_heading_that_names_the_query_is_still_found() {
    let (_dir, db) = scanned_repo(&[(
        "docs/replay.md",
        "# Edit replay cases\n\nThe harness replays the check on real commits.\n",
    )]);
    let conn = open_read_only(&db).unwrap();

    let rows = fts_search_symbols_scoped(&conn, "replay cases", None, None, false, 20).unwrap();

    assert_eq!(
        rows.first().map(|r| r.symbol.name.as_str()),
        Some("Edit replay cases")
    );
    assert!(rows[0].score < 0.0);
}

const BLOG_VIEWS: &str = r#"@bp.route("/create", methods=("GET", "POST"))
@login_required
def create():
    """Create a new post for the current user."""
    return render_template("blog/create.html")


@bp.route("/<int:id>/update", methods=("GET", "POST"))
@login_required
def update(id):
    """Update a post if the current user is the author."""
    post = get_post(id)
    return render_template("blog/update.html", post=post)
"#;

#[test]
fn a_query_word_that_names_the_example_file_keeps_its_view_in_the_top_twenty() {
    let mut helpers = String::new();
    for index in 0..25 {
        helpers.push_str(&format!(
            "def open_resource_{index}(path):\n    \"\"\"Open an existing resource.\"\"\"\n    return path\n\n\n"
        ));
    }
    let (_dir, db) = scanned_repo(&[
        ("src/flask/helpers.py", helpers.as_str()),
        ("examples/tutorial/flaskr/blog.py", BLOG_VIEWS),
    ]);

    let names = searched_names(&db, "view that edits an existing blog post");

    assert!(names.contains(&"update".to_string()), "{names:?}");
}

fn module_credit(db_path: &Path, query: &str, path: &str, name: &str, word: &str) -> String {
    let conn = open_read_only(db_path).unwrap();
    let rows = fts_search_symbols_explained(&conn, query, None, None, false, 50, true).unwrap();
    let row = rows
        .iter()
        .filter(|r| r.symbol.path == path && r.symbol.name == name)
        .min_by_key(|r| r.symbol.kind == "export")
        .unwrap_or_else(|| {
            let listed: Vec<String> = rows
                .iter()
                .map(|r| format!("{} {} ({})", r.symbol.kind, r.symbol.name, r.symbol.path))
                .collect();
            panic!("{query:?} did not list {name} in {path}: {listed:#?}")
        });
    row.explain
        .as_ref()
        .unwrap()
        .terms
        .iter()
        .find(|(term, _, _)| term == word)
        .map(|(_, field, _)| field.clone())
        .unwrap()
}

#[test]
fn a_lua_init_file_takes_the_name_of_its_folder() {
    let (_dir, db) = scanned_repo(&[(
        "lua/blog/init.lua",
        "local M = {}\nfunction M.render(post) return post end\nreturn M\n",
    )]);
    let init = "lua/blog/init.lua";

    assert_eq!(
        module_credit(&db, "render blog", init, "render", "blog"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "render init", init, "render", "init"),
        "none"
    );
}

#[test]
fn a_go_file_takes_the_name_of_its_package_clause() {
    let (_dir, db) = scanned_repo(&[
        ("pkg/go.mod", "module example.com/acme/pkg\n\ngo 1.22\n"),
        (
            "pkg/blogapi/server.go",
            "package blogsvc\n\nfunc Serve() {}\n",
        ),
        (
            "pkg/cmd/tool/main.go",
            "package main\n\nfunc run() {}\n\nfunc main() { run() }\n",
        ),
    ]);
    let server = "pkg/blogapi/server.go";

    assert_eq!(
        module_credit(&db, "serve blogsvc", server, "Serve", "blogsvc"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "serve server", server, "Serve", "server"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "serve blogapi", server, "Serve", "blogapi"),
        "none"
    );
    let main = "pkg/cmd/tool/main.go";
    assert_eq!(module_credit(&db, "run tool", main, "run", "tool"), "path");
}

#[test]
fn a_rust_crate_root_takes_the_name_of_its_crate() {
    let (_dir, db) = scanned_repo(&[
        (
            "crates/blog-kit/Cargo.toml",
            "[package]\nname = \"blog-kit\"\nversion = \"0.1.0\"\n",
        ),
        (
            "crates/blog-kit/src/lib.rs",
            "pub fn render(post: &str) -> &str {\n    post\n}\n",
        ),
        (
            "crates/blog-kit/src/main.rs",
            "fn start() {}\n\nfn main() {\n    start();\n}\n",
        ),
        (
            "crates/blog-kit/src/bin/tool/main.rs",
            "fn launch() {}\n\nfn main() {\n    launch();\n}\n",
        ),
        (
            "crates/blog-kit/src/util.rs",
            "pub mod inner {\n    pub fn compose() {}\n}\n",
        ),
        (
            "crates/books/Cargo.toml",
            "[package]\nname = \"books\"\nversion = \"0.1.0\"\n\n[lib]\nname = \"ledger\"\n",
        ),
        ("crates/books/src/lib.rs", "pub fn settle() {}\n"),
    ]);
    let lib = "crates/blog-kit/src/lib.rs";
    let main = "crates/blog-kit/src/main.rs";
    let tool = "crates/blog-kit/src/bin/tool/main.rs";
    let util = "crates/blog-kit/src/util.rs";
    let books = "crates/books/src/lib.rs";

    assert_eq!(
        module_credit(&db, "render blog kit", lib, "render", "kit"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "render lib", lib, "render", "lib"),
        "none"
    );
    assert_eq!(
        module_credit(&db, "start blog kit", main, "start", "blog"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "launch tool", tool, "launch", "tool"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "compose inner", util, "compose", "inner"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "settle ledger", books, "settle", "ledger"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "settle books", books, "settle", "books"),
        "none"
    );
}

#[test]
fn python_swift_zig_and_scala_entry_files_take_the_name_of_their_package() {
    let (_dir, db) = scanned_repo(&[
        (
            "py/blog/__main__.py",
            "def render(post):\n    return post\n",
        ),
        (
            "Sources/BlogKit/main.swift",
            "func render(post: String) -> String { return post }\n",
        ),
        (
            "ledger/src/root.zig",
            "pub fn render(post: []const u8) []const u8 {\n    return post;\n}\n",
        ),
        (
            "scala/feed/package.scala",
            "package object feed {\n  def render(post: String): String = post\n}\n",
        ),
    ]);

    let python = "py/blog/__main__.py";
    let swift = "Sources/BlogKit/main.swift";
    let zig = "ledger/src/root.zig";
    let scala = "scala/feed/package.scala";
    assert_eq!(
        module_credit(&db, "render blog", python, "render", "blog"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "render blog kit", swift, "render", "kit"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "render ledger", zig, "render", "ledger"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "render feed", scala, "render", "feed"),
        "path"
    );
}

#[test]
fn a_member_of_a_module_takes_the_name_of_that_module() {
    let (_dir, db) = scanned_repo(&[
        (
            "lib/my_app/blog.ex",
            "defmodule MyApp.Blog do\n  defmodule Post do\n    def publish(p), do: p\n  end\nend\n",
        ),
        (
            "lib/content.rb",
            "module Blog\n  module Articles\n    def self.publish(post)\n      post\n    end\n  end\nend\n",
        ),
        (
            "src/Posts.fs",
            "module Blog.Posts\n\nlet publish post = post\n\nmodule Inner =\n    let helper x = x\n",
        ),
        (
            "src/feed.ts",
            "namespace Feed {\n  export function publish(post: string) { return post; }\n}\n",
        ),
        (
            "src/view.tsx",
            "namespace Gallery {\n  export function publish(post: string) { return post; }\n}\n",
        ),
    ]);

    let elixir = "lib/my_app/blog.ex";
    let ruby = "lib/content.rb";
    let fsharp = "src/Posts.fs";
    assert_eq!(
        module_credit(&db, "publish post", elixir, "publish", "post"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "publish articles", ruby, "publish", "articles"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "helper inner", fsharp, "helper", "inner"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "publish feed", "src/feed.ts", "publish", "feed"),
        "path"
    );
    let tsx = "src/view.tsx";
    assert_eq!(
        module_credit(&db, "publish gallery", tsx, "publish", "gallery"),
        "path"
    );
}

#[test]
fn a_swift_file_takes_the_name_of_the_package_target_that_holds_it() {
    let (_dir, db) = scanned_repo(&[
        (
            "app/Package.swift",
            "// swift-tools-version:5.9\nimport PackageDescription\n\nlet package = Package(\n    name: \"Blog\",\n    targets: [\n        .target(name: \"BlogKit\"),\n        .target(name: \"Feeds\", path: \"Custom/FeedCode\"),\n        .executableTarget(name: \"Tool\", path: \"Sources/Tool\"),\n    ]\n)\n",
        ),
        (
            "app/Sources/BlogKit/Views/Post.swift",
            "func publish(text: String) -> String { return text }\n",
        ),
        (
            "app/Custom/FeedCode/Reader.swift",
            "func fetch(url: String) -> String { return url }\n",
        ),
        (
            "app/Sources/Tool/main.swift",
            "func launch(name: String) -> String { return name }\n",
        ),
        (
            "notes/Package.swift",
            "// swift-tools-version:5.9\nimport PackageDescription\n\nlet package = Package(\n    name: \"Notes\",\n    targets: [.target(name: \"NoteKit\")]\n)\n",
        ),
        (
            "notes/src/NoteKit/Store.swift",
            "func save(note: String) -> String { return note }\n",
        ),
        (
            "mixed/Package.swift",
            "// swift-tools-version:5.9\nimport PackageDescription\n\nlet package = Package(\n    name: \"Mixed\",\n    targets: [.target(name: \"Alpha\"), .target(name: \"Beta\")]\n)\n",
        ),
        (
            "mixed/Sources/Alpha/A.swift",
            "func align(x: Int) -> Int { return x }\n",
        ),
        (
            "mixed/src/Beta/B.swift",
            "func bake(x: Int) -> Int { return x }\n",
        ),
        (
            "loose/Sources/Gadget/Gadget.swift",
            "func spin(x: Int) -> Int { return x }\n",
        ),
    ]);

    let post = "app/Sources/BlogKit/Views/Post.swift";
    assert_eq!(
        module_credit(&db, "publish blog kit", post, "publish", "kit"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "publish post", post, "publish", "post"),
        "path"
    );
    let reader = "app/Custom/FeedCode/Reader.swift";
    assert_eq!(
        module_credit(&db, "fetch feeds", reader, "fetch", "feeds"),
        "path"
    );
    let main = "app/Sources/Tool/main.swift";
    assert_eq!(
        module_credit(&db, "launch tool", main, "launch", "tool"),
        "path"
    );
    let store = "notes/src/NoteKit/Store.swift";
    assert_eq!(
        module_credit(&db, "save note kit", store, "save", "kit"),
        "path"
    );
    assert_eq!(
        module_credit(
            &db,
            "align alpha",
            "mixed/Sources/Alpha/A.swift",
            "align",
            "alpha"
        ),
        "path"
    );
    assert_eq!(
        module_credit(&db, "bake beta", "mixed/src/Beta/B.swift", "bake", "beta"),
        "none"
    );
    let gadget = "loose/Sources/Gadget/Gadget.swift";
    assert_eq!(
        module_credit(&db, "spin gadget", gadget, "spin", "gadget"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "spin sources", gadget, "spin", "sources"),
        "none"
    );
}

#[test]
fn a_zig_root_source_file_takes_the_name_that_build_zig_gives_it() {
    let (_dir, db) = scanned_repo(&[
        (
            "pkg/build.zig",
            "const std = @import(\"std\");\n\npub fn build(b: *std.Build) void {\n    _ = b.addModule(\"blogz\", .{ .root_source_file = b.path(\"./src/root.zig\") });\n    const exe = b.addExecutable(.{ .name = \"tool\", .root_source_file = b.path(\"src/main.zig\") });\n    const tests = b.addTest(.{ .root_source_file = b.path(\"src/root.zig\") });\n    _ = exe;\n    _ = tests;\n}\n",
        ),
        (
            "pkg/src/root.zig",
            "pub fn render(post: []const u8) []const u8 {\n    return post;\n}\n",
        ),
        (
            "pkg/src/main.zig",
            "pub fn launch(name: []const u8) []const u8 {\n    return name;\n}\n",
        ),
        (
            "pkg/src/util.zig",
            "pub fn trim(text: []const u8) []const u8 {\n    return text;\n}\n",
        ),
        (
            "plain/src/root.zig",
            "pub fn paint(post: []const u8) []const u8 {\n    return post;\n}\n",
        ),
    ]);

    let root = "pkg/src/root.zig";
    assert_eq!(
        module_credit(&db, "render blogz", root, "render", "blogz"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "render pkg", root, "render", "pkg"),
        "none"
    );
    let main = "pkg/src/main.zig";
    assert_eq!(
        module_credit(&db, "launch tool", main, "launch", "tool"),
        "path"
    );
    let util = "pkg/src/util.zig";
    assert_eq!(
        module_credit(&db, "trim blogz", util, "trim", "blogz"),
        "none"
    );
    assert_eq!(
        module_credit(&db, "trim util", util, "trim", "util"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "paint plain", "plain/src/root.zig", "paint", "plain"),
        "path"
    );
}

#[test]
fn a_dart_library_directive_names_the_module_of_its_file_and_its_parts() {
    let (_dir, db) = scanned_repo(&[
        (
            "app/lib/src/posts_impl.dart",
            "/// The posts library.\nlibrary blog.posts;\n\npart 'drafts.dart';\n\nString publish(String text) => text;\n",
        ),
        (
            "app/lib/src/drafts.dart",
            "part of blog.posts;\n\nString draft(String text) => text;\n",
        ),
        (
            "app/lib/feed.dart",
            "library;\n\nString fetch(String url) => url;\n",
        ),
    ]);

    let library = "app/lib/src/posts_impl.dart";
    assert_eq!(
        module_credit(&db, "publish posts", library, "publish", "posts"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "publish blog", library, "publish", "blog"),
        "none"
    );
    let part = "app/lib/src/drafts.dart";
    assert_eq!(
        module_credit(&db, "draft posts", part, "draft", "posts"),
        "path"
    );
    assert_eq!(
        module_credit(&db, "fetch feed", "app/lib/feed.dart", "fetch", "feed"),
        "path"
    );
}

#[test]
fn a_qmldir_row_takes_the_name_of_its_module_line() {
    let (_dir, db) = scanned_repo(&[
        ("qml/Blog/qmldir", "module Org.Gallery\nPost 1.0 Post.qml\n"),
        ("qml/Blog/Post.qml", "import QtQuick\nItem {}\n"),
    ]);

    let qmldir = "qml/Blog/qmldir";
    assert_eq!(
        module_credit(&db, "post gallery", qmldir, "Post", "gallery"),
        "path"
    );
}

#[test]
fn a_namespace_or_package_that_wraps_whole_files_credits_nothing() {
    let (_dir, db) = scanned_repo(&[
        (
            "cs/Post.cs",
            "namespace Acme.Blog { public class Post { public void Render() {} } }\n",
        ),
        (
            "java/com/acme/blog/Article.java",
            "package com.acme.blog;\npublic class Article { void render() {} }\n",
        ),
        (
            "kt/com/acme/blog/Story.kt",
            "package com.acme.blog\nfun render(post: String) = post\n",
        ),
        (
            "php/src/Blog/Entry.php",
            "<?php\nnamespace App\\Blog;\nclass Entry { public function render() { return 1; } }\n",
        ),
        ("cpp/engine.cpp", "namespace blog { void render() {} }\n"),
    ]);

    for (path, name) in [
        ("cs/Post.cs", "Render"),
        ("java/com/acme/blog/Article.java", "render"),
        ("kt/com/acme/blog/Story.kt", "render"),
        ("php/src/Blog/Entry.php", "render"),
        ("cpp/engine.cpp", "render"),
    ] {
        assert_eq!(
            module_credit(&db, "render blog", path, name, "blog"),
            "none",
            "{path}"
        );
    }
}
