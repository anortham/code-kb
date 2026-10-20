//! The post-edit check: which symbols changed shape against a base revision, and which sites of
//! each the diff did not reach.
//!
//! The base symbols come from a scan of the base revision of the changed files into a side index
//! under `<root>/.code-kb/check/<pid>/`, which is removed when the check ends. The current symbols
//! and the sites come from the main index, after the changed files are refreshed in it.

mod arms;
mod fields;
mod git;
mod session;
mod signatures;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::warn;

use crate::db::{DbError, ensure_fts_index_path, open_read_only};
use crate::formatters::displayed_symbol_name;
use crate::models::{
    ConstructionSite, LabeledSite, ReferenceReport, SiteLabel, Symbol, VariantUse,
    is_generated_lambda_name,
};
use crate::queries::{
    DOCUMENTATION_LANGUAGES, QueryError, declared_fixture_name, escape_like, load_file_symbols,
};
use crate::reference_report::arity::{self, Arity};
use crate::reference_report::{
    destructurings, enum_variant_names, inherited_group_arity, reference_report_for_edit,
    uses_of_variants,
};
use crate::sync::{self, SyncError, ensure_fresh_file};
use crate::workspace::{Workspace, is_hard_excluded};
use arms::Verdict;
use fields::FieldChange;
use git::{ChangedFile, Git, Hunks};
pub use session::{
    BlockedSite, SessionLock, SessionState, any_site_edited, is_session_id, latest_session_base,
    lock_session, mark_sites, prune_sessions, read_session, record_session_base, write_session,
};
use signatures::{Difference, Shape};

#[derive(Debug, Error)]
pub enum EditCheckError {
    #[error("not a git repository: {0}")]
    NotARepository(String),
    #[error("git revision `{0}` does not exist in this repository")]
    UnknownRevision(String),
    #[error("{0}")]
    Git(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sync(#[from] SyncError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Query(#[from] QueryError),
}

/// The commit `HEAD` names in the repository that holds `root`: `None` outside a repository and
/// before the first commit.
pub fn head_commit(root: &Path) -> Option<String> {
    Git::open(root).ok()?.resolve("HEAD").ok().flatten()
}

/// Where the base revision of a check comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseSource {
    Given,
    Session,
    Head,
}

/// The revision a check compares the working tree with.
#[derive(Debug, Clone, Copy)]
pub enum BaseRequest<'a> {
    Given(&'a str),
    Session(&'a str),
    Head,
}

/// The cuts that bound one check.
#[derive(Debug, Clone, Copy)]
pub struct EditLimits {
    pub files: usize,
    pub symbols: usize,
    pub time: Duration,
}

impl Default for EditLimits {
    fn default() -> Self {
        Self {
            files: 200,
            symbols: 10,
            time: Duration::from_secs(20),
        }
    }
}

/// One site of a changed symbol: where it is, the function that holds it, and a note on the
/// argument count it passes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditSite {
    pub path: String,
    pub line: usize,
    pub caller: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A symbol whose shape changed, with its sites sorted by whether the diff reached them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeChange {
    pub kind: String,
    pub display: String,
    pub path: String,
    pub line: usize,
    pub change: String,
    pub in_diff: Vec<EditSite>,
    pub proven: Vec<EditSite>,
    pub likely: Vec<EditSite>,
    pub unresolved: usize,
}

/// The answer of one check. `diff_hash` names the state of the changed files, so a caller can
/// tell a repeated check of the same diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditCheck {
    pub rev: String,
    pub source: BaseSource,
    pub files_changed: usize,
    pub changes: Vec<ShapeChange>,
    pub tests: Vec<String>,
    pub cuts: Vec<String>,
    pub diff_hash: String,
}

const CALLABLE_KINDS: [&str; 3] = ["function", "method", "constructor"];
const TYPE_KINDS: [&str; 6] = ["class", "struct", "interface", "trait", "enum", "record"];
const FIELD_OWNER_KINDS: [&str; 3] = ["class", "struct", "record"];
const BINDING_KINDS: [&str; 3] = ["constant", "variable", "type"];
/// A removed symbol whose name stays in the same scope as another kind of one group changed
/// kind, not name: `type X` became `interface X`, or `const f` became `function f`.
const KIND_GROUPS: [&[&str]; 2] = [
    &[
        "class",
        "struct",
        "interface",
        "trait",
        "enum",
        "record",
        "type",
    ],
    &["constant", "variable", "function"],
];
const FIELD_KINDS: [&str; 2] = ["field", "property"];
const ATTRIBUTE_WRITE_PREFIXES: [&str; 3] = ["self.", "this.", "cls."];
const SIGNATURE_MODIFIERS: [&str; 30] = [
    "pub",
    "public",
    "private",
    "protected",
    "internal",
    "static",
    "final",
    "abstract",
    "async",
    "override",
    "virtual",
    "sealed",
    "open",
    "export",
    "default",
    "unsafe",
    "extern",
    "inline",
    "const",
    "readonly",
    "synchronized",
    "native",
    "partial",
    "new",
    "suspend",
    "tailrec",
    "operator",
    "infix",
    "external",
    "mutating",
];
const CALL_ARGUMENTS_SQL: &str = "SELECT json_extract(metadata_json, '$.arg_count'),
            json_extract(metadata_json, '$.arg_spread') IS NOT NULL
     FROM identifiers
     WHERE name = ?1 AND kind = 'call' AND (path = ?2 OR path = ?3) AND start_line = ?4
       AND json_valid(metadata_json)";
const OWNED_METHODS_SQL: &str = "SELECT name, path, start_line, end_line FROM symbols
     WHERE parent_symbol_id = ?1 AND kind IN ('method', 'function', 'constructor')
     ORDER BY path, start_line";
const STALE_CHECK_AGE: Duration = Duration::from_secs(3600);
const MAX_TESTS: usize = 50;
const MAX_EXTENT_LINES: usize = 80;

static CHECK_LOCK: Mutex<()> = Mutex::new(());

struct Budget {
    started: Instant,
    limit: Duration,
}

impl Budget {
    fn spent(&self) -> bool {
        self.started.elapsed() >= self.limit
    }

    fn cut(&self, before: &str) -> String {
        format!(
            "Stopped at the {} s time budget before {before}.",
            self.limit.as_secs()
        )
    }
}

struct Base {
    diff_base: String,
    commit: Option<String>,
    rev: String,
    source: BaseSource,
}

fn short(sha: &str) -> String {
    sha.chars().take(10).collect()
}

fn resolve_base(git: &Git, request: BaseRequest<'_>) -> Result<Base, EditCheckError> {
    let named = |rev: &str, source: BaseSource, shown: String| {
        let sha = (!rev.is_empty() && !rev.starts_with('-'))
            .then(|| git.resolve(rev))
            .transpose()?
            .flatten()
            .ok_or_else(|| EditCheckError::UnknownRevision(rev.to_string()))?;
        Ok(Base {
            diff_base: sha.clone(),
            commit: Some(sha),
            rev: shown,
            source,
        })
    };
    match request {
        BaseRequest::Given(rev) => named(rev, BaseSource::Given, rev.to_string()),
        BaseRequest::Session(rev) => named(rev, BaseSource::Session, short(rev)),
        BaseRequest::Head => match git.resolve("HEAD")? {
            Some(sha) => Ok(Base {
                rev: short(&sha),
                diff_base: sha.clone(),
                commit: Some(sha),
                source: BaseSource::Head,
            }),
            None => Ok(Base {
                diff_base: git.empty_tree()?,
                commit: None,
                rev: "no commits".to_string(),
                source: BaseSource::Head,
            }),
        },
    }
}

/// A check up to its diff hash: the base, the changed files, and the hash of their content, with
/// no scan of the base. A caller that remembers the hash of an earlier check can drop a repeat
/// before [`PendingCheck::run`] pays for the scan and the report.
pub struct PendingCheck {
    git: Git,
    base: Base,
    files: Vec<ChangedFile>,
    files_cut: Option<String>,
    diff_hash: String,
    limits: EditLimits,
    started: Instant,
}

pub(crate) fn run(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    request: BaseRequest<'_>,
    limits: EditLimits,
) -> Result<EditCheck, EditCheckError> {
    prepare(workspace, conn, request, limits)?.run(workspace, db_path, conn)
}

pub(crate) fn prepare(
    workspace: &Workspace,
    conn: &Connection,
    request: BaseRequest<'_>,
    limits: EditLimits,
) -> Result<PendingCheck, EditCheckError> {
    let started = Instant::now();
    let root = workspace.canonical_root.as_path();
    let git = Git::open(root)?;
    let base = resolve_base(&git, request)?;
    let listed = git.changed_files(&base.diff_base)?;
    let candidates = indexed_candidates(conn, root, listed)?;
    let (files, files_cut) = cap_files(candidates, limits.files);
    let diff_hash = diff_hash(root, &base, &files);
    Ok(PendingCheck {
        git,
        base,
        files,
        files_cut,
        diff_hash,
        limits,
        started,
    })
}

impl PendingCheck {
    /// The hash of the base commit and of the content of each changed file.
    pub fn diff_hash(&self) -> &str {
        &self.diff_hash
    }

