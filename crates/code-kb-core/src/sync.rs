use rusqlite::Connection;
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use tracing::{error, info, warn};

use crate::queries;
use crate::workspace::Workspace;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error(
        "julie-extract not found. Put it on PATH or set JULIE_EXTRACT_BIN. Download: https://github.com/anortham/julie-extractors/releases"
    )]
    BinaryNotFound,
    #[error("Extractor failed with status {0}: {1}")]
    ExtractionFailed(i32, String),
    #[error("IO error during synchronization: {0}")]
    Io(#[from] std::io::Error),
    #[error("Database error during synchronization: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("Database error: {0}")]
    DbInit(#[from] crate::db::DbError),
    #[error("workspace traversal failed: {0}")]
    Walk(#[from] ignore::Error),
}

pub const PINNED_JULIE_VERSION: &str = "3.1.3";

/// Extraction level code-kb asks for on a new artifact: symbol core plus structural facts,
/// without the identifier, literal, and source-region tables code-kb never reads.
pub const EXTRACTION_LEVEL: &str = "facts";

static CACHED_JULIE_BIN: std::sync::OnceLock<Option<(PathBuf, String)>> =
    std::sync::OnceLock::new();

/// Discovers the `julie-extract` binary and caches the result.
///
/// The first candidate whose version matches the pin wins. When none matches, the first
/// candidate found is used and a warning is logged.
pub fn find_julie_extract_binary() -> Option<PathBuf> {
    installed_extractor().map(|(bin, _)| bin)
}

/// Version reported by the `julie-extract` binary in use, or the pin when none was found.
pub fn installed_extractor_version() -> String {
    installed_extractor()
        .map(|(_, version)| version)
        .unwrap_or_else(|| PINNED_JULIE_VERSION.to_string())
}

fn installed_extractor() -> Option<(PathBuf, String)> {
    CACHED_JULIE_BIN
        .get_or_init(|| {
            let candidates: Vec<(PathBuf, String)> = julie_extract_candidates()
                .into_iter()
                .filter_map(|bin| extractor_version(&bin).map(|version| (bin, version)))
                .collect();
            let pinned = candidates
                .iter()
                .find(|(_, version)| version == PINNED_JULIE_VERSION)
                .cloned();
            if pinned.is_some() {
                return pinned;
            }
            let first = candidates.into_iter().next()?;
            tracing::warn!(
                found = %first.1,
                pinned = %PINNED_JULIE_VERSION,
                binary = %first.0.display(),
                "julie-extract version differs from pinned version; AST facts may drift"
            );
            Some(first)
        })
        .clone()
}

fn extractor_version(bin: &Path) -> Option<String> {
    let output = Command::new(bin).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_whitespace().last().map(str::to_string)
}

fn julie_extract_candidates() -> Vec<PathBuf> {
    let exe_name = if cfg!(windows) {
        "julie-extract.exe"
    } else {
        "julie-extract"
    };
    let mut candidates = Vec::new();

    if let Ok(path_str) = std::env::var("JULIE_EXTRACT_BIN") {
        candidates.push(PathBuf::from(path_str));
    }

    if let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        candidates.push(parent.join(exe_name));
        candidates.push(parent.join(".tools").join(exe_name));
    }

    if let Ok(cwd) = std::env::current_dir() {
        let mut probe = cwd;
        loop {
            candidates.push(probe.join(".tools").join(exe_name));
            match probe.parent() {
                Some(parent) if parent != probe => probe = parent.to_path_buf(),
                _ => break,
            }
        }
    }

    if let Ok(p) = which::which("julie-extract") {
        candidates.push(p);
    }

    candidates
        .into_iter()
        .filter(|p| p.is_file())
        .map(|p| crate::workspace::normalize_path(&p))
        .collect()
}

/// Run julie-extract command with arguments.
pub fn execute_julie_extract(args: &[&str]) -> Result<String, SyncError> {
    let bin = find_julie_extract_binary().ok_or(SyncError::BinaryNotFound)?;

    let mut attempts = 0;
    loop {
        let output = Command::new(&bin)
            .args(args)
            .output()
            .map_err(SyncError::Io)?;

        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }

        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        // Retry on database lock / busy collisions with backoff
        if (stderr.contains("database is locked")
            || stderr.contains("busy")
            || stderr.contains("SQLITE_BUSY"))
            && attempts < 5
        {
            attempts += 1;
            std::thread::sleep(std::time::Duration::from_millis(50 * (1 << attempts)));
            continue;
        }

        let code = output.status.code().unwrap_or(-1);
        return Err(SyncError::ExtractionFailed(code, stderr));
    }
}

