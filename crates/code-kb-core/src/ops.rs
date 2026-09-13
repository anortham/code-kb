use std::path::Path;
use rusqlite::Connection;
use thiserror::Error;

use crate::formatters::{
    add_path_to_outline, format_file_skeleton, render_outline_tree, OutlineNode,
};
use crate::models::{ContextSlice, Symbol};
use crate::queries::{self, QueryError};
use crate::slicer::{self, SliceError};
use crate::sync::{self, SyncError};
use crate::workspace::{Workspace, WorkspaceError};

#[derive(Debug, Error)]
pub enum OpError {
    #[error("Symbol '{0}' not found")]
    SymbolNotFound(String),
    #[error("Workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("Synchronization error: {0}")]
    Sync(#[from] SyncError),
    #[error("Query error: {0}")]
    Query(#[from] QueryError),
    #[error("Slice error: {0}")]
    Slice(#[from] SliceError),
}

/// Retrieve the body of a symbol by name, guaranteeing fresh offsets and file contents.
pub fn get_symbol_body_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    symbol_name: &str,
    file_path: Option<&str>,
) -> Result<(Symbol, String), OpError> {
    // 1. If file_path is provided, resolve and refresh file BEFORE querying the symbol
    let resolved_rel = if let Some(fp) = file_path {
        let (_, rel) = workspace.resolve_path(Path::new(fp))?;
        sync::ensure_fresh_file(workspace, db_path, conn, &rel)?;
        Some(rel)
    } else {
        None
    };

    // 2. Query symbol from database
    let initial_symbol = queries::get_symbol_by_name(conn, symbol_name, resolved_rel.as_deref())?
        .ok_or_else(|| OpError::SymbolNotFound(symbol_name.to_string()))?;

    // 3. If file_path was not provided initially, refresh the file found from the symbol
    let symbol = if resolved_rel.is_none() {
        let was_refreshed = sync::ensure_fresh_file(workspace, db_path, conn, &initial_symbol.path)?;
        if was_refreshed {
            // CRUCIAL: Reload symbol after re-indexing so we have fresh offsets!
            queries::get_symbol_by_name_exact(conn, symbol_name, &initial_symbol.path)?
                .ok_or_else(|| OpError::SymbolNotFound(symbol_name.to_string()))?
        } else {
            initial_symbol
        }
    } else {
        initial_symbol
    };

    let abs_file = workspace.canonical_root.join(&symbol.path);
    let body = slicer::slice_symbol_body(&abs_file, &symbol)?;

    Ok((symbol, body))
}

/// Retrieve a complete context slice for a symbol, guaranteeing fresh offsets.
pub fn get_context_slice_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    symbol_name: &str,
    file_path: Option<&str>,
) -> Result<ContextSlice, OpError> {
    let (target_symbol, target_body) =
        get_symbol_body_op(workspace, db_path, conn, symbol_name, file_path)?;

    // Find callees
    let mut callee_signatures = Vec::new();
    if let Ok(callees) = queries::find_references(conn, symbol_name, "callees", 10) {
        for c in callees {
            if let Ok(Some(s)) = queries::get_symbol_by_name(conn, &c.to_symbol_name, None) {
                let sig = s.signature.unwrap_or(s.name);
                callee_signatures.push(format!("{sig} ({}:{})", s.path, s.start_line));
            }
        }
    }

    // Find related types
    let mut related_types = Vec::new();
    if let Ok(types) = queries::find_type_facts(conn, &target_symbol.symbol_id) {
        for t in types {
            related_types.push(t.resolved_type);
        }
    }

    // Find related unit tests
    let related_tests = match queries::search_symbols(conn, symbol_name, None, true, 5) {
        Ok(tests) => tests.into_iter().filter(|s| s.is_test).collect(),
        Err(_) => Vec::new(),
    };

    Ok(ContextSlice {
        target_symbol,
        target_body,
        callee_signatures,
        related_types,
        related_tests,
    })
}

/// Generate file skeleton with fresh file synchronization.
pub fn file_skeleton_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    file_path: &str,
) -> Result<String, OpError> {
    let (_, rel_path) = workspace.resolve_path(Path::new(file_path))?;
    sync::ensure_fresh_file(workspace, db_path, conn, &rel_path)?;

    let symbols = queries::load_file_symbols(conn, &rel_path)?;
    let file_meta = queries::get_file(conn, &rel_path)?;
    let line_count = file_meta.and_then(|m| m.line_count.map(|l| l as usize));

    Ok(format_file_skeleton(&rel_path, &symbols, line_count))
}

/// Generate a memory-bounded codebase outline pushed down into SQLite.
pub fn codebase_outline_op(
    workspace: &Workspace,
    conn: &Connection,
    depth: usize,
    path_filter: Option<&str>,
) -> Result<String, OpError> {
    let symbols_by_file = queries::load_scoped_outline_symbols(conn, path_filter, depth, 5)?;
    let norm = path_filter.map(|p| p.replace('\\', "/").trim_matches('/').to_string());
    let prefix = norm.as_ref().map(|p| format!("{p}/%"));

    let mut stmt = conn
        .prepare(
            "SELECT path FROM files
             WHERE (:path IS NULL OR path = :path OR path LIKE :path_prefix)
             ORDER BY path ASC",
        )
        .map_err(QueryError::Sqlite)?;

    let mut rows = stmt
        .query(rusqlite::named_params! {
            ":path": norm.as_deref(),
            ":path_prefix": prefix.as_deref(),
        })
        .map_err(QueryError::Sqlite)?;

    let mut root_node = OutlineNode::default();
    let norm_filter = norm.as_deref().unwrap_or_default();

    while let Some(row) = rows.next().map_err(QueryError::Sqlite)? {
        let file_path: String = row.get(0).map_err(QueryError::Sqlite)?;
        add_path_to_outline(&mut root_node, &file_path, &symbols_by_file, depth, norm_filter);
    }

    let display_root = if norm_filter.is_empty() {
        format!("{}/", workspace.repo_name)
    } else {
        format!("{}/{}/", workspace.repo_name, norm_filter)
    };

    let mut out = String::new();
    out.push_str(&format!("{display_root}\n"));
    render_outline_tree(&mut out, &root_node, "", 0, depth);

    Ok(out)
}