    /// Runs the rest of the check. `workspace` and `conn` are those that prepared it.
    pub fn run(
        self,
        workspace: &Workspace,
        db_path: &Path,
        conn: &Connection,
    ) -> Result<EditCheck, EditCheckError> {
        let budget = Budget {
            started: self.started,
            limit: self.limits.time,
        };
        let _serial = CHECK_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let root = workspace.canonical_root.as_path();
        let PendingCheck {
            git,
            base,
            files,
            files_cut,
            diff_hash,
            limits,
            ..
        } = self;
        let mut check = EditCheck {
            rev: base.rev.clone(),
            source: base.source,
            files_changed: files.len(),
            changes: Vec::new(),
            tests: Vec::new(),
            cuts: files_cut.into_iter().collect(),
            diff_hash,
        };
        if files.is_empty() {
            return Ok(check);
        }
        if budget.spent() {
            check.cuts.push(budget.cut("the changed files were read"));
            return Ok(check);
        }

        let hunks = git.new_side_hunks(&base.diff_base, &files)?;
        refresh_files(workspace, db_path, conn, &files, &budget);
        let Some(base_index) = BaseIndex::build(&git, workspace, &base, &files)? else {
            return Ok(check);
        };
        if budget.spent() {
            check.cuts.push(budget.cut("the shapes were compared"));
            return Ok(check);
        }

        let mut candidates = Detector {
            conn,
            root,
            base: &base_index,
        }
        .detect(&files)?;
        if candidates.len() > limits.symbols {
            for candidate in &mut candidates {
                candidate.uses = name_uses(conn, &candidate.rank_name)?;
            }
            candidates.sort_by(|a, b| {
                b.uses
                    .cmp(&a.uses)
                    .then_with(|| a.path.cmp(&b.path))
                    .then_with(|| a.line.cmp(&b.line))
            });
            check.cuts.push(format!(
                "Checked {} of {} changed symbols, those with the most uses of their name first.",
                limits.symbols,
                candidates.len()
            ));
            candidates.truncate(limits.symbols);
        }

        let mut lister = SiteLister::new(conn, root, &hunks);
        let total = candidates.len();
        for (checked, candidate) in candidates.into_iter().enumerate() {
            if budget.spent() {
                check.cuts.push(format!(
                    "Stopped at the {} s time budget after {checked} of {total} changed symbols.",
                    limits.time.as_secs()
                ));
                break;
            }
            match lister.list(&candidate) {
                Ok(listed) => {
                    for command in listed.tests {
                        if !check.tests.contains(&command) && check.tests.len() < MAX_TESTS {
                            check.tests.push(command);
                        }
                    }
                    check.changes.push(ShapeChange {
                        kind: candidate.kind,
                        display: candidate.display,
                        path: candidate.path,
                        line: candidate.line,
                        change: candidate.change,
                        in_diff: listed.placed.in_diff,
                        proven: listed.placed.proven,
                        likely: listed.placed.likely,
                        unresolved: listed.placed.unresolved,
                    });
                }
                Err(error) => check.cuts.push(format!(
                    "The sites of `{}` could not be listed: {error}.",
                    candidate.display
                )),
            }
        }
        check.changes.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.display.cmp(&b.display))
        });
        Ok(check)
    }
}

/// The changed files the check can say anything about: not excluded from the index, not a folder
/// (a submodule), and either in the index or of a kind the index holds.
fn indexed_candidates(
    conn: &Connection,
    root: &Path,
    files: Vec<ChangedFile>,
) -> Result<Vec<ChangedFile>, EditCheckError> {
    let mut kept = Vec::new();
    for file in files {
        let excluded = is_hard_excluded(&file.path)
            || file.base_path.as_deref().is_some_and(is_hard_excluded)
            || root.join(&file.path).is_dir();
        if excluded {
            continue;
        }
        let mut known = false;
        for path in std::iter::once(file.path.as_str()).chain(file.base_path.as_deref()) {
            known = known || file_is_indexed(conn, path)?;
        }
        if known || extension_is_indexed(conn, &file.path)? {
            kept.push(file);
        }
    }
    Ok(kept)
}

fn file_is_indexed(conn: &Connection, path: &str) -> Result<bool, EditCheckError> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM files WHERE path = ?1 OR path = ?2 LIMIT 1",
            params![path, path.replace('/', "\\")],
            |_| Ok(()),
        )
        .optional()
        .map_err(QueryError::from)?
        .is_some())
}

fn extension_is_indexed(conn: &Connection, path: &str) -> Result<bool, EditCheckError> {
    let Some(extension) = Path::new(path).extension().and_then(|e| e.to_str()) else {
        return Ok(false);
    };
    let pattern = format!("%.{}", escape_like(extension));
    Ok(conn
        .query_row(
            "SELECT 1 FROM files WHERE path LIKE ?1 ESCAPE '\\' LIMIT 1",
            [pattern],
            |_| Ok(()),
        )
        .optional()
        .map_err(QueryError::from)?
        .is_some())
}

fn cap_files(mut files: Vec<ChangedFile>, limit: usize) -> (Vec<ChangedFile>, Option<String>) {
    if files.len() <= limit {
        return (files, None);
    }
    let cut = format!("Checked {limit} of {} changed files.", files.len());
    files.truncate(limit);
    (files, Some(cut))
}

fn diff_hash(root: &Path, base: &Base, files: &[ChangedFile]) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(base.diff_base.as_bytes());
    for file in files {
        hasher.update(file.path.as_bytes());
        hasher.update(b"\0");
        match std::fs::read(root.join(&file.path)) {
            Ok(bytes) => hasher.update(blake3::hash(&bytes).as_bytes()),
            Err(_) => hasher.update(b"deleted"),
        };
        hasher.update(b"\0");
    }
    hasher.finalize().to_hex().to_string()
}

fn refresh_files(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    files: &[ChangedFile],
    budget: &Budget,
) {
    for file in files {
        let moved_from = file.base_path.as_deref().filter(|old| *old != file.path);
        for path in std::iter::once(file.path.as_str()).chain(moved_from) {
            if budget.spent() {
                return;
            }
            if let Err(error) = ensure_fresh_file(workspace, db_path, conn, path) {
                warn!(
                    path,
                    "could not refresh a changed file in the index: {error}"
                );
            }
        }
    }
}

/// The index of the changed files as they were at the base revision. It lives in a folder under
/// `.code-kb/check`, which `Drop` removes after the connection is closed.
struct BaseIndex {
    conn: Connection,
    tree: PathBuf,
    _folder: CheckFolder,
}

struct CheckFolder {
    path: PathBuf,
}

impl Drop for CheckFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

impl BaseIndex {
    /// Scans the base content of `files` into a side index. `None` when the base has no file the
    /// extractor can read, so no symbol can have changed shape.
    fn build(
        git: &Git,
        workspace: &Workspace,
        base: &Base,
        files: &[ChangedFile],
    ) -> Result<Option<Self>, EditCheckError> {
        let Some(commit) = base.commit.as_deref() else {
            return Ok(None);
        };
        let folder = CheckFolder::create(&workspace.canonical_root)?;
        let tree = folder.path.join("tree");
        let mut written = 0;
        for file in files {
            let Some(base_path) = file.base_path.as_deref() else {
                continue;
            };
            let Some(content) = git.base_blob(commit, base_path) else {
                continue;
            };
            if content.iter().take(8000).any(|&byte| byte == 0) || !is_plain_relative(&file.path) {
                continue;
            }
            let target = tree.join(&file.path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(target, content)?;
            written += 1;
        }
        if written == 0 {
            return Ok(None);
        }
        let db = folder.path.join("base.db");
        let allow_all = folder.path.join("allow.ignore");
        let spool = folder.path.join("spool");
        std::fs::write(&allow_all, "!*\n")?;
        std::fs::create_dir_all(&spool)?;
        let (tree_arg, db_arg) = (tree.to_string_lossy(), db.to_string_lossy());
        let (allow_arg, spool_arg) = (allow_all.to_string_lossy(), spool.to_string_lossy());
        let pid = std::process::id().to_string();
        let mut args = vec![
            "scan",
            "--root",
            &tree_arg,
            "--db",
            &db_arg,
            "--level",
            sync::EXTRACTION_LEVEL,
            "--ignore-file",
            &allow_arg,
            "--spool-dir",
            &spool_arg,
        ];
        if cfg!(unix) {
            args.extend(["--parent-pid", &pid]);
        }
        match sync::execute_julie_extract(&args) {
            Ok(_) => {}
            Err(SyncError::ExtractionFailed(1, stderr))
                if stderr.starts_with("partial") && db.exists() => {}
            Err(error) => return Err(error.into()),
        }
        ensure_fts_index_path(&db)?;
        Ok(Some(BaseIndex {
            conn: open_read_only(&db)?,
            tree,
            _folder: folder,
        }))
    }
}

impl CheckFolder {
    /// Creates `<root>/.code-kb/check/<pid>` empty, after removing the folders that processes
    /// which died left behind.
    fn create(root: &Path) -> Result<Self, EditCheckError> {
        sync::ensure_index_dir(&root.join(".code-kb").join("check"))?;
        let checks = root.join(".code-kb").join("check");
        sweep_stale(&checks);
        let path = checks.join(std::process::id().to_string());
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
        std::fs::create_dir_all(&path)?;
        Ok(CheckFolder { path })
    }
}

fn sweep_stale(checks: &Path) {
    let Ok(entries) = std::fs::read_dir(checks) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > STALE_CHECK_AGE);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn is_plain_relative(path: &str) -> bool {
    let path = Path::new(path);
    path.is_relative()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
}

/// A symbol of one side of the comparison with the names of the symbols that hold it.
struct Row {
    symbol: Symbol,
    parents: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Key {
    parents: Vec<String>,
    kind: String,
    name: String,
}

impl Row {
    fn key(&self) -> Key {
        Key {
            parents: self.parents.clone(),
            kind: self.symbol.kind.clone(),
            name: self.symbol.name.clone(),
        }
    }