/// Tier 1 & Incremental Update: updates a single file in the database.
pub fn update_file(workspace: &Workspace, db_path: &Path, rel_path: &str) -> Result<(), SyncError> {
    let root_str = workspace.canonical_root.to_string_lossy();
    let db_str = db_path.to_string_lossy();

    execute_julie_extract(&[
        "update", "--root", &root_str, "--db", &db_str, "--file", rel_path,
    ])?;

    Ok(())
}

/// Delete a file's extraction records from the database.
pub fn delete_file(workspace: &Workspace, db_path: &Path, rel_path: &str) -> Result<(), SyncError> {
    let root_str = workspace.canonical_root.to_string_lossy();
    let db_str = db_path.to_string_lossy();

    execute_julie_extract(&[
        "delete", "--root", &root_str, "--db", &db_str, "--file", rel_path,
    ])?;

    Ok(())
}

/// Initial or full scan to build/refresh the database.
///
/// An artifact the extractor cannot read (empty, torn, or older schema) is removed and
/// rebuilt from scratch.
pub fn scan_workspace(workspace: &Workspace, db_path: &Path, force: bool) -> Result<(), SyncError> {
    let root_str = workspace.canonical_root.to_string_lossy();
    let db_str = db_path.to_string_lossy();
    let own_pid = std::process::id().to_string();

    ensure_index_dir(db_path)?;

    let scan_args = |new_artifact: bool| {
        let mut args = vec!["scan", "--root", &*root_str, "--db", &*db_str];
        if new_artifact {
            args.extend(["--level", EXTRACTION_LEVEL]);
        }
        if cfg!(unix) {
            args.extend(["--parent-pid", &*own_pid]);
        }
        if force {
            args.push("--force");
        }
        args
    };

    match execute_julie_extract(&scan_args(!db_path.exists())) {
        Ok(_) => {}
        Err(SyncError::ExtractionFailed(_, stderr))
            if stderr.contains("schema_incompatible") && db_path.exists() =>
        {
            warn!("Extractor cannot read the existing artifact; rebuilding from scratch");
            remove_artifact_files(db_path)?;
            execute_julie_extract(&scan_args(true))?;
        }
        Err(e) => return Err(e),
    }

    crate::db::ensure_fts_index_path(db_path).map_err(|e| {
        error!(db = %db_path.display(), "FTS index preparation failed: {e}");
        e
    })?;

    Ok(())
}

/// Rebuilds the index when it was written by a `julie-extract` other than the one in use or
/// at another extraction level. Returns `true` when a rebuild ran. The old artifact is removed first because the
/// extractor refuses to write into an artifact with an older schema.
pub fn ensure_index_matches_extractor(
    workspace: &Workspace,
    db_path: &Path,
    extractor_version: &str,
) -> Result<bool, SyncError> {
    if !db_path.exists() {
        return Ok(false);
    }
    let metadata = |key: &str| -> Option<String> {
        let conn = crate::db::open_read_only(db_path).ok()?;
        conn.query_row(
            "SELECT value FROM artifact_metadata WHERE key = ?1",
            [key],
            |r| r.get(0),
        )
        .ok()
    };
    let Some(recorded) = metadata("binary_version") else {
        return Ok(false);
    };
    let level = metadata("index_level").unwrap_or_else(|| "full".to_string());
    if recorded == extractor_version
        && level == EXTRACTION_LEVEL
        && !has_file_written_by_another_extractor(db_path, extractor_version)
    {
        return Ok(false);
    }
    info!(
        recorded = %recorded,
        installed = %extractor_version,
        level = %level,
        wanted_level = %EXTRACTION_LEVEL,
        "Index holds rows from a different julie-extract version or level; rebuilding"
    );
    remove_artifact_files(db_path)?;
    scan_workspace(workspace, db_path, true)?;
    Ok(true)
}

