use rusqlite::Connection;
use serde::{Deserialize, Serialize};
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
    #[error("Workspace path error: {0}")]
    Workspace(#[from] crate::workspace::WorkspaceError),
    #[error("Query error: {0}")]
    Query(#[from] queries::QueryError),
}

/// Result of an atomic symbol body replacement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditResult {
    pub symbol_name: String,
    pub file_path: String,
    pub old_body_hash: String,
    pub new_body_hash: String,
    pub bytes_written: usize,
    pub syntax_checked: bool,
}

/// Computes SHA256 of a string content.
pub fn hash_content(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

fn is_transient_lock_error(err: &std::io::Error) -> bool {
    #[cfg(windows)]
    {
        if let Some(code) = err.raw_os_error() {
            // ERROR_ACCESS_DENIED = 5, ERROR_SHARING_VIOLATION = 32, ERROR_LOCK_VIOLATION = 33
            if code == 5 || code == 32 || code == 33 {
                return true;
            }
        }
    }
    matches!(err.kind(), std::io::ErrorKind::PermissionDenied)
}

fn persist_with_retry(
    mut temp_file: tempfile::NamedTempFile,
    dest: &Path,
    expected_dest_bytes: Option<&[u8]>,
) -> Result<(), std::io::Error> {
    for attempt in 0..5 {
        if attempt > 0
            && let Some(expected) = expected_dest_bytes
            && let Ok(current) = fs::read(dest)
            && current != expected
        {
            return Err(std::io::Error::other(
                "Destination file was concurrently modified during retry",
            ));
        }
        match temp_file.persist(dest) {
            Ok(_) => return Ok(()),
            Err(e) => {
                let is_transient = is_transient_lock_error(&e.error);
                temp_file = e.file;
                if is_transient && attempt < 4 {
                    std::thread::sleep(std::time::Duration::from_millis(10 * (1 << attempt)));
                    continue;
                }
                return Err(e.error);
            }
        }
    }
    unreachable!()
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
    let (abs_path, rel_path) = workspace.resolve_path(Path::new(file_path))?;

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

    // Verify optimistic lock if caller specified expected_body_hash
    if let Some(expected) = expected_body_hash
        && expected != current_sha256
    {
        return Err(EditError::HashMismatch(
            expected.to_string(),
            current_sha256,
        ));
    }

    // Match file line endings (CRLF vs LF)
    let is_crlf = existing_bytes.windows(2).any(|w| w == b"\r\n");
    let normalized_body = if is_crlf {
        // Uniformly normalize all line endings to CRLF, including mixed inputs
        let lf_only = new_body.replace("\r\n", "\n");
        lf_only.replace('\n', "\r\n")
    } else {
        // Uniformly normalize all line endings to LF, including mixed inputs
        new_body.replace("\r\n", "\n")
    };

    // Construct new file content with replaced byte span
    let mut new_file_bytes = Vec::with_capacity(existing_bytes.len() + normalized_body.len());
    new_file_bytes.extend_from_slice(&existing_bytes[..body_start]);
    new_file_bytes.extend_from_slice(normalized_body.as_bytes());
    new_file_bytes.extend_from_slice(&existing_bytes[body_end..]);

    // Pre-flight syntax validation before touching disk
    let new_file_str =
        std::str::from_utf8(&new_file_bytes).map_err(|e| EditError::InvalidUtf8(e.to_string()))?;
    let syntax_checked = syntax::validate_syntax(&rel_path, new_file_str)?;

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

    persist_with_retry(temp_file, &abs_path, Some(&existing_bytes)).map_err(|e| {
        if e.to_string().contains("concurrently modified") {
            EditError::ConcurrentModification(rel_path.clone())
        } else {
            EditError::Io(abs_path.display().to_string(), e)
        }
    })?;

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
            persist_with_retry(rollback_tmp, &abs_path, Some(&new_file_bytes))?;
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
        syntax_checked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mixed_crlf_lf_normalization() {
        let mixed_input = "line1\r\nline2\nline3\r\nline4\n";
        // When file is CRLF:
        let lf_only = mixed_input.replace("\r\n", "\n");
        let crlf_normalized = lf_only.replace('\n', "\r\n");
        assert_eq!(crlf_normalized, "line1\r\nline2\r\nline3\r\nline4\r\n");

        // When file is LF:
        let lf_normalized = mixed_input.replace("\r\n", "\n");
        assert_eq!(lf_normalized, "line1\nline2\nline3\nline4\n");
    }

    #[test]
    fn test_persist_with_retry_succeeds() {
        let dir = crate::safe_tempdir();
        let target_file = dir.path().join("test_persist.txt");
        fs::write(&target_file, "initial").unwrap();

        let mut temp_file = tempfile::Builder::new()
            .prefix(".test-persist-")
            .suffix(".tmp")
            .tempfile_in(dir.path())
            .unwrap();
        temp_file.write_all(b"updated").unwrap();
        temp_file.flush().unwrap();

        persist_with_retry(temp_file, &target_file, Some(b"initial"))
            .expect("persist_with_retry must succeed");
        assert_eq!(fs::read_to_string(&target_file).unwrap(), "updated");
    }
}