    /// A julie lambda is named after its line, so a line shift would read as a rename. A
    /// constant, variable, or type alias counts only at the top level and when another file can
    /// import it.
    fn is_shape(&self) -> bool {
        let symbol = &self.symbol;
        let kind = symbol.kind.as_str();
        let exported_binding = BINDING_KINDS.contains(&kind)
            && self.parents.is_empty()
            && symbol.visibility.as_deref() == Some("public")
            && !DOCUMENTATION_LANGUAGES.contains(&symbol.language.as_str());
        (CALLABLE_KINDS.contains(&kind) || TYPE_KINDS.contains(&kind) || exported_binding)
            && !is_generated_lambda_name(
                &symbol.name,
                &symbol.language,
                kind,
                symbol.signature.as_deref(),
            )
    }

    fn display(&self) -> String {
        match self.parents.last() {
            Some(owner) => format!("{owner}.{}", self.symbol.name),
            None => self.symbol.name.clone(),
        }
    }

    fn changed_kind_to(&self, other: &Row) -> bool {
        other.symbol.name == self.symbol.name
            && other.symbol.kind != self.symbol.kind
            && other.parents == self.parents
            && other.is_shape()
            && KIND_GROUPS.iter().any(|group| {
                group.contains(&self.symbol.kind.as_str())
                    && group.contains(&other.symbol.kind.as_str())
            })
    }
}

const IMPL_OWNERS_SQL: &str = "SELECT symbol_id,
            CASE WHEN json_valid(metadata_json)
                 THEN json_extract(metadata_json, '$.impl_type_name') END
     FROM symbols
     WHERE path = ?1 OR path = ?2";

/// The symbols of `path` with their parent names. A Rust method whose `impl` block is in another
/// file than its type keeps the type named in its metadata, linked to a parent row or not.
fn rows_of(conn: &Connection, path: &str) -> Result<Vec<Row>, QueryError> {
    let symbols = load_file_symbols(conn, path)?;
    let mut stmt = conn.prepare(IMPL_OWNERS_SQL)?;
    let owners: HashMap<String, String> = stmt
        .query_map(params![path, path.replace('/', "\\")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter_map(|(id, owner)| owner.map(|owner| (id, owner)))
        .collect();
    let names: HashMap<&str, (&str, Option<&str>)> = symbols
        .iter()
        .map(|s| {
            (
                s.symbol_id.as_str(),
                (s.name.as_str(), s.parent_symbol_id.as_deref()),
            )
        })
        .collect();
    let mut rows = Vec::with_capacity(symbols.len());
    for symbol in &symbols {
        let mut parents = Vec::new();
        let mut next = symbol.parent_symbol_id.as_deref();
        while let Some(id) = next.filter(|_| parents.len() < 32) {
            let Some((name, parent)) = names.get(id) else {
                break;
            };
            parents.push((*name).to_string());
            next = *parent;
        }
        if parents.is_empty()
            && let Some(owner) = owners.get(&symbol.symbol_id)
        {
            parents.push(owner.clone());
        }
        parents.reverse();
        rows.push(Row {
            symbol: symbol.clone(),
            parents,
        });
    }
    rows.sort_by_key(|row| row.symbol.start_byte);
    Ok(rows)
}

/// The name tests take as a parameter for a Python function under a `fixture` decorator.
fn pytest_fixture_name(symbol: &Symbol) -> Option<String> {
    let signature = symbol.signature.as_deref()?;
    let decorated = signature
        .lines()
        .any(|line| line.trim_start().starts_with('@') && line.contains("fixture"));
    (symbol.language == "python" && decorated)
        .then(|| declared_fixture_name(signature).unwrap_or_else(|| symbol.name.clone()))
}

const FIXTURE_DEFINITIONS_SQL: &str = "SELECT replace(path, '\\', '/') FROM symbols
     WHERE name = ?1 AND language = 'python' AND kind IN ('function', 'method')
       AND signature LIKE '%fixture%'";

const FIXTURE_PARAMETERS_SQL: &str = "SELECT parameter.path, parameter.start_line, t.name,
            COALESCE(t.body_start_line, t.start_line), t.end_line
     FROM symbols parameter
     JOIN symbols t ON t.symbol_id = parameter.parent_symbol_id
     WHERE parameter.name = ?1
       AND (parameter.kind = 'parameter'
            OR (parameter.kind = 'variable' AND parameter.start_byte < t.body_start_byte))
       AND t.is_test = 1 AND t.language = 'python'
       AND replace(t.path, '\\', '/') LIKE ?2 ESCAPE '\\'
     ORDER BY parameter.path, parameter.start_line";

const PARAMETER_READS_SQL: &str = "SELECT DISTINCT start_line FROM identifiers
     WHERE name = ?1 AND path = ?2 AND start_line BETWEEN ?3 AND ?4
     ORDER BY start_line";

type FixtureUser = (String, usize, String, usize, usize);

/// The paths of the pytest fixtures named `name`, and the parameters named `name` of the tests
/// and fixtures under `scope` as `(path, line, function, first body line, last line)`.
fn fixture_rows(
    conn: &Connection,
    name: &str,
    scope: &str,
) -> rusqlite::Result<(Vec<String>, Vec<FixtureUser>)> {
    let overrides = conn
        .prepare(FIXTURE_DEFINITIONS_SQL)?
        .query_map(params![name], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let users = conn
        .prepare(FIXTURE_PARAMETERS_SQL)?
        .query_map(params![name, scope], |row| {
            Ok((
                row.get(0)?,
                row.get::<_, i64>(1)? as usize,
                row.get(2)?,
                row.get::<_, i64>(3)? as usize,
                row.get::<_, i64>(4)? as usize,
            ))
        })?
        .collect::<Result<_, _>>()?;
    Ok((overrides, users))
}

fn signature_tail(symbol: &Symbol) -> String {
    signatures::tail_of(&signatures::signature_of(symbol), &symbol.name)
}

fn word_position(text: &str, word: &str) -> Option<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word)
        .map(|(at, _)| at)
        .find(|&at| !text[..at].ends_with(is_word) && !text[at + word.len()..].starts_with(is_word))
}

struct Pairing {
    pairs: Vec<(usize, usize)>,
    removed: Vec<usize>,
    added: Vec<usize>,
}

/// Matches the symbols of `base` with those of `current` that share a key. Equal counts pair in
/// source order; otherwise equal signatures pair first, so a new overload does not shift the
/// others. What stays unpaired is removed or added.
fn pair_rows(base: &[Row], current: &[Row]) -> Pairing {
    let group = |rows: &[Row]| {
        let mut groups: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
        for (index, row) in rows.iter().enumerate().filter(|(_, row)| row.is_shape()) {
            groups.entry(row.key()).or_default().push(index);
        }
        groups
    };
    let (base_groups, current_groups) = (group(base), group(current));
    let mut pairing = Pairing {
        pairs: Vec::new(),
        removed: Vec::new(),
        added: Vec::new(),
    };
    let keys: BTreeSet<&Key> = base_groups.keys().chain(current_groups.keys()).collect();
    for key in keys {
        let mut before: Vec<usize> = base_groups.get(key).cloned().unwrap_or_default();
        let mut after: Vec<usize> = current_groups.get(key).cloned().unwrap_or_default();
        if before.len() != after.len() {
            let mut unmatched = Vec::new();
            for &old in &before {
                let same = after.iter().position(|&new| {
                    signature_tail(&current[new].symbol) == signature_tail(&base[old].symbol)
                });
                match same {
                    Some(position) => pairing.pairs.push((old, after.remove(position))),
                    None => unmatched.push(old),
                }
            }
            before = unmatched;
        }
        let common = before.len().min(after.len());
        pairing
            .pairs
            .extend(before.iter().copied().zip(after.iter().copied()));
        pairing.removed.extend(&before[common..]);
        pairing.added.extend(&after[common..]);
    }
    pairing
}

/// The unpaired base symbols that an unpaired current symbol of the same parents, kind, and body
/// hash replaces, as `(base index, current index)`. A hash that more than one symbol on either
/// side has names no rename.
fn renames(base: &[Row], current: &[Row], pairing: &Pairing) -> Vec<(usize, usize)> {
    let side = |rows: &[Row], indexes: &[usize]| {
        let mut by_hash: HashMap<(Vec<String>, String, String), Vec<usize>> = HashMap::new();
        for &index in indexes {
            let symbol = &rows[index].symbol;
            if let Some(hash) = symbol.body_hash.as_deref().filter(|hash| !hash.is_empty()) {
                by_hash
                    .entry((
                        rows[index].parents.clone(),
                        symbol.kind.clone(),
                        hash.to_string(),
                    ))
                    .or_default()
                    .push(index);
            }
        }
        by_hash
    };
    let (removed, added) = (side(base, &pairing.removed), side(current, &pairing.added));
    let mut found: Vec<(usize, usize)> = removed
        .iter()
        .filter_map(
            |(key, old)| match (old.as_slice(), added.get(key)?.as_slice()) {
                ([old], [new]) => Some((*old, *new)),
                _ => None,
            },
        )
        .collect();
    found.sort_unstable();
    found
}

/// One symbol whose shape changed, before its sites are listed.
struct Candidate {
    kind: String,
    display: String,
    path: String,
    line: usize,
    change: String,
    subject: Symbol,
    what: What,
    rank_name: String,
    uses: usize,
}

enum What {
    Parameters,
    Signature,
    Renamed {
        renames: Vec<(String, String)>,
    },
    Removed {
        overloads: Option<Overloads>,
        /// The name tests take as a parameter when the symbol is a pytest fixture.
        fixture: Option<String>,
    },
    Fields(FieldChange),
    Variants {
        added: Vec<String>,
        removed: Vec<String>,
    },
}

/// The overloads that stay under the name of a callable that was removed or renamed, with the
/// argument counts each group accepts. A group has no range when the index cannot tell, and the
/// remaining group has none when an overload remains that the index does not list.
struct Overloads {
    remaining: Option<Arity>,
    removed: Option<Arity>,
}

/// Whether a call can reach an overload that remains.
enum Reach {
    Fits,
    Unknown,
    OnlyRemoved(u32),
}

impl Overloads {
    fn judge(&self, remaining: &Arity, language: &str, arguments: &CallArguments) -> Reach {
        let removed_takes = |count| {
            self.removed
                .as_ref()
                .is_none_or(|removed| removed.accepts(language, count) != Some(false))
        };
        let mut unknown = arguments.spread || arguments.counts.is_empty();
        for &count in &arguments.counts {
            match remaining.accepts(language, count) {
                Some(true) => unknown |= removed_takes(count),
                Some(false) if removed_takes(count) => return Reach::OnlyRemoved(count),
                Some(false) => {}
                None => unknown = true,
            }
        }
        if unknown { Reach::Unknown } else { Reach::Fits }
    }
}

/// The argument counts of the calls of one name on one line.
#[derive(Default)]
struct CallArguments {
    counts: Vec<u32>,
    spread: bool,
}

impl CallArguments {
    fn rejected_by(&self, range: &Arity, language: &str) -> Option<u32> {
        self.counts
            .iter()
            .copied()
            .find(|&count| range.accepts(language, count) == Some(false))
    }
}

struct OwnedMethod {
    name: String,
    path: String,
    first: usize,
    last: usize,
}

struct Detector<'a> {
    conn: &'a Connection,
    root: &'a Path,
    base: &'a BaseIndex,
}

impl Detector<'_> {
    fn detect(&self, files: &[ChangedFile]) -> Result<Vec<Candidate>, EditCheckError> {
        let mut candidates = Vec::new();
        for file in files {
            if file.base_path.is_none() {
                continue;
            }
            let before = rows_of(&self.base.conn, &file.path)?;
            if before.is_empty() {
                continue;
            }
            let after = rows_of(self.conn, &file.path)?;
            let unreadable = after.is_empty()
                && self.root.join(&file.path).exists()
                && !file_is_indexed(self.conn, &file.path)?;
            if unreadable {
                continue;
            }
            candidates.extend(self.compare(&before, &after)?);
        }
        Ok(candidates)
    }