/// A `julie-extract update` run by a newer binary stamps its version into `artifact_metadata`
/// but leaves every unchanged file's rows as the older binary wrote them, so the guard also
/// checks the revision that last wrote each file.
fn has_file_written_by_another_extractor(db_path: &Path, extractor_version: &str) -> bool {
    let Ok(conn) = crate::db::open_read_only(db_path) else {
        return false;
    };
    conn.query_row(
        "SELECT 1 FROM files f
         JOIN extraction_revisions r ON r.revision_id = f.last_revision_id
         WHERE r.binary_version != ?1
         LIMIT 1",
        [extractor_version],
        |_| Ok(true),
    )
    .unwrap_or(false)
}

fn remove_artifact_files(db_path: &Path) -> Result<(), SyncError> {
    for suffix in ["", "-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", db_path.display()));
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Check if disk content matches the stored hash in the database.
pub fn compute_content_hash_matches(disk_bytes: &[u8], stored_hash: &str) -> bool {
    if stored_hash.starts_with("blake3:") {
        let b3 = format!("blake3:{}", blake3::hash(disk_bytes).to_hex());
        b3 == stored_hash
    } else {
        let b3 = blake3::hash(disk_bytes).to_hex().to_string();
        if b3 == stored_hash {
            return true;
        }
        let mut hasher = sha2::Sha256::new();
        sha2::Digest::update(&mut hasher, disk_bytes);
        let sha = hex::encode(sha2::Digest::finalize(hasher));
        sha == stored_hash || format!("sha256:{sha}") == stored_hash
    }
}

/// Tier 2: Just-In-Time Staleness Guard
/// Checks if a file on disk has been modified since it was indexed in SQLite.
/// If dirty or missing from index, re-extracts the file synchronously (< 5ms).
pub fn ensure_fresh_file(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    rel_path: &str,
) -> Result<bool, SyncError> {
    let abs_path = workspace.canonical_root.join(rel_path);
    let meta = match std::fs::metadata(&abs_path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let existing_file = queries::get_file(conn, rel_path).map_err(|e| match e {
                queries::QueryError::Sqlite(err) => SyncError::Db(err),
                _ => SyncError::Db(rusqlite::Error::QueryReturnedNoRows),
            })?;
            if existing_file.is_some() {
                delete_file(workspace, db_path, rel_path)?;
                return Ok(true);
            }
            return Ok(false);
        }
        Err(error) => return Err(SyncError::Io(error)),
    };

    if meta.is_dir() {
        return Ok(false);
    }

    let disk_bytes = meta.len() as i64;

    // Look up file in SQLite files table
    let existing_file = queries::get_file(conn, rel_path).map_err(|e| match e {
        queries::QueryError::Sqlite(err) => SyncError::Db(err),
        _ => SyncError::Db(rusqlite::Error::QueryReturnedNoRows),
    })?;

    let is_dirty = match existing_file {
        None => true, // Not yet in index
        Some(f) => {
            // Fast check: byte count difference
            if f.content_bytes != disk_bytes {
                true
            } else {
                let disk_content = std::fs::read(&abs_path)?;
                !compute_content_hash_matches(&disk_content, &f.content_hash)
            }
        }
    };

    if is_dirty {
        let stored_root: Option<String> = conn
            .query_row(
                "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
                [],
                |r| r.get(0),
            )
            .ok();
        if let Some(r) = stored_root
            && !crate::workspace::paths_equal(Path::new(&r), &workspace.canonical_root)
        {
            crate::db::retarget_artifact_root(db_path, &workspace.canonical_root)?;
        }

        update_file(workspace, db_path, rel_path)?;
        return Ok(true);
    }

    Ok(false)
}

/// Cold-Start Reconciliation Report
#[derive(Debug, Default)]
pub struct ReconcileReport {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
}

fn extract_error_path(err: &ignore::Error) -> Option<&Path> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path.as_path()),
        ignore::Error::WithDepth { err, .. } => extract_error_path(err),
        ignore::Error::WithLineNumber { err, .. } => extract_error_path(err),
        ignore::Error::Loop { ancestor, .. } => Some(ancestor.as_path()),
        _ => None,
    }
}

