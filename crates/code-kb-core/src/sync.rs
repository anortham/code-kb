use rusqlite::Connection;
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use tracing::{info, warn};

use crate::queries;
use crate::workspace::Workspace;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("Extractor binary not found. Set JULIE_EXTRACT_BIN or ensure julie-extract is in PATH")]
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

pub const PINNED_JULIE_VERSION: &str = "2.42.3";

static CACHED_JULIE_BIN: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();

/// Discovers the location of the `julie-extract` binary and caches the result.
pub fn find_julie_extract_binary() -> Option<PathBuf> {
    CACHED_JULIE_BIN
        .get_or_init(|| {
            let bin = discover_julie_extract_binary()?;

            // Validate version against pinned extractor release
            if let Ok(output) = Command::new(&bin).arg("--version").output() {
                let ver_str = String::from_utf8_lossy(&output.stdout);
                if !ver_str.contains(PINNED_JULIE_VERSION) {
                    tracing::warn!(
                        found = %ver_str.trim(),
                        pinned = %PINNED_JULIE_VERSION,
                        binary = %bin.display(),
                        "julie-extract version differs from pinned version; AST facts may drift"
                    );
                }
            }

            Some(bin)
        })
        .clone()
}

fn discover_julie_extract_binary() -> Option<PathBuf> {
    // 1. Check JULIE_EXTRACT_BIN env var
    if let Ok(path_str) = std::env::var("JULIE_EXTRACT_BIN") {
        let p = PathBuf::from(path_str);
        if p.exists() {
            return Some(crate::workspace::normalize_path(&p));
        }
    }

    let exe_name = if cfg!(windows) {
        "julie-extract.exe"
    } else {
        "julie-extract"
    };

    // 2. Check next to current running executable (bundled release distribution)
    if let Some(parent) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        let sibling = parent.join(exe_name);
        if sibling.exists() {
            return Some(crate::workspace::normalize_path(&sibling));
        }
        let tools_sibling = parent.join(".tools").join(exe_name);
        if tools_sibling.exists() {
            return Some(crate::workspace::normalize_path(&tools_sibling));
        }
    }

    // 3. Check upward traversal for .tools/julie-extract from CWD
    if let Ok(cwd) = std::env::current_dir() {
        let mut probe = cwd;
        loop {
            let candidate = probe.join(".tools").join(exe_name);
            if candidate.exists() {
                return Some(crate::workspace::normalize_path(&candidate));
            }
            if let Some(parent) = probe.parent() {
                if parent == probe {
                    break;
                }
                probe = parent.to_path_buf();
            } else {
                break;
            }
        }
    }

    // 4. Check PATH using platform-agnostic which crate
    if let Ok(p) = which::which("julie-extract") {
        return Some(crate::workspace::normalize_path(&p));
    }

    None
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
pub fn scan_workspace(workspace: &Workspace, db_path: &Path, force: bool) -> Result<(), SyncError> {
    let root_str = workspace.canonical_root.to_string_lossy();
    let db_str = db_path.to_string_lossy();

    // Ensure parent dir exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut args = vec!["scan", "--root", &root_str, "--db", &db_str];

    if force {
        args.push("--force");
    }

    execute_julie_extract(&args)?;

    // Ensure FTS5 index is built and triggers are established
    let _ = crate::db::ensure_fts_index_path(db_path);

    Ok(())
}

/// Rebuilds the index when it was written by a `julie-extract` other than the pinned one.
/// Returns `true` when a rebuild ran. The old artifact is removed first because the
/// extractor refuses to write into an artifact with an older schema.
pub fn ensure_index_matches_pin(workspace: &Workspace, db_path: &Path) -> Result<bool, SyncError> {
    if !db_path.exists() {
        return Ok(false);
    }
    let recorded: Option<String> = {
        let conn = crate::db::open_read_only(db_path)?;
        conn.query_row(
            "SELECT value FROM artifact_metadata WHERE key = 'binary_version'",
            [],
            |r| r.get(0),
        )
        .ok()
    };
    let Some(recorded) = recorded else {
        return Ok(false);
    };
    if recorded == PINNED_JULIE_VERSION {
        return Ok(false);
    }
    info!(
        recorded = %recorded,
        pinned = %PINNED_JULIE_VERSION,
        "Index was written by a different julie-extract version; rebuilding"
    );
    for suffix in ["", "-wal", "-shm"] {
        let sidecar = PathBuf::from(format!("{}{suffix}", db_path.display()));
        match std::fs::remove_file(&sidecar) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    scan_workspace(workspace, db_path, true)?;
    Ok(true)
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
    use super::*;

    #[test]
    fn test_find_julie_extract_binary() {
        let path = find_julie_extract_binary()
            .expect("julie-extract binary must be present for tests (see scripts/julie-pins.json)");
        assert!(path.exists(), "Discovered path must exist: {:?}", path);
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
