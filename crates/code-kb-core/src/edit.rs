use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::Path;
use thiserror::Error;

use crate::queries;
use crate::slicer;
use crate::sync;
use crate::syntax::{self, SyntaxError};
use crate::workspace::Workspace;

#[derive(Debug, Error)]
pub enum EditError {
    #[error("Symbol '{0}' not found in '{1}'")]
    SymbolNotFound(String, String),
    #[error("Symbol has no body defined (e.g. trait declaration without default implementation)")]
    NoBodyDefined,
    #[error("Optimistic lock failed: expected body hash '{0}', found '{1}'")]
    HashMismatch(String, String),
    #[error(
        "File offsets out of bounds: range [{0}..{1}], but file length is {2} bytes (file may have shrunk or changed)"
    )]
    InvalidOffsetRange(usize, usize, usize),
    #[error("Pre-flight syntax validation failed: {0}")]
    Syntax(#[from] SyntaxError),
    #[error("Replacement content is not valid UTF-8: {0}")]
    InvalidUtf8(String),
    #[error("Failed to read/write file '{0}': {1}")]
    Io(String, #[source] std::io::Error),
    #[error("Synchronization failed and file was rolled back: {0}")]
    SyncWithRollback(String),
    #[error(
        "Synchronization failed after edit ({sync_error}) and rollback also failed: {rollback_error}"
    )]
    SyncRollbackFailed {
        sync_error: String,
        rollback_error: String,
    },
    #[error("File '{0}' was concurrently modified; edit aborted")]
    ConcurrentModification(String),
    #[error("Synchronization failed after edit: {0}")]
    Sync(#[from] sync::SyncError),
    #[error("Query error: {0}")]
    Query(#[from] queries::QueryError),
}

/// Result of an atomic symbol body replacement.
#[derive(Debug, Clone)]
pub struct EditResult {
    pub symbol_name: String,
    pub file_path: String,
    pub old_body_hash: String,
    pub new_body_hash: String,
    pub bytes_written: usize,
}

/// Computes SHA256 of a string content.
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Atomically replaces the implementation body of a symbol by name.
pub fn replace_symbol_body(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    symbol_name: &str,
    file_path: &str,
    new_body: &str,
    expected_body_hash: Option<&str>,
) -> Result<EditResult, EditError> {
    let (abs_path, rel_path) = workspace
        .resolve_path(Path::new(file_path))
        .map_err(|e| EditError::SymbolNotFound(symbol_name.to_string(), e.to_string()))?;

    // Tier 2: Refresh file in index before querying symbol offsets, propagating any sync errors
    sync::ensure_fresh_file(workspace, db_path, conn, &rel_path)?;

    // Find symbol in database with exact path
    let symbol = queries::get_symbol_by_name_exact(conn, symbol_name, &rel_path)?
        .ok_or_else(|| EditError::SymbolNotFound(symbol_name.to_string(), rel_path.clone()))?;

    let body_start = symbol.body_start_byte.ok_or(EditError::NoBodyDefined)?;
    let body_end = symbol.body_end_byte.ok_or(EditError::NoBodyDefined)?;

    // Read existing file metadata (permissions) and content
    let existing_metadata =
        fs::metadata(&abs_path).map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;
    let existing_permissions = existing_metadata.permissions();
    let existing_bytes =
        fs::read(&abs_path).map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;

    // Bounds check to prevent out-of-bounds panics
    if body_start > body_end || body_end > existing_bytes.len() {
        return Err(EditError::InvalidOffsetRange(
            body_start,
            body_end,
            existing_bytes.len(),
        ));
    }

    let existing_body =
        slicer::slice_bytes_safe(&existing_bytes, body_start, body_end).map_err(|e| {
            EditError::Io(
                abs_path.display().to_string(),
                std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()),
            )
        })?;

    let current_sha256 = hash_content(existing_body);
    let current_blake3 = blake3::hash(existing_body.as_bytes()).to_hex().to_string();

    // Verify optimistic lock if caller specified expected_body_hash
    if let Some(expected) = expected_body_hash {
        let mut matches = expected == current_sha256 || expected == current_blake3;

        // If expected matches the indexed body_hash from julie-extract, verify that the
        // file on disk has NOT been modified since the index was created.
        if !matches && symbol.body_hash.as_deref().is_some_and(|h| h == expected) {
            let disk_body_hash = hash_content(existing_body);
            if disk_body_hash == current_sha256 {
                matches = true;
            }
        }

        if !matches {
            return Err(EditError::HashMismatch(
                expected.to_string(),
                current_sha256,
            ));
        }
    }

    // Match file line endings (CRLF vs LF)
    let is_crlf = existing_bytes.windows(2).any(|w| w == b"\r\n");
    let normalized_body = if is_crlf && !new_body.contains("\r\n") && new_body.contains('\n') {
        new_body.replace('\n', "\r\n")
    } else if !is_crlf && new_body.contains("\r\n") {
        new_body.replace("\r\n", "\n")
    } else {
        new_body.to_string()
    };

    // Construct new file content with replaced byte span
    let mut new_file_bytes = Vec::with_capacity(existing_bytes.len() + normalized_body.len());
    new_file_bytes.extend_from_slice(&existing_bytes[..body_start]);
    new_file_bytes.extend_from_slice(normalized_body.as_bytes());
    new_file_bytes.extend_from_slice(&existing_bytes[body_end..]);

    // Pre-flight syntax validation before touching disk
    let new_file_str =
        std::str::from_utf8(&new_file_bytes).map_err(|e| EditError::InvalidUtf8(e.to_string()))?;
    syntax::validate_syntax(&rel_path, new_file_str)?;

    // Backup original bytes for rollback if re-indexing fails
    let backup_bytes = existing_bytes.clone();

    // Final pre-commit disk check: ensure file was not concurrently modified between read and write
    let current_disk =
        fs::read(&abs_path).map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;
    if current_disk != existing_bytes {
        return Err(EditError::ConcurrentModification(rel_path));
    }

    // Write file atomically: write to a temporary file in the target directory, then persist
    let target_dir = abs_path.parent().unwrap_or(Path::new("."));
    let mut temp_file = tempfile::Builder::new()
        .prefix(".code-kb-edit-")
        .suffix(".tmp")
        .tempfile_in(target_dir)
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;

    temp_file
        .write_all(&new_file_bytes)
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;
    temp_file
        .flush()
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;

    // Preserve existing file permissions (e.g. +x 0755)
    let _ = temp_file
        .as_file()
        .set_permissions(existing_permissions.clone());

    temp_file
        .persist(&abs_path)
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e.error))?;

    // Tier 1: Immediately re-index the file so catalog is 100% fresh.
    // If indexing fails, roll back to original content safely and atomically.
    if let Err(err) = sync::update_file(workspace, db_path, &rel_path) {
        // First check if the file on disk is still our newly written file
        let disk_post_write = fs::read(&abs_path);
        if disk_post_write.as_deref().ok() != Some(new_file_bytes.as_slice()) {
            return Err(EditError::ConcurrentModification(format!(
                "File was concurrently modified during re-indexing; rollback aborted: {err}"
            )));
        }

        // Perform rollback atomically via temporary file
        let rollback_res = (|| -> Result<(), std::io::Error> {
            let mut rollback_tmp = tempfile::Builder::new()
                .prefix(".code-kb-rollback-")
                .suffix(".tmp")
                .tempfile_in(target_dir)?;
            rollback_tmp.write_all(&backup_bytes)?;
            rollback_tmp.flush()?;
            let _ = rollback_tmp
                .as_file()
                .set_permissions(existing_permissions.clone());
            rollback_tmp.persist(&abs_path).map_err(|e| e.error)?;
            Ok(())
        })();

        match rollback_res {
            Ok(()) => return Err(EditError::SyncWithRollback(err.to_string())),
            Err(rollback_err) => {
                return Err(EditError::SyncRollbackFailed {
                    sync_error: err.to_string(),
                    rollback_error: rollback_err.to_string(),
                });
            }
        }
    }

    let new_body_hash = hash_content(&normalized_body);

    Ok(EditResult {
        symbol_name: symbol_name.to_string(),
        file_path: rel_path,
        old_body_hash: current_sha256,
        new_body_hash,
        bytes_written: new_file_bytes.len(),
    })
}
