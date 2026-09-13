use rusqlite::Connection;
use sha2::Digest;
use std::path::{Path, PathBuf};
use std::process::Command;
use thiserror::Error;
use tracing::info;

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
    #[error("workspace traversal failed: {0}")]
    Walk(#[from] ignore::Error),
}

/// Discovers the location of the `julie-extract` binary.
pub fn find_julie_extract_binary() -> Option<PathBuf> {
    if let Ok(path_str) = std::env::var("JULIE_EXTRACT_BIN") {
        let p = PathBuf::from(path_str);
        if p.exists() {
            return Some(p);
        }
    }

    // Check adjacent release/debug paths in local development
    let exe_name = if cfg!(windows) {
        "julie-extract.exe"
    } else {
        "julie-extract"
    };
    let dev_candidates = [
        PathBuf::from(format!(
            r"c:\source\julie-extractors\target\release\{exe_name}"
        )),
        PathBuf::from(format!(
            r"c:\source\julie-extractors\target\debug\{exe_name}"
        )),
        PathBuf::from(format!("../julie-extractors/target/release/{exe_name}")),
        PathBuf::from(format!("../julie-extractors/target/debug/{exe_name}")),
    ];

    for c in &dev_candidates {
        if c.exists() {
            return Some(crate::workspace::normalize_path(c));
        }
    }

    // Check PATH using platform-agnostic which crate
    if let Ok(p) = which::which("julie-extract") {
        return Some(crate::workspace::normalize_path(&p));
    }

    None
}

/// Run julie-extract command with arguments.
pub fn execute_julie_extract(args: &[&str]) -> Result<String, SyncError> {
    let bin = find_julie_extract_binary().ok_or(SyncError::BinaryNotFound)?;

    let output = Command::new(bin)
        .args(args)
        .output()
        .map_err(SyncError::Io)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let code = output.status.code().unwrap_or(-1);
        return Err(SyncError::ExtractionFailed(code, stderr));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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

/// Cold-start background sweep: checks filesystem against SQLite `files` records.
/// Streams disk checks through a temporary SQLite index.
pub fn reconcile_offline_edits(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
) -> Result<ReconcileReport, SyncError> {
    let mut report = ReconcileReport::default();

    let seen_db = tempfile::NamedTempFile::new()?;
    let temp_conn = Connection::open(seen_db.path()).map_err(SyncError::Db)?;
    temp_conn
        .execute("CREATE TABLE _seen (path TEXT PRIMARY KEY)", [])
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
        .add_custom_ignore_filename(".julieignore");
    let walker = walker.build();

    temp_conn
        .execute("BEGIN TRANSACTION", [])
        .map_err(SyncError::Db)?;

    for result in walker {
        let entry = result?;

        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let path = entry.path();
            if let Ok(rel) = path.strip_prefix(&workspace.canonical_root) {
                let rel_str = crate::workspace::to_forward_slash(rel);
                let bytes = entry.metadata()?.len() as i64;

                insert_seen_stmt
                    .execute([&rel_str])
                    .map_err(SyncError::Db)?;

                let mut rows = check_file_stmt.query([&rel_str]).map_err(SyncError::Db)?;

                if let Some(row) = rows.next().map_err(SyncError::Db)? {
                    let indexed_bytes: i64 = row.get(0).map_err(SyncError::Db)?;
                    let stored_hash: String = row.get(1).map_err(SyncError::Db)?;

                    if indexed_bytes != bytes
                        || !compute_content_hash_matches(&std::fs::read(path)?, &stored_hash)
                    {
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
    drop(seen_db);

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
                update_file(workspace, db_path, added)?;
            }
            for modified in &report.modified {
                update_file(workspace, db_path, modified)?;
            }
            for deleted in &report.deleted {
                delete_file(workspace, db_path, deleted)?;
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
        let bin = find_julie_extract_binary();
        assert!(
            bin.is_some(),
            "Expected julie-extract binary to be discovered via candidates or PATH"
        );
        let path = bin.unwrap();
        assert!(path.exists(), "Discovered path must exist: {:?}", path);
    }
}
