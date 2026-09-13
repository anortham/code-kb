use rusqlite::Connection;
use std::path::Path;
use thiserror::Error;

use crate::formatters::{
    OutlineNode, add_path_to_outline, format_file_skeleton, render_outline_tree,
};
use crate::models::{BlastRadiusResult, ContextSlice, Symbol};
use crate::queries::{self, QueryError};
use crate::slicer::{self, SliceError};
use crate::sync::{self, SyncError};
use crate::workspace::{Workspace, WorkspaceError};

#[derive(Debug, Error)]
pub enum OpError {
    #[error("Symbol '{0}' not found")]
    SymbolNotFound(String),
    #[error("Symbol '{0}' not found. Did you mean one of:\n{1}")]
    SymbolNotFoundWithSuggestions(String, String),
    #[error("File '{0}' not found")]
    FileNotFound(String),
    #[error("Path '{0}' is a directory, not a file")]
    IsADirectory(String),
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
        let (effective_abs, rel) = workspace.resolve_path(Path::new(fp))?;
        if !effective_abs.exists() {
            return Err(OpError::FileNotFound(rel));
        }
        if effective_abs.is_dir() {
            return Err(OpError::IsADirectory(rel));
        }
        sync::ensure_fresh_file(workspace, db_path, conn, &rel)?;
        Some(rel)
    } else {
        None
    };

    // 2. Query symbol from database
    let initial_symbol = queries::get_symbol_by_name(conn, symbol_name, resolved_rel.as_deref())?
        .ok_or_else(|| {
        let suggestions = queries::search_symbols_scoped(
            conn,
            symbol_name,
            None,
            resolved_rel.as_deref(),
            false,
            3,
        )
        .unwrap_or_default();
        if suggestions.is_empty() {
            OpError::SymbolNotFound(symbol_name.to_string())
        } else {
            let list = suggestions
                .into_iter()
                .map(|s| format!("  - {} `{}` ({}:{})", s.kind, s.name, s.path, s.start_line))
                .collect::<Vec<_>>()
                .join("\n");
            OpError::SymbolNotFoundWithSuggestions(symbol_name.to_string(), list)
        }
    })?;

    // 3. If file_path was not provided initially, refresh the file found from the symbol
    let symbol = if resolved_rel.is_none() {
        let was_refreshed =
            sync::ensure_fresh_file(workspace, db_path, conn, &initial_symbol.path)?;
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

    let callee_signatures =
        queries::find_callee_signatures(conn, &target_symbol.name, &target_symbol.symbol_id, 10)
            .unwrap_or_default();

    // Find related types
    let mut related_types = Vec::new();
    if let Ok(types) = queries::find_type_facts(conn, &target_symbol.symbol_id) {
        for t in types {
            if !related_types.contains(&t.resolved_type) {
                related_types.push(t.resolved_type);
            }
        }
    }

    let related_tests = queries::find_related_tests(conn, &target_symbol, 5).unwrap_or_default();

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
    let (effective_abs, rel_path) = workspace.resolve_path(Path::new(file_path))?;
    if !effective_abs.exists() {
        return Err(OpError::FileNotFound(rel_path));
    }
    if effective_abs.is_dir() {
        return Err(OpError::IsADirectory(rel_path));
    }
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
    let rel_filter = path_filter.map(|p| workspace.relativize_filter(p));
    let path_filter = rel_filter.as_deref().filter(|path| !path.is_empty());
    let symbols_by_file = queries::load_scoped_outline_symbols(conn, path_filter, depth, 5)?;
    let norm = path_filter.map(|p| p.replace('\\', "/").trim_matches('/').to_string());
    let prefix = norm
        .as_ref()
        .map(|path| format!("{}/%", queries::escape_like(path)));

    let mut stmt = conn
        .prepare(
            "SELECT path FROM files
             WHERE (:path IS NULL OR path = :path OR path LIKE :path_prefix ESCAPE '\\')
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
    let mut files_found = 0;

    while let Some(row) = rows.next().map_err(QueryError::Sqlite)? {
        files_found += 1;
        let file_path: String = row.get(0).map_err(QueryError::Sqlite)?;
        add_path_to_outline(
            &mut root_node,
            &file_path,
            &symbols_by_file,
            depth,
            norm_filter,
        );
    }

    if let Some(filter) = path_filter
        && files_found == 0
    {
        return Err(OpError::FileNotFound(filter.to_string()));
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

/// Compute blast radius and likely tests for a symbol, file, or uncommitted git changes.
pub fn blast_radius_op(
    workspace: &Workspace,
    conn: &Connection,
    symbol: Option<&str>,
    file: Option<&str>,
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, OpError> {
    let mut seed_symbols = Vec::new();
    let mut seed_paths = Vec::new();

    let clean_symbol = symbol.and_then(|s| {
        let t = s.trim();
        if t.is_empty() { None } else { Some(t) }
    });
    let clean_file = file.and_then(|f| {
        let t = f.trim();
        if t.is_empty() {
            None
        } else {
            Some(workspace.relativize_filter(t))
        }
    });

    let mut discovered = Vec::new();

    if let Some(s) = clean_symbol {
        seed_symbols.push(s);
    }
    if let Some(ref f) = clean_file {
        seed_paths.push(f.as_str());
    }
    if seed_symbols.is_empty() && seed_paths.is_empty() {
        // Zero arguments: discover uncommitted working tree changes via git status
        let git_status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&workspace.root)
            .output();

        if let Ok(output) = git_status
            && output.status.success()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let trimmed = line.trim();
                if trimmed.len() > 3 {
                    let path_part = trimmed[2..].trim();
                    let target = if let Some((_, to)) = path_part.split_once("->") {
                        to.trim()
                    } else {
                        path_part
                    };
                    let p = target.trim_matches('"');
                    if !p.is_empty() && !crate::workspace::is_hard_excluded(p) {
                        discovered.push(p.to_string());
                    }
                }
            }
        }
        for d in &discovered {
            seed_paths.push(d.as_str());
        }
    }

    let depth = if max_depth == 0 { 2 } else { max_depth.min(5) };
    let row_limit = if limit == 0 { 20 } else { limit };

    let res = queries::compute_blast_radius(conn, &seed_symbols, &seed_paths, depth, row_limit)?;
    Ok(res)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::codebase_outline_op;
    use crate::workspace::Workspace;

    #[test]
    fn codebase_outline_accepts_absolute_workspace_root_filter() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("root.rs"), "pub fn root() {}\n").unwrap();
        let workspace = Workspace::new(temp.path().to_path_buf());
        let conn = Connection::open(temp.path().join("index.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT
            );
            CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            INSERT INTO files VALUES ('f', 'root.rs', 'rust', 'hash', 17, 1, 'now');
            INSERT INTO symbols VALUES (
                's', 'f', 'root.rs', 'rust', 'root', 'function', 'pub fn root()', NULL,
                'pub', NULL, 1, 0, 1, 16, 0, 16, 1, 0, 1, 16, 0, 16, NULL, NULL, 0, 0
            );",
        )
        .unwrap();

        let outline =
            codebase_outline_op(&workspace, &conn, 1, Some(temp.path().to_str().unwrap())).unwrap();

        assert!(outline.contains("root.rs"));

        fs::create_dir(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "pub fn nested() {}\n").unwrap();
        conn.execute_batch(
            "INSERT INTO files VALUES ('nested-file', 'src/lib.rs', 'rust', 'hash', 19, 1, 'now');
             INSERT INTO symbols VALUES (
                'nested-symbol', 'nested-file', 'src/lib.rs', 'rust', 'nested', 'function',
                'pub fn nested()', NULL, 'pub', NULL, 1, 0, 1, 18, 0, 18, 1, 0, 1, 18, 0, 18,
                NULL, NULL, 0, 0
             );",
        )
        .unwrap();

        assert!(
            codebase_outline_op(&workspace, &conn, 2, Some("."))
                .unwrap()
                .contains("root.rs")
        );
        assert!(
            codebase_outline_op(&workspace, &conn, 1, Some("src"))
                .unwrap()
                .contains("lib.rs")
        );
        assert!(
            codebase_outline_op(
                &workspace,
                &conn,
                1,
                Some(temp.path().join("src").to_str().unwrap()),
            )
            .unwrap()
            .contains("lib.rs")
        );
        assert!(
            codebase_outline_op(
                &workspace,
                &conn,
                1,
                Some(temp.path().parent().unwrap().to_str().unwrap()),
            )
            .is_err()
        );
    }
}