/// Cold-start background sweep: checks filesystem against SQLite `files` records.
/// Streams disk checks and uses an in-memory SQLite index to eliminate repository-wide heap HashMaps.
/// Creates a missing index. A git worktree copies its parent repository's index and
/// reconciles it against the worktree files, which is much faster than a full scan;
/// anything else runs a full scan.
pub fn create_index(workspace: &Workspace, db_path: &Path) -> Result<(), SyncError> {
    if let Some(parent_db) = parent_repository_db(&workspace.canonical_root)
        && copy_parent_index(workspace, db_path, &parent_db)
    {
        return Ok(());
    }
    scan_workspace(workspace, db_path, false)
}

/// Creates the index directory. A `.code-kb` directory also gets a `.gitignore` that
/// hides it from git, so no project `.gitignore` edit is ever needed. Any other
/// directory is left alone: a `*` ignore file there would hide the user's own files
/// from the extractor and from git.
fn ensure_index_dir(db_path: &Path) -> std::io::Result<()> {
    let Some(dir) = db_path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    if dir.file_name().is_none_or(|name| name != ".code-kb") {
        return Ok(());
    }
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        std::fs::write(gitignore, "*\n")?;
    }
    Ok(())
}

fn parent_repository_db(root: &Path) -> Option<PathBuf> {
    let git_marker = root.join(".git");
    if !git_marker.is_file() {
        return None;
    }
    let git_content = std::fs::read_to_string(&git_marker).ok()?;
    let gitdir = git_content
        .lines()
        .find_map(|l| l.strip_prefix("gitdir:"))?
        .trim();
    let gitdir = Path::new(gitdir);
    let mut probe = if gitdir.is_absolute() {
        gitdir.to_path_buf()
    } else {
        root.join(gitdir)
    };
    while let Some(parent) = probe.parent() {
        if parent == probe {
            break;
        }
        if parent.join(".git").exists() {
            let db = parent.join(".code-kb").join("artifact.db");
            return db.exists().then_some(db);
        }
        probe = parent.to_path_buf();
    }
    None
}

fn copy_parent_index(workspace: &Workspace, db_path: &Path, parent_db: &Path) -> bool {
    let flushed = crate::db::open_read_write(parent_db)
        .map(|conn| crate::db::checkpoint_truncate(&conn).is_ok())
        .unwrap_or(false);
    if !flushed {
        return false;
    }
    if ensure_index_dir(db_path).is_err() || std::fs::copy(parent_db, db_path).is_err() {
        return false;
    }
    info!(from = %parent_db.display(), to = %db_path.display(), "Worktree fast-path: copied parent database, reconciling");
    let reconciled = crate::db::retarget_artifact_root(db_path, &workspace.canonical_root)
        .and_then(|_| crate::db::ensure_fts_index_path(db_path))
        .is_ok()
        && crate::db::open_read_only(db_path)
            .ok()
            .and_then(|conn| reconcile_offline_edits(workspace, db_path, &conn).ok())
            .is_some();
    if !reconciled {
        warn!(
            "Failed to retarget worktree database root; removing copied db and falling back to full scan"
        );
        let _ = std::fs::remove_file(db_path);
    }
    reconciled
}