    fn compare(&self, before: &[Row], after: &[Row]) -> Result<Vec<Candidate>, EditCheckError> {
        let pairing = pair_rows(before, after);
        let mut candidates = Vec::new();
        for &(old, new) in &pairing.pairs {
            if let Some(candidate) = self.changed_pair(before, old, after, new)? {
                candidates.push(candidate);
            }
        }
        let renamed: HashMap<usize, usize> = renames(before, after, &pairing).into_iter().collect();
        let gone: HashSet<&str> = pairing
            .removed
            .iter()
            .map(|&index| before[index].symbol.symbol_id.as_str())
            .collect();
        for &index in &pairing.removed {
            let row = &before[index];
            let inside_removed = row
                .symbol
                .parent_symbol_id
                .as_deref()
                .is_some_and(|parent| gone.contains(parent));
            let value_beside = row.symbol.kind == "type"
                && pairing.removed.iter().any(|&other| {
                    let other = &before[other];
                    matches!(other.symbol.kind.as_str(), "constant" | "variable")
                        && other.symbol.name == row.symbol.name
                        && other.parents == row.parents
                });
            if inside_removed
                || value_beside
                || after.iter().any(|other| row.changed_kind_to(other))
            {
                continue;
            }
            let renamed_to = renamed
                .get(&index)
                .map(|&new| after[new].symbol.name.as_str());
            candidates.push(self.removed(row, after, renamed_to)?);
        }
        Ok(candidates)
    }

    fn removed(
        &self,
        row: &Row,
        after: &[Row],
        renamed_to: Option<&str>,
    ) -> Result<Candidate, EditCheckError> {
        let owner = row.parents.split_last().and_then(|(name, above)| {
            after
                .iter()
                .find(|other| other.symbol.name == *name && other.parents == above)
        });
        let mut subject = row.symbol.clone();
        subject.symbol_id = format!("removed:{}", subject.symbol_id);
        subject.parent_symbol_id = owner.map(|owner| owner.symbol.symbol_id.clone());
        Ok(Candidate {
            kind: row.symbol.kind.clone(),
            display: row.display(),
            path: row.symbol.path.clone(),
            line: row.symbol.start_line,
            change: match renamed_to {
                Some(new) => format!("renamed to `{new}`"),
                None => "removed".to_string(),
            },
            rank_name: row.symbol.name.clone(),
            uses: 0,
            subject,
            what: What::Removed {
                overloads: self.remaining_overloads(row, after)?,
                fixture: pytest_fixture_name(&row.symbol),
            },
        })
    }

    /// The overloads of a removed callable that remain under its name in the same owner. A method
    /// that the index gives no owner (a Go method) shares its key with the methods of other
    /// types, so it has none. A constructor of a class that has one the index lists no symbol for
    /// keeps an overload with no range.
    fn remaining_overloads(
        &self,
        row: &Row,
        after: &[Row],
    ) -> Result<Option<Overloads>, EditCheckError> {
        let ownerless = row.symbol.kind == "method" && row.parents.is_empty();
        if !CALLABLE_KINDS.contains(&row.symbol.kind.as_str()) || ownerless {
            return Ok(None);
        }
        let key = row.key();
        let remaining: Vec<Symbol> = after
            .iter()
            .filter(|other| other.key() == key)
            .map(|other| other.symbol.clone())
            .collect();
        let hidden = row.symbol.kind == "constructor"
            && arity::has_unlisted_constructor(&self.base.conn, &row.symbol)?;
        if remaining.is_empty() && !hidden {
            return Ok(None);
        }
        Ok(Some(Overloads {
            remaining: if hidden {
                None
            } else {
                arity::declared_group_arity(self.conn, self.root, &remaining)?
            },
            removed: arity::declared_group_arity(
                &self.base.conn,
                &self.base.tree,
                std::slice::from_ref(&row.symbol),
            )?,
        }))
    }

    fn changed_pair(
        &self,
        before: &[Row],
        old: usize,
        after: &[Row],
        new: usize,
    ) -> Result<Option<Candidate>, EditCheckError> {
        let (was, is) = (&before[old], &after[new]);
        let kind = is.symbol.kind.as_str();
        let (change, what) = if CALLABLE_KINDS.contains(&kind) {
            match self.callable_change(&was.symbol, &is.symbol)? {
                Some(found) => found,
                None => return Ok(None),
            }
        } else if FIELD_OWNER_KINDS.contains(&kind) {
            let dataclass = is
                .symbol
                .signature
                .as_deref()
                .is_some_and(|sig| sig.starts_with("@dataclass"));
            let old_components = was
                .symbol
                .signature
                .as_deref()
                .filter(|_| is.symbol.language == "java")
                .and_then(|sig| fields::record_components(sig, &was.symbol.name));
            let is_field = |row: &Row| {
                (FIELD_KINDS.contains(&row.symbol.kind.as_str())
                    || dataclass && is_annotated_variable(&row.symbol))
                    && !row.symbol.signature.as_deref().is_some_and(|sig| {
                        ATTRIBUTE_WRITE_PREFIXES.iter().any(|p| sig.starts_with(p))
                    })
            };
            let (added, removed) = member_changes(before, &was.symbol, after, &is.symbol, is_field);
            if added.is_empty() && removed.is_empty() {
                return Ok(None);
            }
            let required = after.iter().any(|row| {
                row.symbol.parent_symbol_id.as_deref() == Some(&is.symbol.symbol_id)
                    && is_field(row)
                    && added.contains(&row.symbol.name)
                    && fields::needs_argument(row.symbol.signature.as_deref())
            });
            (
                format!("fields {}", member_text(&added, &removed)),
                What::Fields(FieldChange {
                    added,
                    removed,
                    required,
                    old_components,
                }),
            )
        } else if kind == "enum" {
            let c_enum = is.symbol.language == "c";
            let (added, removed) = member_changes(before, &was.symbol, after, &is.symbol, |row| {
                row.symbol.kind == "enum_member" || (c_enum && row.symbol.kind == "constant")
            });
            if added.is_empty() && removed.is_empty() {
                return Ok(None);
            }
            (
                format!("variants {}", member_text(&added, &removed)),
                What::Variants { added, removed },
            )
        } else {
            return Ok(None);
        };
        Ok(Some(Candidate {
            kind: is.symbol.kind.clone(),
            display: is.display(),
            path: is.symbol.path.clone(),
            line: is.symbol.start_line,
            change,
            rank_name: is.symbol.name.clone(),
            uses: 0,
            subject: is.symbol.clone(),
            what,
        }))
    }

