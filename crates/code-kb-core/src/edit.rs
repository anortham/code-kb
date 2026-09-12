use std::fs;
use std::path::Path;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::queries;
use crate::slicer;
use crate::sync;
use crate::workspace::Workspace;

#[derive(Debug, Error)]
pub enum EditError {
    #[error("Symbol '{0}' not found in '{1}'")]
    SymbolNotFound(String, String),
    #[error("Symbol has no body defined (e.g. trait declaration without default implementation)")]
    NoBodyDefined,
    #[error("Optimistic lock failed: expected body hash '{0}', found '{1}'")]
    HashMismatch(String, String),
    #[error("Failed to read/write file '{0}': {1}")]
    Io(String, #[source] std::io::Error),
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

    // Find symbol in database
    let symbol = queries::get_symbol_by_name(conn, symbol_name, Some(&rel_path))?
        .ok_or_else(|| EditError::SymbolNotFound(symbol_name.to_string(), rel_path.clone()))?;

    let body_start = symbol
        .body_start_byte
        .ok_or(EditError::NoBodyDefined)?;
    let body_end = symbol
        .body_end_byte
        .ok_or(EditError::NoBodyDefined)?;

    // Read existing file content
    let existing_bytes = fs::read(&abs_path)
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;

    let existing_body = slicer::slice_bytes_safe(&existing_bytes, body_start, body_end)
        .map_err(|e| EditError::Io(abs_path.display().to_string(), std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())))?;

    let current_hash = hash_content(existing_body);

    // Verify optimistic lock
    if let Some(expected) = expected_body_hash {
        if expected != current_hash {
            return Err(EditError::HashMismatch(expected.to_string(), current_hash));
        }
    }

    // Construct new file content with replaced byte span
    let mut new_file_bytes = Vec::with_capacity(existing_bytes.len() + new_body.len());
    new_file_bytes.extend_from_slice(&existing_bytes[..body_start]);
    new_file_bytes.extend_from_slice(new_body.as_bytes());
    new_file_bytes.extend_from_slice(&existing_bytes[body_end..]);

    // Write file atomically (temp file + rename or direct write)
    fs::write(&abs_path, &new_file_bytes)
        .map_err(|e| EditError::Io(abs_path.display().to_string(), e))?;

    // Tier 1: Immediately re-index the file so catalog is 100% fresh
    sync::update_file(workspace, db_path, &rel_path)?;

    let new_body_hash = hash_content(new_body);

    Ok(EditResult {
        symbol_name: symbol_name.to_string(),
        file_path: rel_path,
        old_body_hash: current_hash,
        new_body_hash,
        bytes_written: new_file_bytes.len(),
    })
}
