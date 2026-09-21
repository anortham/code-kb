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
    #[error("Symbol '{name}' not found in {workspace}. {hint}")]
    SymbolNotFound {
        name: String,
        workspace: String,
        hint: String,
    },
    #[error("File '{path}' not found in {workspace}. {hint}")]
    FileNotFound {
        path: String,
        workspace: String,
        hint: String,
    },
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

fn symbol_not_found(conn: &Connection, name: &str, path_filter: Option<&str>) -> OpError {
    let (workspace, hint) = queries::symbol_not_found_parts(conn, name, path_filter);
    OpError::SymbolNotFound {
        name: name.to_string(),
        workspace,
        hint,
    }
}

fn file_not_found(conn: &Connection, rel_path: &str) -> OpError {
    let (workspace, hint) = queries::file_not_found_parts(conn, rel_path);
    OpError::FileNotFound {
        path: rel_path.to_string(),
        workspace,
        hint,
    }
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
            return Err(file_not_found(conn, &rel));
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
    let initial_symbol =
        queries::get_symbol_by_name(conn, symbol_name, resolved_rel.as_deref())?
            .ok_or_else(|| symbol_not_found(conn, symbol_name, resolved_rel.as_deref()))?;

    // 3. If file_path was not provided initially, refresh the file found from the symbol
    let symbol = if resolved_rel.is_none() {
        let was_refreshed =
            sync::ensure_fresh_file(workspace, db_path, conn, &initial_symbol.path)?;
        if was_refreshed {
            // CRUCIAL: Reload symbol after re-indexing so we have fresh offsets!
            queries::get_symbol_by_name_exact(conn, symbol_name, &initial_symbol.path)?
                .ok_or_else(|| symbol_not_found(conn, symbol_name, resolved_rel.as_deref()))?
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
    include_external: bool,
) -> Result<ContextSlice, OpError> {
    let (target_symbol, target_body) =
        get_symbol_body_op(workspace, db_path, conn, symbol_name, file_path)?;

    let callee_signatures = queries::find_callee_signatures(
        conn,
        &target_symbol.name,
        &target_symbol.symbol_id,
        10,
        include_external,
    )?;

    // Find related types
    let mut related_types = Vec::new();
    let types = queries::find_type_facts(conn, &target_symbol.symbol_id)?;
    for t in types {
        if !related_types.contains(&t.resolved_type) {
            related_types.push(t.resolved_type);
        }
    }

    let related_tests = queries::find_related_tests(conn, &target_symbol, 5)?;

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
        return Err(file_not_found(conn, &rel_path));
    }
    if effective_abs.is_dir() {
        return codebase_outline_op(workspace, conn, 2, Some(&rel_path));
    }
    sync::ensure_fresh_file(workspace, db_path, conn, &rel_path)?;

    let symbols = queries::load_file_symbols(conn, &rel_path)?;
    let file_meta = queries::get_file(conn, &rel_path)?;
    let line_count = file_meta.and_then(|m| m.line_count.map(|l| l as usize));
    let parse_errors = queries::count_parse_diagnostics(conn, &rel_path);

    Ok(format_file_skeleton(
        &rel_path,
        &symbols,
        line_count,
        parse_errors,
    ))
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
    let norm = path_filter
        .map(|p| p.replace('\\', "/").trim_matches('/').to_string())
        .filter(|p| !p.is_empty());
    let norm_bs = norm.as_ref().map(|p| p.replace('/', "\\"));
    let prefix = norm
        .as_ref()
        .map(|path| format!("{}/%", queries::escape_like(path)));
    let prefix_bs = norm_bs
        .as_ref()
        .map(|path| format!("{}\\\\%", queries::escape_like(path)));

    let mut stmt = conn
        .prepare(
            "SELECT path FROM files
             WHERE (:path IS NULL
                OR path = :path COLLATE NOCASE
                OR path = :path_bs COLLATE NOCASE
                OR path LIKE :path_prefix ESCAPE '\\'
                OR path LIKE :path_prefix_bs ESCAPE '\\')
             ORDER BY path ASC
             LIMIT 1001",
        )
        .map_err(QueryError::Sqlite)?;

    let mut rows = stmt
        .query(rusqlite::named_params! {
            ":path": norm.as_deref(),
            ":path_bs": norm_bs.as_deref(),
            ":path_prefix": prefix.as_deref(),
            ":path_prefix_bs": prefix_bs.as_deref(),
        })
        .map_err(QueryError::Sqlite)?;

    let mut file_paths = Vec::new();
    let mut files_found = 0;
    let mut truncated = false;

    while let Some(row) = rows.next().map_err(QueryError::Sqlite)? {
        files_found += 1;
        if files_found > 1000 {
            truncated = true;
            break;
        }
        let file_path: String = row.get(0).map_err(QueryError::Sqlite)?;
        file_paths.push(file_path);
    }

    if let Some(filter) = path_filter
        && files_found == 0
    {
        return Err(file_not_found(conn, filter));
    }

    let symbols_by_file = if file_paths.is_empty() {
        std::collections::HashMap::new()
    } else {
        queries::load_scoped_outline_symbols(conn, path_filter, depth, 5)?
    };

    let mut root_node = OutlineNode::default();
    let norm_filter = norm.as_deref().unwrap_or_default();

    for file_path in &file_paths {
        add_path_to_outline(
            &mut root_node,
            file_path,
            &symbols_by_file,
            depth,
            norm_filter,
        );
    }

    let display_root = if norm_filter.is_empty() {
        format!("{}/", workspace.repo_name)
    } else {
        format!("{}/{}/", workspace.repo_name, norm_filter)
    };

    let mut out = String::new();
    out.push_str(&format!("{display_root}\n"));
    render_outline_tree(&mut out, &root_node, "", 0, depth);

    if truncated {
        let msg = if path_filter.is_some() {
            "\n[Outline truncated: path matches over 1,000 files. Narrow your path filter or specify a deeper path to reduce scope.]\n"
        } else {
            "\n[Outline truncated: workspace contains over 1,000 files. Use a path filter (e.g. `code-kb outline <path>`) to narrow scope.]\n"
        };
        out.push_str(msg);
    }

    let unsupported = queries::count_unsupported_files(conn, norm.as_deref());
    if unsupported > 0 {
        let noun = if unsupported == 1 { "file" } else { "files" };
        out.push_str(&format!(
            "\n[{unsupported} unsupported {noun}: no extractor for the language]\n"
        ));
    }

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
    let (seed_symbols, symbol_path_filter, seed_paths) = match (clean_symbol, clean_file) {
        (Some(s), Some(f)) => (vec![s], Some(f), vec![]),
        (Some(s), None) => (vec![s], None, vec![]),
        (None, Some(f)) => (vec![], None, vec![f]),
        (None, None) => {
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
                    if line.len() > 3 {
                        let path_part = line.get(3..).unwrap_or("").trim();
                        let target = if let Some((_, to)) = path_part.split_once("->") {
                            to.trim()
                        } else {
                            path_part
                        };
                        let p = target.trim_matches('"');
                        let p_fwd = p.replace('\\', "/");
                        if !p_fwd.is_empty() && !crate::workspace::is_hard_excluded(&p_fwd) {
                            discovered.push(p_fwd);
                        }
                    }
                }
            }
            (vec![], None, discovered)
        }
    };

    let depth = if max_depth == 0 { 2 } else { max_depth.min(5) };
    let row_limit = if limit == 0 { 20 } else { limit };

    let seed_paths_refs: Vec<&str> = seed_paths.iter().map(|s| s.as_str()).collect();
    let res = queries::compute_blast_radius_scoped(
        conn,
        &seed_symbols,
        symbol_path_filter.as_deref(),
        &seed_paths_refs,
        depth,
        row_limit,
    )?;
    Ok(res)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rusqlite::Connection;

    use super::codebase_outline_op;
    use crate::workspace::Workspace;

    #[test]
    fn codebase_outline_accepts_absolute_workspace_root_filter() {
        let temp = crate::safe_tempdir();
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

    #[test]
    fn codebase_outline_counts_unsupported_files() {
        let temp = crate::safe_tempdir();
        let workspace = Workspace::new(temp.path().to_path_buf());
        let conn = Connection::open(temp.path().join("index.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT, status TEXT
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
            INSERT INTO files VALUES ('f1', 'src/lib.rs', 'rust', 'h', 0, 1, 'now', 'indexed');
            INSERT INTO files VALUES ('f2', 'src/blob.bin', 'unknown', 'h', 0, 1, 'now', 'unsupported');
            INSERT INTO files VALUES ('f3', 'docs/blob.bin', 'unknown', 'h', 0, 1, 'now', 'unsupported');
            INSERT INTO symbols VALUES (
                's', 'f1', 'src/lib.rs', 'rust', 'root', 'function', 'pub fn root()', NULL,
                'pub', NULL, 1, 0, 1, 16, 0, 16, 1, 0, 1, 16, 0, 16, NULL, NULL, 0, 0
            );",
        )
        .unwrap();

        let all = codebase_outline_op(&workspace, &conn, 2, None).unwrap();
        assert!(all.contains("2 unsupported files"));

        let scoped = codebase_outline_op(&workspace, &conn, 2, Some("src")).unwrap();
        assert!(scoped.contains("1 unsupported file:"));
    }

    #[test]
    fn codebase_outline_truncates_over_1000_files() {
        let temp = crate::safe_tempdir();
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
            );",
        )
        .unwrap();

        for i in 1..=1005 {
            conn.execute(
                "INSERT INTO files VALUES (?1, ?2, 'rust', 'hash', 10, 1, 'now')",
                rusqlite::params![format!("f{i}"), format!("src/file_{i}.rs")],
            )
            .unwrap();
        }

        let outline = codebase_outline_op(&workspace, &conn, 2, None).unwrap();
        assert!(outline.contains("[Outline truncated: workspace contains over 1,000 files."));

        let scoped_outline = codebase_outline_op(&workspace, &conn, 2, Some("src")).unwrap();
        assert!(scoped_outline.contains("[Outline truncated: path matches over 1,000 files."));
    }
}