    fn callable_change(
        &self,
        was: &Symbol,
        is: &Symbol,
    ) -> Result<Option<(String, What)>, EditCheckError> {
        let (before, after) = (
            self.parameter_shape(&self.base.conn, &self.base.tree, was)?,
            self.parameter_shape(self.conn, self.root, is)?,
        );
        if before.differs(&after) {
            return Ok(Some((before.change_text(&after), What::Parameters)));
        }
        let shapes = (
            Shape::of(&self.base.conn, was, &self.base.tree)?,
            Shape::of(self.conn, is, self.root)?,
        );
        Ok(match shapes.0.compare(&shapes.1, &is.language) {
            Difference::None => None,
            Difference::Signature => Some(("signature changed".to_string(), What::Signature)),
            Difference::Modifiers(words) => Some((
                format!("signature changed: {}", words.join(", ")),
                What::Signature,
            )),
            Difference::Renamed(renames) => {
                let pairs: Vec<String> = renames
                    .iter()
                    .map(|(old, new)| format!("{old} → {new}"))
                    .collect();
                Some((
                    format!("parameter renamed: {}", pairs.join(", ")),
                    What::Renamed { renames },
                ))
            }
        })
    }

    fn parameter_shape(
        &self,
        conn: &Connection,
        root: &Path,
        symbol: &Symbol,
    ) -> Result<ParameterShape, EditCheckError> {
        Ok(ParameterShape {
            range: arity::group_arity(conn, root, std::slice::from_ref(symbol), false)?,
            count: arity::parameter_count(conn, symbol)? + signatures::ignored_parameters(symbol),
        })
    }
}

/// What a caller must pass to a callable: the accepted range when the index knows it, else the
/// number of parameter rows.
struct ParameterShape {
    range: Option<Arity>,
    count: usize,
}

impl ParameterShape {
    fn differs(&self, other: &ParameterShape) -> bool {
        match (&self.range, &other.range) {
            (Some(a), Some(b)) => a != b,
            _ => self.count != other.count,
        }
    }

    /// The range when both sides have one, else the row count, so the two texts compare alike.
    fn text(&self, other: &ParameterShape) -> String {
        match (&self.range, &other.range) {
            (Some(range), Some(_)) => range.range_text(),
            _ => self.count.to_string(),
        }
    }

    fn change_text(&self, other: &ParameterShape) -> String {
        parameters_change(
            (self.text(other), other.text(self)),
            (self.count, other.count),
        )
    }
}

/// `parameters 2 → 3` from the two range texts. When they read alike (a `self` parameter that came
/// or went), the row counts say it, and `parameters changed` when those are equal too.
fn parameters_change((was, is): (String, String), (was_rows, is_rows): (usize, usize)) -> String {
    if was != is {
        return format!("parameters {was} → {is}");
    }
    if was_rows != is_rows {
        return format!("parameters {was_rows} → {is_rows}");
    }
    "parameters changed".to_string()
}

/// A class-body variable with a type annotation that is no `ClassVar`, which is a field of a
/// Python dataclass.
fn is_annotated_variable(symbol: &Symbol) -> bool {
    symbol.kind == "variable"
        && symbol
            .signature
            .as_deref()
            .is_some_and(fields::is_dataclass_field)
}

/// The names of the members of `was` that `is` lacks and the names it gains, among the children
/// of each that `member` accepts.
fn member_changes(
    before: &[Row],
    was: &Symbol,
    after: &[Row],
    is: &Symbol,
    member: impl Fn(&Row) -> bool,
) -> (Vec<String>, Vec<String>) {
    let names = |rows: &[Row], owner: &Symbol| -> BTreeSet<String> {
        rows.iter()
            .filter(|row| row.symbol.parent_symbol_id.as_deref() == Some(&owner.symbol_id))
            .filter(|row| member(row))
            .map(|row| row.symbol.name.clone())
            .collect()
    };
    let (old, new) = (names(before, was), names(after, is));
    (
        new.difference(&old).cloned().collect(),
        old.difference(&new).cloned().collect(),
    )
}

/// `+1 (z), -0`: how many members a type gained and lost, and the names of each.
fn member_text(added: &[String], removed: &[String]) -> String {
    let part = |sign: char, names: &[String]| {
        if names.is_empty() {
            format!("{sign}0")
        } else {
            format!("{sign}{} ({})", names.len(), names.join(", "))
        }
    };
    format!("{}, {}", part('+', added), part('-', removed))
}

fn name_uses(conn: &Connection, name: &str) -> Result<usize, EditCheckError> {
    let count: i64 = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM identifiers WHERE name = ?1)
                  + (SELECT COUNT(*) FROM pending_relationships WHERE target_terminal_name = ?1)",
            [name],
            |row| row.get(0),
        )
        .map_err(QueryError::from)?;
    Ok(count.max(0) as usize)
}

#[derive(Default)]
struct Placed {
    in_diff: Vec<EditSite>,
    proven: Vec<EditSite>,
    likely: Vec<EditSite>,
    unresolved: usize,
}

impl Placed {
    fn add(&mut self, label: SiteLabel, reached: bool, site: EditSite) {
        if reached {
            self.in_diff.push(site);
            return;
        }
        match label {
            SiteLabel::Proven => self.proven.push(site),
            SiteLabel::Likely => self.likely.push(site),
            SiteLabel::Unresolved => self.unresolved += 1,
            SiteLabel::Excluded => {}
        }
    }
}

struct Listed {
    placed: Placed,
    tests: Vec<String>,
}

struct SiteLister<'a> {
    conn: &'a Connection,
    root: &'a Path,
    hunks: &'a Hunks,
    sources: HashMap<String, Option<Vec<String>>>,
    languages: HashMap<String, String>,
}

fn is_call(kind: &str) -> bool {
    matches!(kind, "calls" | "call" | "instantiates")
}

fn site_span(labeled: &LabeledSite) -> (usize, usize) {
    let first = labeled.site.start_line.unwrap_or(0);
    (first, labeled.end_line.unwrap_or(first).max(first))
}

fn args_noun(count: u32) -> &'static str {
    if count == 1 { "arg" } else { "args" }
}

fn edit_site(labeled: &LabeledSite, line: usize, note: Option<String>) -> EditSite {
    EditSite {
        path: labeled.site.path.clone(),
        line,
        caller: displayed_symbol_name(
            &labeled.site.from_symbol_name,
            labeled.site.enclosing_symbol_name.as_deref(),
        ),
        note,
    }
}

impl<'a> SiteLister<'a> {
    fn new(conn: &'a Connection, root: &'a Path, hunks: &'a Hunks) -> Self {
        Self {
            conn,
            root,
            hunks,
            sources: HashMap::new(),
            languages: HashMap::new(),
        }
    }

    fn list(&mut self, candidate: &Candidate) -> Result<Listed, EditCheckError> {
        let report = reference_report_for_edit(self.conn, self.root, &candidate.subject)?;
        let placed = match &candidate.what {
            What::Parameters => {
                let range = inherited_group_arity(self.conn, self.root, &candidate.subject)?;
                let mut placed = self.calls(&report, range.as_ref());
                self.overrides(&report, &mut placed)?;
                placed
            }
            What::Signature => {
                let mut placed = self.calls(&report, None);
                self.overrides(&report, &mut placed)?;
                placed
            }
            What::Renamed { renames } => self.named_calls(&report, renames),
            What::Removed {
                overloads: Some(overloads),
                ..
            } => self.removed_overloads(&report, overloads),
            What::Removed {
                overloads: None,
                fixture,
            } => {
                let mut placed = self.every_site(&report);
                self.imports(&report, &mut placed);
                if let Some(name) = fixture {
                    self.fixture_users(name, &candidate.subject.path, &mut placed)?;
                }
                placed
            }
            What::Fields(change) => self.constructions(&report, &candidate.subject, change)?,
            What::Variants { added, removed } => {
                self.variant_uses(&report, &candidate.subject, added, removed)?
            }
        };
        let tests = report
            .tests
            .iter()
            .filter_map(|test| test.command.clone())
            .collect();
        Ok(Listed { placed, tests })
    }

