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
