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
use crate::workspace::{Workspace, WorkspaceError, paths_equal};

#[derive(Debug, Clone)]
pub enum SymbolSelector {
    Name(String),
    Id(String),
}

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
    #[error(
        "symbol_id '{id}' is no longer indexed in {workspace}; run lookup_symbol or search_symbols and select a current id"
    )]
    StaleSymbolId { id: String, workspace: String },
    #[error("A database path is required to refresh a symbol_id selector")]
    SymbolIdRequiresDatabasePath,
    #[error("Selected symbol is in '{actual}', not requested file '{requested}'")]
    FileGuardMismatch { requested: String, actual: String },
    #[error("Workspace error: {0}")]
    Workspace(#[from] WorkspaceError),
    #[error("Synchronization error: {0}")]
    Sync(#[from] SyncError),
    #[error("Query error: {0}")]
    Query(#[from] QueryError),
    #[error("Slice error: {0}")]
    Slice(#[from] SliceError),
}

fn stale_symbol_id(conn: &Connection, id: &str) -> OpError {
    OpError::StaleSymbolId {
        id: id.to_string(),
        workspace: queries::workspace_name(conn),
    }
}

pub fn resolve_symbol_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
) -> Result<Symbol, OpError> {
    let id_selector = matches!(selector, SymbolSelector::Id(_));
    let guard = if let Some(file_path) = file_path {
        let (absolute, rel) = workspace.resolve_path(Path::new(file_path))?;
        if !absolute.exists() {
            return Err(file_not_found(conn, &rel));
        }
        if absolute.is_dir() {
            return Err(OpError::IsADirectory(rel));
        }
        if !id_selector {
            sync::ensure_fresh_file(workspace, db_path, conn, &rel)?;
        }
        Some((absolute, rel))
    } else {
        None
    };
    let lookup = |conn: &Connection| match selector {
        SymbolSelector::Name(name) => {
            queries::get_symbol_by_name(conn, name, guard.as_ref().map(|(_, rel)| rel.as_str()))
        }
        SymbolSelector::Id(id) => queries::get_symbol_by_id(conn, id),
    };
    let initial = lookup(conn)?.ok_or_else(|| match selector {
        SymbolSelector::Name(name) => {
            symbol_not_found(conn, name, guard.as_ref().map(|(_, rel)| rel.as_str()))
        }
        SymbolSelector::Id(id) => stale_symbol_id(conn, id),
    })?;
    if !id_selector
        && let Some((requested_abs, requested_rel)) = &guard
        && !paths_equal(requested_abs, &workspace.canonical_root.join(&initial.path))
    {
        return Err(OpError::FileGuardMismatch {
            requested: requested_rel.clone(),
            actual: initial.path,
        });
    }
    let refreshed = sync::ensure_fresh_file(workspace, db_path, conn, &initial.path)?;
    let refreshed_symbol = if refreshed {
        match selector {
            SymbolSelector::Name(name) => {
                queries::get_symbol_by_name_exact(conn, name, &initial.path)?
            }
            SymbolSelector::Id(id) => queries::get_symbol_by_id(conn, id)?,
        }
        .ok_or_else(|| match selector {
            SymbolSelector::Name(name) => {
                symbol_not_found(conn, name, guard.as_ref().map(|(_, rel)| rel.as_str()))
            }
            SymbolSelector::Id(id) => stale_symbol_id(conn, id),
        })?
    } else {
        initial
    };
    if let Some((requested_abs, requested_rel)) = &guard
        && !paths_equal(
            requested_abs,
            &workspace.canonical_root.join(&refreshed_symbol.path),
        )
    {
        return Err(OpError::FileGuardMismatch {
            requested: requested_rel.clone(),
            actual: refreshed_symbol.path,
        });
    }
    Ok(refreshed_symbol)
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
    get_symbol_body_selected_op(
        workspace,
        db_path,
        conn,
        &SymbolSelector::Name(symbol_name.to_string()),
        file_path,
    )
}

pub fn get_symbol_body_selected_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
) -> Result<(Symbol, String), OpError> {
    let symbol = resolve_symbol_op(workspace, db_path, conn, selector, file_path)?;

    let abs_file = workspace.canonical_root.join(&symbol.path);
    let start = decorated_start(conn, &symbol).unwrap_or(symbol.start_byte);
    let source = slicer::slice_symbol_source(&abs_file, &symbol, start)?;

    Ok((symbol, source))
}

/// Where the decorators above a Python symbol begin: julie starts the symbol at its `def` or
/// `class` line and records the decorated block as a `decorated_definition` fact.
fn decorated_start(conn: &Connection, symbol: &Symbol) -> Option<usize> {
    conn.query_row(
        "SELECT MAX(start_byte) FROM structural_facts
         WHERE path = ?1 AND pattern_id LIKE '%.decorated_definition.%'
           AND end_byte = ?2 AND start_byte < ?3",
        rusqlite::params![
            symbol.path,
            symbol.end_byte as i64,
            symbol.start_byte as i64
        ],
        |row| row.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
    .map(|start| start as usize)
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
    get_context_slice_selected_op(
        workspace,
        db_path,
        conn,
        &SymbolSelector::Name(symbol_name.to_string()),
        file_path,
        include_external,
    )
}

pub fn get_context_slice_selected_op(
    workspace: &Workspace,
    db_path: &Path,
    conn: &Connection,
    selector: &SymbolSelector,
    file_path: Option<&str>,
    include_external: bool,
) -> Result<ContextSlice, OpError> {
    let (target_symbol, target_body) =
        get_symbol_body_selected_op(workspace, db_path, conn, selector, file_path)?;

    let callee_signatures = queries::find_callee_signatures(
        conn,
        &target_symbol.name,
        &target_symbol.symbol_id,
        10,
        include_external,
    )?;

    let related_types = queries::find_related_types(conn, &target_symbol, 10)?;
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

    let mut symbols = queries::load_file_symbols(conn, &rel_path)?;
    let inherited =
        queries::inherited_writes_among(conn, symbols.iter().map(|s| s.symbol_id.as_str()))?;
    symbols.retain(|symbol| !inherited.contains(&symbol.symbol_id));
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
    let scope = queries::OutlineScope::new(path_filter, depth);
    let symbols_by_file = queries::load_scoped_outline_symbols(conn, path_filter, depth, 5)?;
    let counts = queries::load_outline_counts(conn, path_filter, depth)?;

    let mut root_node = OutlineNode::default();
    let norm_filter = norm.as_deref().unwrap_or_default();
    let mut files_found = 0;
    let mut listed = 0;
    let mut truncated = false;
    queries::for_each_outline_path(conn, &scope, |file_path| {
        files_found += 1;
        if scope.lists(&file_path) {
            listed += 1;
            if listed > queries::OUTLINE_FILE_CAP {
                truncated = true;
                return false;
            }
        }
        add_path_to_outline(
            &mut root_node,
            &file_path,
            &symbols_by_file,
            &counts,
            depth,
            norm_filter,
        );
        true
    })?;

    let unsupported = queries::count_unsupported_files(conn, norm.as_deref());
    if let Some(filter) = path_filter
        && files_found == 0
        && unsupported == 0
    {
        return Err(file_not_found(conn, filter));
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
    blast_radius_selected_op(
        workspace,
        None,
        conn,
        symbol.map(|s| SymbolSelector::Name(s.to_string())),
        file,
        max_depth,
        limit,
    )
}

pub fn blast_radius_selected_op(
    workspace: &Workspace,
    db_path: Option<&Path>,
    conn: &Connection,
    selector: Option<SymbolSelector>,
    file: Option<&str>,
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, OpError> {
    if let Some(selector @ SymbolSelector::Id(_)) = selector.as_ref() {
        let db_path = db_path.ok_or(OpError::SymbolIdRequiresDatabasePath)?;
        let selected = resolve_symbol_op(workspace, db_path, conn, selector, file)?;
        return Ok(queries::compute_blast_radius_scoped_with_ids(
            conn,
            &[],
            &[&selected.symbol_id],
            None,
            &[],
            if max_depth == 0 { 2 } else { max_depth.min(5) },
            limit,
        )?);
    }
    let symbol = selector.as_ref().and_then(|selector| match selector {
        SymbolSelector::Name(name) => Some(name.as_str()),
        SymbolSelector::Id(_) => None,
    });
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
    let seed_paths_refs: Vec<&str> = seed_paths.iter().map(|s| s.as_str()).collect();
    let res = queries::compute_blast_radius_scoped(
        conn,
        &seed_symbols,
        symbol_path_filter.as_deref(),
        &seed_paths_refs,
        depth,
        limit,
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
    fn codebase_outline_names_plain_files_and_shows_an_overload_once() {
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
            INSERT INTO files VALUES ('f1', 'src/helpers.py', 'python', 'h', 0, 9, 'now', 'indexed');
            INSERT INTO files VALUES ('f2', 'src/signals.py', 'python', 'h', 0, 3, 'now', 'indexed');
            INSERT INTO files VALUES ('f3', 'src/py.typed', 'unknown', 'h', 0, 0, 'now', 'unsupported');
            INSERT INTO symbols VALUES (
                'a', 'f1', 'src/helpers.py', 'python', 'stream', 'function', 'def stream()', NULL,
                NULL, NULL, 1, 0, 2, 0, 0, 10, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            INSERT INTO symbols VALUES (
                'b', 'f1', 'src/helpers.py', 'python', 'stream', 'function', 'def stream(x)', NULL,
                NULL, NULL, 4, 0, 5, 0, 11, 20, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            INSERT INTO symbols VALUES (
                'c', 'f2', 'src/signals.py', 'python', 'started', 'variable', 'started = Signal()',
                NULL, NULL, NULL, 1, 0, 1, 0, 0, 18, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );",
        )
        .unwrap();

        let out = codebase_outline_op(&workspace, &conn, 2, Some("src")).unwrap();

        assert!(out.contains("helpers.py [function stream]\n"), "{out}");
        assert!(
            out.contains("(1 file without functions or classes: signals.py)"),
            "{out}"
        );
        assert!(!out.contains("py.typed"), "{out}");
    }

    #[test]
    fn codebase_outline_lists_namespaced_classes_past_deep_files_and_caps_each_subfolder() {
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
            INSERT INTO files VALUES ('z', 'z/Late.cs', 'csharp', 'h', 0, 9, 'now', 'indexed');
            INSERT INTO symbols VALUES (
                'ns', 'z', 'z/Late.cs', 'csharp', 'App', 'namespace', 'namespace App', NULL,
                NULL, NULL, 1, 0, 9, 0, 0, 90, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            INSERT INTO symbols VALUES (
                'late', 'z', 'z/Late.cs', 'csharp', 'Late', 'class', 'class Late', NULL,
                NULL, 'ns', 2, 0, 8, 0, 20, 80, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );",
        )
        .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        for i in 0..1005 {
            tx.execute(
                "INSERT INTO files VALUES (?1, ?2, 'rust', 'h', 0, 1, 'now', 'indexed')",
                rusqlite::params![format!("d{i}"), format!("a/deep/x/file_{i}.rs")],
            )
            .unwrap();
        }
        for i in 10..55 {
            let path = format!("m/mod_{i}.rs");
            tx.execute(
                "INSERT INTO files VALUES (?1, ?2, 'rust', 'h', 0, 1, 'now', 'indexed')",
                rusqlite::params![format!("m{i}"), path],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO symbols VALUES (?1, ?2, ?3, 'rust', ?4, 'function', 'fn f()', NULL,
                 NULL, NULL, 1, 0, 1, 0, 0, 9, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0)",
                rusqlite::params![format!("s{i}"), format!("m{i}"), path, format!("f{i}")],
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let out = codebase_outline_op(&workspace, &conn, 2, None).unwrap();

        assert!(out.contains("deep/ (1005 indexed files)"), "{out}");
        assert!(out.contains("Late.cs [class Late]"), "{out}");
        assert!(out.contains("mod_49.rs [function f49]"), "{out}");
        assert!(!out.contains("mod_50.rs"), "{out}");
        assert!(
            out.contains("(+5 more files with functions or classes; pass this folder as the path"),
            "{out}"
        );
        assert!(!out.contains("truncated"), "{out}");

        let scoped = codebase_outline_op(&workspace, &conn, 2, Some("m")).unwrap();
        assert!(scoped.contains("mod_54.rs [function f54]"), "{scoped}");
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

        let tx = conn.unchecked_transaction().unwrap();
        for i in 1..=1005 {
            tx.execute(
                "INSERT INTO files VALUES (?1, ?2, 'rust', 'hash', 10, 1, 'now')",
                rusqlite::params![format!("f{i}"), format!("src/file_{i}.rs")],
            )
            .unwrap();
        }
        tx.commit().unwrap();

        let outline = codebase_outline_op(&workspace, &conn, 2, None).unwrap();
        assert!(outline.contains("[Outline truncated: workspace contains over 1,000 files."));

        let scoped_outline = codebase_outline_op(&workspace, &conn, 2, Some("src")).unwrap();
        assert!(scoped_outline.contains("[Outline truncated: path matches over 1,000 files."));
    }
}