    fn calls(&mut self, report: &ReferenceReport, range: Option<&Arity>) -> Placed {
        let mut placed = Placed::default();
        for labeled in report
            .sites
            .iter()
            .filter(|site| site.label != SiteLabel::Excluded && is_call(&site.site.kind))
        {
            let (first, last) = site_span(labeled);
            let path = labeled.site.path.as_str();
            let touched = self.touches(path, first, last);
            let language = self.language_of(path);
            let arguments = range.map(|_| self.arguments(labeled)).unwrap_or_default();
            let count = range
                .and_then(|range| arguments.rejected_by(range, &language))
                .or_else(|| arguments.counts.first().copied());
            let verdict = range
                .zip(count)
                .and_then(|(range, count)| Some((range, count, range.accepts(&language, count)?)));
            if !touched && verdict.is_some_and(|(_, _, fits)| fits) {
                continue;
            }
            if !touched && verdict.is_none() && arguments.spread {
                placed.unresolved += 1;
                continue;
            }
            let rejected = verdict.filter(|(_, _, fits)| !fits);
            let note = rejected.map(|(range, count, _)| {
                format!(
                    "passes {count} {}; takes {}",
                    args_noun(count),
                    range.range_text()
                )
            });
            placed.add(
                labeled.label,
                touched && rejected.is_none(),
                edit_site(labeled, first, note),
            );
        }
        placed
    }

    /// The argument counts of the call at the site: the count the report gives, else the counts of
    /// the calls of the name on its line. A line with two calls of one name gets no count in the
    /// report, so the call rows are read.
    fn arguments(&mut self, labeled: &LabeledSite) -> CallArguments {
        if let Some(count) = labeled.arg_count {
            return CallArguments {
                counts: vec![count],
                spread: false,
            };
        }
        if !is_call(&labeled.site.kind) {
            return CallArguments::default();
        }
        let mut arguments = self.line_arguments(labeled).unwrap_or_default();
        if arguments.counts.is_empty() && !arguments.spread {
            arguments.counts = self.written_counts(labeled);
        }
        arguments
    }