pub fn reconcile_offline_edits(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
) -> Result<ReconcileReport, SyncError> {
    // Ensure artifact_metadata root_path matches workspace canonical root
    // (self-heals worktrees where artifact.db was copied from a parent repository)
    let stored_root: Option<String> = conn
        .query_row(
            "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
            [],
            |r| r.get(0),
        )
        .ok();
    if let Some(r) = stored_root
        && !crate::workspace::paths_equal(Path::new(&r), &workspace.canonical_root)
    {
        crate::db::retarget_artifact_root(db_path, &workspace.canonical_root)?;
    }

    let mut report = ReconcileReport::default();

    // In-memory table to track paths seen on disk without allocating repository-wide HashMaps in heap
    let temp_conn = Connection::open_in_memory().map_err(SyncError::Db)?;
    temp_conn
        .execute(
            "CREATE TABLE _seen (path TEXT COLLATE NOCASE PRIMARY KEY)",
            [],
        )
        .map_err(SyncError::Db)?;

    let mut insert_seen_stmt = temp_conn
        .prepare("INSERT OR IGNORE INTO _seen (path) VALUES (?1)")
        .map_err(SyncError::Db)?;

    let mut check_file_stmt = conn
        .prepare("SELECT content_bytes, content_hash FROM files WHERE path = ?1")
        .map_err(SyncError::Db)?;

    let mut walker = ignore::WalkBuilder::new(&workspace.canonical_root);
    walker
        .standard_filters(true)
        .hidden(false)
        .add_custom_ignore_filename(".julieignore")
        .add_custom_ignore_filename(".code-kb-ignore")
        .add_custom_ignore_filename(".codekbignore")
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !crate::workspace::is_hard_excluded(&name)
        });
    let walker = walker.build();

    temp_conn
        .execute("BEGIN TRANSACTION", [])
        .map_err(SyncError::Db)?;

    let mut unreadable_prefixes: Vec<String> = Vec::new();

    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(e) => {
                warn!("Reconciliation walker encountered error: {e}");
                if let Some(path) = extract_error_path(&e) {
                    let norm_path = dunce::simplified(path);
                    if let Some(rel) =
                        crate::workspace::strip_prefix_lossy(norm_path, &workspace.canonical_root)
                    {
                        unreadable_prefixes.push(crate::workspace::to_forward_slash(rel));
                    }
                }
                continue;
            }
        };

        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let path = entry.path();
            let norm_path = dunce::simplified(path);
            if let Some(rel) =
                crate::workspace::strip_prefix_lossy(norm_path, &workspace.canonical_root)
            {
                let rel_str = crate::workspace::to_forward_slash(rel);
                if crate::workspace::is_hard_excluded(&rel_str) {
                    continue;
                }
                let bytes = match entry.metadata() {
                    Ok(m) => m.len() as i64,
                    Err(e) => {
                        warn!("Failed to read metadata for '{}': {e}", path.display());
                        let _ = insert_seen_stmt.execute([&rel_str]);
                        continue;
                    }
                };

                insert_seen_stmt
                    .execute([&rel_str])
                    .map_err(SyncError::Db)?;

                let mut rows = check_file_stmt.query([&rel_str]).map_err(SyncError::Db)?;

                if let Some(row) = rows.next().map_err(SyncError::Db)? {
                    let indexed_bytes: i64 = row.get(0).map_err(SyncError::Db)?;
                    let stored_hash: String = row.get(1).map_err(SyncError::Db)?;

                    let is_modified = if indexed_bytes != bytes {
                        true
                    } else {
                        match std::fs::read(path) {
                            Ok(content) => !compute_content_hash_matches(&content, &stored_hash),
                            Err(e) => {
                                warn!(
                                    "Failed to read '{}' for hash verification: {e}",
                                    path.display()
                                );
                                false
                            }
                        }
                    };

                    if is_modified {
                        report.modified.push(rel_str);
                    }
                } else {
                    report.added.push(rel_str);
                }
            }
        }
    }

    temp_conn.execute("COMMIT", []).map_err(SyncError::Db)?;

    let mut files_stmt = conn
        .prepare("SELECT path FROM files")
        .map_err(SyncError::Db)?;

    let mut exists_seen_stmt = temp_conn
        .prepare("SELECT 1 FROM _seen WHERE path = ?1")
        .map_err(SyncError::Db)?;

    let mut file_rows = files_stmt.query([]).map_err(SyncError::Db)?;
    while let Some(row) = file_rows.next().map_err(SyncError::Db)? {
        let indexed_path: String = row.get(0).map_err(SyncError::Db)?;

        // If the indexed file belongs to an unreadable directory, preserve it
        let in_unreadable_prefix = unreadable_prefixes.iter().any(|prefix| {
            indexed_path == *prefix || indexed_path.starts_with(&format!("{prefix}/"))
        });
        if in_unreadable_prefix {
            continue;
        }

        let mut seen_rows = exists_seen_stmt
            .query([&indexed_path])
            .map_err(SyncError::Db)?;
        if seen_rows.next().map_err(SyncError::Db)?.is_none() {
            report.deleted.push(indexed_path);
        }
    }

    drop(file_rows);
    drop(files_stmt);
    drop(check_file_stmt);
    drop(exists_seen_stmt);
    drop(insert_seen_stmt);
    drop(temp_conn);

    let total_changes = report.added.len() + report.modified.len() + report.deleted.len();
    if total_changes > 0 {
        info!(
            "Cold start reconciliation detected {} changes (+{}, ~{}, -{})",
            total_changes,
            report.added.len(),
            report.modified.len(),
            report.deleted.len()
        );

        if total_changes > 50 {
            // Trigger bulk scan if large changes
            scan_workspace(workspace, db_path, false)?;
        } else {
            // Incremental single-file updates
            for added in &report.added {
                if let Err(e) = update_file(workspace, db_path, added) {
                    warn!("Failed to index added file '{}': {e}", added);
                }
            }
            for modified in &report.modified {
                if let Err(e) = update_file(workspace, db_path, modified) {
                    warn!("Failed to index modified file '{}': {e}", modified);
                }
            }
            for deleted in &report.deleted {
                if let Err(e) = delete_file(workspace, db_path, deleted) {
                    warn!("Failed to remove deleted file '{}': {e}", deleted);
                }
            }
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    #[test]
    fn pinned_version_matches_the_pins_file() {
        let pins = include_str!("../../../scripts/julie-pins.json");
        assert!(pins.contains(&format!("\"version\": \"{}\"", super::PINNED_JULIE_VERSION)));
    }

    use super::*;

    #[test]
    fn test_find_julie_extract_binary() {
        let path = find_julie_extract_binary()
            .expect("julie-extract binary must be present for tests (see scripts/julie-pins.json)");
        assert!(path.exists(), "Discovered path must exist: {:?}", path);
    }

    #[test]
    fn ensure_index_dir_writes_self_ignoring_gitignore() {
        let temp = crate::safe_tempdir();
        let db_path = temp.path().join(".code-kb").join("artifact.db");
        ensure_index_dir(&db_path).unwrap();
        let gitignore = db_path.parent().unwrap().join(".gitignore");
        assert_eq!(std::fs::read_to_string(&gitignore).unwrap(), "*\n");
        std::fs::write(&gitignore, "custom\n").unwrap();
        ensure_index_dir(&db_path).unwrap();
        assert_eq!(std::fs::read_to_string(&gitignore).unwrap(), "custom\n");
    }

    #[test]
    fn ensure_index_dir_leaves_non_code_kb_directories_alone() {
        let temp = crate::safe_tempdir();
        let db_path = temp.path().join("test.db");
        ensure_index_dir(&db_path).unwrap();
        assert!(!temp.path().join(".gitignore").exists());
    }

    #[test]
    fn test_reconcile_offline_edits_drive_case_mismatch() {
        let temp = crate::safe_tempdir();
        let db_path = temp.path().join("test.db");
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

        // Create a real file on disk
        let src_dir = temp.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let file_path = src_dir.join("main.rs");
        let content = "fn main() {}\n";
        std::fs::write(&file_path, content).unwrap();

        let hash = sha2::Sha256::digest(content.as_bytes());
        let hash_hex = hex::encode(hash);

        conn.execute(
            "INSERT INTO files VALUES ('f1', 'src/main.rs', 'rust', ?1, ?2, 1, '2026-09-14T00:00:00Z')",
            rusqlite::params![hash_hex, content.len() as i64],
        )
        .unwrap();

        #[allow(unused_mut)]
        let mut ws = Workspace::new(temp.path().to_path_buf());
        #[cfg(windows)]
        {
            let root_str = ws.canonical_root.to_string_lossy().to_string();
            if let Some(first_char) = root_str.chars().next() {
                let flipped = if first_char.is_ascii_uppercase() {
                    first_char.to_ascii_lowercase()
                } else {
                    first_char.to_ascii_uppercase()
                };
                let altered_root = format!("{}{}", flipped, &root_str[1..]);
                ws.canonical_root = PathBuf::from(altered_root);
            }
        }

        let report = reconcile_offline_edits(&ws, &db_path, &conn).unwrap();
        assert!(
            report.deleted.is_empty(),
            "Files should not be marked deleted due to drive casing difference: {:?}",
            report.deleted
        );
    }
}