    /// The argument counts of the Java calls of the name on the line of the site, read from the
    /// source, because julie writes no count for `new Foo(a)`.
    fn written_counts(&mut self, labeled: &LabeledSite) -> Vec<u32> {
        let path = labeled.site.path.as_str();
        if self.language_of(path) != "java" {
            return Vec::new();
        }
        let line = labeled.site.start_line.unwrap_or(0);
        self.source(path)
            .map(|lines| fields::call_counts(lines, line, &labeled.site.to_symbol_name))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|count| u32::try_from(count).ok())
            .collect()
    }

    fn line_arguments(&self, labeled: &LabeledSite) -> Option<CallArguments> {
        let path = labeled.site.path.as_str();
        let mut stmt = self.conn.prepare(CALL_ARGUMENTS_SQL).ok()?;
        let rows = stmt
            .query_map(
                params![
                    labeled.site.to_symbol_name,
                    path,
                    path.replace('/', "\\"),
                    labeled.site.start_line.unwrap_or(0) as i64
                ],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, bool>(1)?)),
            )
            .ok()?;
        let mut arguments = CallArguments::default();
        for (count, spread) in rows.filter_map(Result::ok) {
            arguments.spread |= spread;
            if let Some(count) = count
                .filter(|_| !spread)
                .and_then(|c| u32::try_from(c).ok())
            {
                arguments.counts.push(count);
            }
        }
        Some(arguments)
    }

    /// The sites of a removed or renamed overload when other overloads of the name remain. A call
    /// that only a remaining overload takes is no site, a call that only the removed one took is,
    /// and a call that both take, a call with no known count, and every call while the remaining
    /// overloads have no range are counted as unresolved.
    fn removed_overloads(&mut self, report: &ReferenceReport, overloads: &Overloads) -> Placed {
        let mut placed = Placed::default();
        for labeled in report
            .sites
            .iter()
            .filter(|site| site.label != SiteLabel::Excluded)
        {
            let (first, last) = site_span(labeled);
            let path = labeled.site.path.as_str();
            let language = self.language_of(path);
            let reach = match &overloads.remaining {
                Some(remaining) => overloads.judge(remaining, &language, &self.arguments(labeled)),
                None => Reach::Unknown,
            };
            match reach {
                Reach::Fits => {}
                Reach::Unknown => {
                    placed.unresolved += usize::from(!self.touches(path, first, last));
                }
                Reach::OnlyRemoved(count) => {
                    let note = format!(
                        "passes {count} {}; no remaining overload takes it",
                        args_noun(count)
                    );
                    placed.add(labeled.label, false, edit_site(labeled, first, Some(note)));
                }
            }
        }
        placed
    }

    /// The classes that implement or extend the owner of a changed method, each with its own
    /// version of the method. The section gives no label, so a row is `likely`.
    fn overrides(
        &self,
        report: &ReferenceReport,
        placed: &mut Placed,
    ) -> Result<(), EditCheckError> {
        for row in &report.overrides {
            let (first, last) = self
                .symbol_span(&row.symbol_id)?
                .unwrap_or((row.line, row.line));
            let reached = self.touches(&row.path, first, last);
            placed.add(
                SiteLabel::Likely,
                reached,
                EditSite {
                    path: row.path.clone(),
                    line: row.line,
                    caller: format!("{}.{}", row.owner, row.name),
                    note: None,
                },
            );
        }
        Ok(())
    }

    /// The import lines of a removed symbol's name that name its file, each `likely`.
    fn imports(&self, report: &ReferenceReport, placed: &mut Placed) {
        for (path, line) in &report.imports {
            let reached = self.touches(path, *line, *line);
            placed.add(
                SiteLabel::Likely,
                reached,
                EditSite {
                    path: path.clone(),
                    line: *line,
                    caller: String::new(),
                    note: Some(format!("imports `{}`", report.target.name)),
                },
            );
        }
    }

    /// The calls that pass an old parameter name as a named argument. A call that still does so
    /// is stale even when the diff touched its line.
    fn named_calls(&mut self, report: &ReferenceReport, renames: &[(String, String)]) -> Placed {
        let mut placed = Placed::default();
        for labeled in report
            .sites
            .iter()
            .filter(|site| site.label != SiteLabel::Excluded && is_call(&site.site.kind))
        {
            let (first, last) = site_span(labeled);
            let path = labeled.site.path.as_str();
            let callee = labeled.site.to_symbol_name.as_str();
            let language = self.language_of(path);
            let Some(lines) = self.source(path) else {
                continue;
            };
            let last = last.max(construction_end(lines, first, callee));
            let span: Vec<&str> = lines
                .get(first.saturating_sub(1)..last.min(lines.len()))
                .unwrap_or_default()
                .iter()
                .map(String::as_str)
                .collect();
            let named = renames
                .iter()
                .find(|(old, _)| signatures::names_parameter(&span, callee, old, &language));
            if let Some((old, _)) = named {
                let note = Some(format!("names parameter `{old}`"));
                placed.add(labeled.label, false, edit_site(labeled, first, note));
            }
        }
        placed
    }

    /// The tests and fixtures in the scope of a pytest fixture that take it as a parameter, and
    /// the reads of that parameter in their bodies. A user that a fixture of the same name in its
    /// own file or in a nearer `conftest.py` serves is left out.
    fn fixture_users(
        &self,
        name: &str,
        fixture_path: &str,
        placed: &mut Placed,
    ) -> Result<(), EditCheckError> {
        let fixture_path = fixture_path.replace('\\', "/");
        let fixture_dir = fixture_path
            .strip_suffix("conftest.py")
            .map(|dir| dir.trim_end_matches('/'));
        let scope = match fixture_dir {
            Some("") => "%".to_string(),
            Some(dir) => format!("{}/%", escape_like(dir)),
            None => escape_like(&fixture_path),
        };
        let (overrides, users) = fixture_rows(self.conn, name, &scope).map_err(QueryError::from)?;
        let depth = fixture_dir.map_or(usize::MAX, str::len);
        let served_elsewhere = |path: &str| {
            overrides.iter().any(|other| {
                other == path
                    || other.strip_suffix("conftest.py").is_some_and(|dir| {
                        let dir = dir.trim_end_matches('/');
                        dir.len() > depth && path.starts_with(&format!("{dir}/"))
                    })
            })
        };
        let mut listed: HashSet<(String, usize)> =
            [&placed.in_diff, &placed.proven, &placed.likely]
                .into_iter()
                .flatten()
                .map(|site| (site.path.clone(), site.line))
                .collect();
        let mut reads = self
            .conn
            .prepare(PARAMETER_READS_SQL)
            .map_err(QueryError::from)?;
        for (path, line, caller, first, last) in users {
            let shown = path.replace('\\', "/");
            if served_elsewhere(&shown) {
                continue;
            }
            let read_lines: Vec<i64> = reads
                .query_map(params![name, path, first as i64, last as i64], |row| {
                    row.get(0)
                })
                .and_then(Iterator::collect)
                .map_err(QueryError::from)?;
            let note = format!("takes the fixture `{name}`");
            let sites = std::iter::once((line, Some(note)))
                .chain(read_lines.into_iter().map(|read| (read as usize, None)));
            for (line, note) in sites {
                if !listed.insert((shown.clone(), line)) {
                    continue;
                }
                let reached = self.touches(&shown, line, line);
                placed.add(
                    SiteLabel::Proven,
                    reached,
                    EditSite {
                        path: shown.clone(),
                        line,
                        caller: caller.clone(),
                        note,
                    },
                );
            }
        }
        Ok(())
    }

    fn every_site(&mut self, report: &ReferenceReport) -> Placed {
        let mut placed = Placed::default();
        for labeled in report
            .sites
            .iter()
            .filter(|site| site.label != SiteLabel::Excluded)
        {
            let (first, last) = site_span(labeled);
            let reached = self.touches(&labeled.site.path, first, last);
            placed.add(labeled.label, reached, edit_site(labeled, first, None));
        }
        placed
    }

    fn constructions(
        &mut self,
        report: &ReferenceReport,
        subject: &Symbol,
        change: &FieldChange,
    ) -> Result<Placed, EditCheckError> {
        let rust_struct = subject.language == "rust" && subject.kind == "struct";
        let mut rows: Vec<ConstructionSite> = report.constructions.clone();
        if rust_struct {
            for pattern in destructurings(self.root, &report.target.name, &report.sites) {
                if !rows
                    .iter()
                    .any(|row| row.path == pattern.path && row.line == pattern.line)
                {
                    rows.push(pattern);
                }
            }
        }
        let mut placed = Placed::default();
        for construction in &rows {
            let site = report
                .sites
                .iter()
                .filter(|site| {
                    site.label != SiteLabel::Excluded
                        && site.site.path == construction.path
                        && site.site.start_line == Some(construction.line)
                })
                .min_by_key(|site| site.label.rank());
            let lines = self.source(&construction.path).map(Vec::as_slice);
            if !change.breaks(subject, lines, construction.line) {
                continue;
            }
            let last = self
                .source(&construction.path)
                .map_or(construction.line, |lines| {
                    construction_end(lines, construction.line, &report.target.name)
                });
            let reached = self.touches(&construction.path, construction.line, last);
            let caller = site
                .map(|site| {
                    displayed_symbol_name(
                        &site.site.from_symbol_name,
                        site.site.enclosing_symbol_name.as_deref(),
                    )
                })
                .unwrap_or_default();
            placed.add(
                site.map_or(SiteLabel::Likely, |site| site.label),
                reached,
                EditSite {
                    path: construction.path.clone(),
                    line: construction.line,
                    caller,
                    note: None,
                },
            );
        }
        if rust_struct {
            self.self_literals(subject, change, &rows, &mut placed)?;
        }
        Ok(placed)
    }

    /// The `Self { … }` literals and patterns in the methods of a Rust struct, which no
    /// `Constructions` row holds, because they do not name the struct.
    fn self_literals(
        &mut self,
        subject: &Symbol,
        change: &FieldChange,
        listed: &[ConstructionSite],
        placed: &mut Placed,
    ) -> Result<(), EditCheckError> {
        for method in self.owned_methods(&subject.symbol_id)? {
            for line in method.first..=method.last {
                let Some(lines) = self.source(&method.path) else {
                    break;
                };
                let names_self = lines
                    .get(line - 1)
                    .is_some_and(|text| word_position(text, "Self").is_some());
                let already = listed
                    .iter()
                    .any(|row| row.path == method.path && row.line == line);
                if !names_self || already || !change.breaks_self(lines, line) {
                    continue;
                }
                let last = construction_end(lines, line, "Self");
                let reached = self.touches(&method.path, line, last);
                placed.add(
                    SiteLabel::Proven,
                    reached,
                    EditSite {
                        path: method.path.clone(),
                        line,
                        caller: format!("{}.{}", subject.name, method.name),
                        note: None,
                    },
                );
            }
        }
        Ok(())
    }

    fn owned_methods(&self, owner_id: &str) -> Result<Vec<OwnedMethod>, EditCheckError> {
        let mut stmt = self
            .conn
            .prepare(OWNED_METHODS_SQL)
            .map_err(QueryError::from)?;
        let methods = stmt
            .query_map([owner_id], |row| {
                Ok(OwnedMethod {
                    name: row.get(0)?,
                    path: row.get::<_, String>(1)?.replace('\\', "/"),
                    first: row.get::<_, i64>(2)?.max(1) as usize,
                    last: row.get::<_, i64>(3)?.max(1) as usize,
                })
            })
            .map_err(QueryError::from)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(QueryError::from)?;
        Ok(methods)
    }

    /// The functions that name a removed variant, each at its first use of one and never in the
    /// diff, then the functions that need an arm for an added variant.
    fn variant_uses(
        &mut self,
        report: &ReferenceReport,
        subject: &Symbol,
        added: &[String],
        removed: &[String],
    ) -> Result<Placed, EditCheckError> {
        let mut placed = Placed::default();
        let mut listed = HashSet::new();
        for used in uses_of_variants(self.conn, self.root, subject, &report.sites, removed)? {
            let span = self.symbol_span(&used.caller_id)?;
            let label = variant_label(report, &used, span);
            listed.insert((used.path.clone(), used.caller_id.clone()));
            placed.add(label, false, variant_site(&used));
        }
        if added.is_empty() {
            return Ok(placed);
        }
        let variants = enum_variant_names(self.conn, subject)?;
        for used in &report.variant_uses {
            if listed.contains(&(used.path.clone(), used.caller_id.clone())) {
                continue;
            }
            let span = self.symbol_span(&used.caller_id)?;
            let (first, last) = span.unwrap_or((used.line, used.line));
            let reached = self.names_in_diff(&used.path, first, last, added);
            match self.needs_arm(&used.path, span, &report.target.name, &variants) {
                Verdict::Unresolved => placed.unresolved += usize::from(!reached),
                Verdict::Site => {
                    let label = variant_label(report, used, span);
                    placed.add(label, reached, variant_site(used));
                }
            }
        }
        Ok(placed)
    }

    /// Whether the function that spans `span` of `path` needs a new arm for a new variant of
    /// the enum `enum_name`, judged from its lines.
    fn needs_arm(
        &mut self,
        path: &str,
        span: Option<(usize, usize)>,
        enum_name: &str,
        variants: &[String],
    ) -> Verdict {
        let Some((first, last)) = span else {
            return Verdict::Unresolved;
        };
        let language = self.language_of(path);
        let Some(lines) = self.source(path) else {
            return Verdict::Unresolved;
        };
        let Some(function) = lines.get(first.saturating_sub(1)..last.min(lines.len())) else {
            return Verdict::Unresolved;
        };
        let bare = match language.as_str() {
            "java" | "c" | "cpp" => true,
            "rust" => lines.iter().any(|line| imports_variants(line, enum_name)),
            _ => false,
        };
        arms::judge(
            &language,
            function,
            &arms::EnumNames {
                enum_name,
                variants,
                bare,
            },
        )
    }

    fn touches(&self, path: &str, first: usize, last: usize) -> bool {
        self.hunks
            .get(path)
            .is_some_and(|ranges| ranges.iter().any(|&(a, b)| a <= last && first <= b))
    }

    /// Whether the diff changed a line of `first..=last` that names one of `added`.
    fn names_in_diff(&mut self, path: &str, first: usize, last: usize, added: &[String]) -> bool {
        let ranges = self.hunks.get(path).cloned().unwrap_or_default();
        let Some(lines) = self.source(path) else {
            return false;
        };
        ranges.iter().any(|&(a, b)| {
            (a.max(first)..=b.min(last)).any(|line| {
                lines.get(line.wrapping_sub(1)).is_some_and(|text| {
                    added.iter().any(|name| word_position(text, name).is_some())
                })
            })
        })
    }

    fn source(&mut self, path: &str) -> Option<&Vec<String>> {
        let root = self.root;
        self.sources
            .entry(path.to_string())
            .or_insert_with(|| {
                std::fs::read_to_string(root.join(path))
                    .ok()
                    .map(|text| text.lines().map(str::to_string).collect())
            })
            .as_ref()
    }

    fn language_of(&mut self, path: &str) -> String {
        if let Some(language) = self.languages.get(path) {
            return language.clone();
        }
        let language = self
            .conn
            .query_row(
                "SELECT language FROM files WHERE path = ?1 OR path = ?2",
                params![path, path.replace('/', "\\")],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or_default();
        self.languages.insert(path.to_string(), language.clone());
        language
    }

    fn symbol_span(&self, symbol_id: &str) -> Result<Option<(usize, usize)>, EditCheckError> {
        if symbol_id.is_empty() {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                "SELECT start_line, end_line FROM symbols WHERE symbol_id = ?1",
                [symbol_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as usize,
                        row.get::<_, i64>(1)? as usize,
                    ))
                },
            )
            .optional()
            .map_err(QueryError::from)?)
    }
}

/// The best label among the sites of the enum's name inside the function of `used`.
fn variant_label(
    report: &ReferenceReport,
    used: &VariantUse,
    span: Option<(usize, usize)>,
) -> SiteLabel {
    let (first, last) = span.unwrap_or((used.line, used.line));
    report
        .sites
        .iter()
        .filter(|site| {
            site.label != SiteLabel::Excluded
                && site.site.path == used.path
                && site
                    .site
                    .start_line
                    .is_some_and(|line| (first..=last).contains(&line))
        })
        .map(|site| site.label)
        .min_by_key(|label| label.rank())
        .unwrap_or(SiteLabel::Likely)
}

fn variant_site(used: &VariantUse) -> EditSite {
    EditSite {
        path: used.path.clone(),
        line: used.line,
        caller: used.caller.clone(),
        note: None,
    }
}

/// Whether `line` is a `use` item that brings in variants of `enum_name` (`use a::Kind::*;`,
/// `use a::Kind::{A, B};`, `use a::Kind::A;`).
fn imports_variants(line: &str, enum_name: &str) -> bool {
    let item = line.trim_start();
    let item = item
        .strip_prefix("pub")
        .map_or(item, |rest| match rest.trim_start().strip_prefix('(') {
            Some(group) => group.split_once(')').map_or(rest, |(_, after)| after),
            None => rest,
        })
        .trim_start();
    item.starts_with("use ") && item.contains(&format!("{enum_name}::"))
}

/// The last line of the literal or call that starts at `line` and names `name`: the line of the
/// bracket that closes the first one opened after the name. `line` when the line opens none.
fn construction_end(lines: &[String], line: usize, name: &str) -> usize {
    let Some(first) = line.checked_sub(1).and_then(|index| lines.get(index)) else {
        return line;
    };
    let from = word_position(first, name).unwrap_or(0);
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, text) in lines
        .iter()
        .enumerate()
        .skip(line - 1)
        .take(MAX_EXTENT_LINES)
    {
        let text = if index == line - 1 {
            &first[from..]
        } else {
            text
        };
        for c in text.chars() {
            if let Some(open) = quote {
                match (escaped, c) {
                    (true, _) => escaped = false,
                    (false, '\\') => escaped = true,
                    (false, c) if c == open => quote = None,
                    _ => {}
                }
                continue;
            }
            match c {
                '"' | '\'' if depth > 0 => quote = Some(c),
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' if depth > 0 => {
                    depth -= 1;
                    if depth == 0 {
                        return index + 1;
                    }
                }
                _ => {}
            }
        }
        if depth == 0 {
            return line;
        }
    }
    line
}

#[cfg(test)]
fn test_symbol(language: &str, kind: &str, name: &str, signature: &str) -> Symbol {
    Symbol {
        symbol_id: name.to_string(),
        file_id: String::new(),
        path: "a.py".to_string(),
        language: language.to_string(),
        name: name.to_string(),
        kind: kind.to_string(),
        signature: Some(signature.to_string()),
        doc_comment: None,
        visibility: None,
        parent_symbol_id: None,
        start_line: 1,
        start_column: 0,
        end_line: 1,
        end_column: 0,
        start_byte: 0,
        end_byte: 0,
        body_start_line: None,
        body_start_column: None,
        body_end_line: None,
        body_end_column: None,
        body_start_byte: None,
        body_end_byte: None,
        body_hash: None,
        semantic_group: None,
        is_test: false,
        test_container: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        name: &str,
        kind: &str,
        signature: &str,
        hash: Option<&str>,
        parents: &[&str],
        start: usize,
    ) -> Row {
        Row {
            symbol: Symbol {
                symbol_id: format!("{name}{start}"),
                start_line: start,
                end_line: start,
                start_byte: start,
                end_byte: start,
                body_hash: hash.map(str::to_string),
                ..test_symbol("python", kind, name, signature)
            },
            parents: parents.iter().map(|p| p.to_string()).collect(),
        }
    }

    fn file(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_string(),
            base_path: Some(path.to_string()),
            untracked: false,
        }
    }

    #[test]
    fn more_files_than_the_limit_are_cut_and_the_cut_is_stated() {
        let files: Vec<ChangedFile> = (0..250).map(|n| file(&format!("f{n}.py"))).collect();
        let (kept, cut) = cap_files(files, 200);
        assert_eq!(kept.len(), 200);
        assert_eq!(cut.as_deref(), Some("Checked 200 of 250 changed files."));
        let (kept, cut) = cap_files(vec![file("a.py")], 200);
        assert_eq!((kept.len(), cut), (1, None));
    }

    #[test]
    fn a_signature_is_compared_from_its_name_and_without_modifiers() {
        let plain = row("add", "method", "int add(int a)", None, &[], 1);
        let modified = row(
            "add",
            "method",
            "@Override  public   final int add(int a)",
            None,
            &[],
            1,
        );
        assert_eq!(
            signature_tail(&plain.symbol),
            signature_tail(&modified.symbol)
        );
        let wider = row("add", "method", "long add(int a)", None, &[], 1);
        assert_ne!(signature_tail(&plain.symbol), signature_tail(&wider.symbol));
        let longer = row("add", "method", "int add(int a, int b)", None, &[], 1);
        assert_ne!(
            signature_tail(&plain.symbol),
            signature_tail(&longer.symbol)
        );
    }

    #[test]
    fn a_name_inside_another_word_is_not_the_signature_name() {
        let symbol = row("add", "function", "def padd(x)", None, &[], 1).symbol;
        assert_eq!(signature_tail(&symbol), "def padd(x)");
    }

    #[test]
    fn a_new_overload_does_not_shift_the_others() {
        let before = vec![
            row("f", "method", "f(int a)", None, &["K"], 1),
            row("f", "method", "f(int a, int b)", None, &["K"], 5),
        ];
        let after = vec![
            row("f", "method", "f(String s)", None, &["K"], 1),
            row("f", "method", "f(int a)", None, &["K"], 5),
            row("f", "method", "f(int a, int b)", None, &["K"], 9),
        ];
        let pairing = pair_rows(&before, &after);
        assert_eq!(pairing.pairs, vec![(0, 1), (1, 2)]);
        assert_eq!(pairing.added, vec![0]);
        assert!(pairing.removed.is_empty());
    }

    #[test]
    fn symbols_with_other_parents_or_kinds_do_not_pair() {
        let before = vec![row("f", "function", "f()", None, &[], 1)];
        let after = vec![
            row("f", "method", "f()", None, &["K"], 1),
            row("f", "class", "class f", None, &[], 4),
        ];
        let pairing = pair_rows(&before, &after);
        assert!(pairing.pairs.is_empty());
        assert_eq!(pairing.removed, vec![0]);
        assert_eq!(pairing.added.len(), 2);
    }

    #[test]
    fn a_unique_body_hash_names_a_rename_and_a_shared_one_does_not() {
        let before = vec![
            row("old", "function", "old()", Some("h1"), &[], 1),
            row("twin_a", "function", "twin_a()", Some("h2"), &[], 5),
        ];
        let after = vec![
            row("new", "function", "new()", Some("h1"), &[], 1),
            row("twin_b", "function", "twin_b()", Some("h2"), &[], 5),
            row("twin_c", "function", "twin_c()", Some("h2"), &[], 9),
        ];
        let pairing = pair_rows(&before, &after);
        assert_eq!(renames(&before, &after, &pairing), vec![(0, 0)]);
    }

    #[test]
    fn a_rename_needs_the_same_parents_kind_and_a_body_hash() {
        let before = vec![
            row("old", "function", "old()", Some("h1"), &["K"], 1),
            row("bare", "function", "bare()", None, &[], 5),
        ];
        let after = vec![
            row("new", "function", "new()", Some("h1"), &[], 1),
            row("fresh", "function", "fresh()", None, &[], 5),
        ];
        let pairing = pair_rows(&before, &after);
        assert!(renames(&before, &after, &pairing).is_empty());
    }

    #[test]
    fn parameters_with_different_ranges_show_the_ranges() {
        let change = parameters_change(("1".to_string(), "1-2".to_string()), (1, 2));
        assert_eq!(change, "parameters 1 → 1-2");
    }

    #[test]
    fn parameters_with_equal_ranges_and_different_row_counts_show_the_row_counts() {
        let change = parameters_change(("1".to_string(), "1".to_string()), (2, 1));
        assert_eq!(change, "parameters 2 → 1");
    }

    #[test]
    fn parameters_with_equal_ranges_and_equal_row_counts_say_they_changed() {
        let change = parameters_change(("1".to_string(), "1".to_string()), (1, 1));
        assert_eq!(change, "parameters changed");
    }

    #[test]
    fn member_text_counts_and_names_both_sides() {
        let none: Vec<String> = Vec::new();
        let names = vec!["b".to_string(), "c".to_string()];
        assert_eq!(member_text(&names, &none), "+2 (b, c), -0");
        assert_eq!(member_text(&none, &names[..1]), "+0, -1 (b)");
    }

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn a_literal_ends_where_its_first_bracket_closes() {
        let source =
            lines("let a = Cfg {\n    x: f(1, 2),\n    y: \"}\",\n};\nlet b = Cfg { x: 1 };\n");
        assert_eq!(construction_end(&source, 1, "Cfg"), 4);
        assert_eq!(construction_end(&source, 5, "Cfg"), 5);
    }

    #[test]
    fn a_call_across_lines_ends_at_its_closing_parenthesis() {
        let source = lines("total = area(\n    1,\n    2,\n)\nrest()\n");
        assert_eq!(construction_end(&source, 1, "area"), 4);
    }

    #[test]
    fn a_name_with_no_bracket_on_its_line_ends_on_that_line() {
        let source = lines("x = Foo\ny = bar(\n  1)\n");
        assert_eq!(construction_end(&source, 1, "Foo"), 1);
        assert_eq!(construction_end(&source, 9, "Foo"), 9);
    }
}
