use std::collections::HashMap;
use rusqlite::{params, Connection, Row};
use thiserror::Error;

use crate::models::{
    FileFact, LiteralFact, ReferenceSite, StructuralFact, Symbol, SymbolSearchResult, TypeFact,
};

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("Database query error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Symbol '{0}' not found")]
    SymbolNotFound(String),
    #[error("Ambiguous symbol '{0}': found {1} matching candidates. Specify file_path or qualified name to disambiguate:\n{2}")]
    AmbiguousSymbol(String, usize, String),
}

fn map_symbol(row: &Row) -> rusqlite::Result<Symbol> {
    Ok(Symbol {
        symbol_id: row.get("symbol_id")?,
        file_id: row.get("file_id")?,
        path: row.get("path")?,
        language: row.get("language")?,
        name: row.get("name")?,
        kind: row.get("kind")?,
        signature: row.get("signature")?,
        doc_comment: row.get("doc_comment")?,
        visibility: row.get("visibility")?,
        parent_symbol_id: row.get("parent_symbol_id")?,
        start_line: row.get::<_, i64>("start_line")? as usize,
        start_column: row.get::<_, i64>("start_column")? as usize,
        end_line: row.get::<_, i64>("end_line")? as usize,
        end_column: row.get::<_, i64>("end_column")? as usize,
        start_byte: row.get::<_, i64>("start_byte")? as usize,
        end_byte: row.get::<_, i64>("end_byte")? as usize,
        body_start_line: row.get::<_, Option<i64>>("body_start_line")?.map(|v| v as usize),
        body_start_column: row.get::<_, Option<i64>>("body_start_column")?.map(|v| v as usize),
        body_end_line: row.get::<_, Option<i64>>("body_end_line")?.map(|v| v as usize),
        body_end_column: row.get::<_, Option<i64>>("body_end_column")?.map(|v| v as usize),
        body_start_byte: row.get::<_, Option<i64>>("body_start_byte")?.map(|v| v as usize),
        body_end_byte: row.get::<_, Option<i64>>("body_end_byte")?.map(|v| v as usize),
        body_hash: row.get("body_hash")?,
        semantic_group: row.get("semantic_group")?,
        is_test: row.get::<_, i64>("is_test")? != 0,
        test_container: row.get::<_, i64>("test_container")? != 0,
    })
}

/// Retrieve all indexed files from `files` table.
pub fn load_files(conn: &Connection) -> Result<Vec<FileFact>, QueryError> {
    load_scoped_files(conn, None)
}

/// Retrieve indexed files optionally scoped by path filter, pushed down to SQLite.
pub fn load_scoped_files(
    conn: &Connection,
    path_filter: Option<&str>,
) -> Result<Vec<FileFact>, QueryError> {
    let norm = path_filter.map(|p| p.replace('\\', "/").trim_matches('/').to_string());
    let prefix = norm.as_ref().map(|p| format!("{p}/%"));

    let sql = "SELECT file_id, path, language, content_hash, content_bytes, line_count, indexed_at
               FROM files
               WHERE (:path IS NULL OR path = :path OR path LIKE :path_prefix)
               ORDER BY path ASC";

    let mut stmt = conn.prepare(sql)?;
    let files = stmt
        .query_map(
            rusqlite::named_params! {
                ":path": norm.as_deref(),
                ":path_prefix": prefix.as_deref(),
            },
            |row| {
                Ok(FileFact {
                    file_id: row.get(0)?,
                    path: row.get(1)?,
                    language: row.get(2)?,
                    content_hash: row.get(3)?,
                    content_bytes: row.get(4)?,
                    line_count: row.get(5)?,
                    indexed_at: row.get(6)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(files)
}

/// Load up to `limit_per_file` symbols per file for scoped files, directly aggregated in SQLite.
/// Files deeper than `depth` are filtered out in SQLite to keep memory strictly bounded.
pub fn load_scoped_outline_symbols(
    conn: &Connection,
    path_filter: Option<&str>,
    depth: usize,
    limit_per_file: usize,
) -> Result<HashMap<String, Vec<Symbol>>, QueryError> {
    let norm = path_filter.map(|p| p.replace('\\', "/").trim_matches('/').to_string());
    let prefix = norm.as_ref().map(|p| format!("{p}/%"));

    let max_slashes = match &norm {
        None => {
            if depth > 0 {
                (depth - 1) as i64
            } else {
                0
            }
        }
        Some(f) => {
            let filter_slashes = f.chars().filter(|&c| c == '/').count();
            (filter_slashes + depth) as i64
        }
    };

    let sql = "
        WITH ranked AS (
            SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                   visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                   start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                   body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                   is_test, test_container,
                   ROW_NUMBER() OVER (PARTITION BY path ORDER BY start_line ASC) as rn
            FROM symbols
            WHERE (:path IS NULL OR path = :path OR path LIKE :path_prefix)
              AND (length(path) - length(replace(path, '/', '')) <= :max_slashes)
        )
        SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
               visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
               start_byte, end_byte, body_start_line, body_start_column, body_end_line,
               body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
               is_test, test_container
        FROM ranked
        WHERE rn <= :limit
        ORDER BY path ASC, start_line ASC
    ";

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(rusqlite::named_params! {
        ":path": norm.as_deref(),
        ":path_prefix": prefix.as_deref(),
        ":max_slashes": max_slashes,
        ":limit": limit_per_file as i64,
    })?;

    let mut symbols_by_file: HashMap<String, Vec<Symbol>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let sym = map_symbol(row)?;
        symbols_by_file.entry(sym.path.clone()).or_default().push(sym);
    }

    Ok(symbols_by_file)
}

/// Lookup single file metadata by path with slash-boundary matching.
pub fn get_file(conn: &Connection, path: &str) -> Result<Option<FileFact>, QueryError> {
    let normalized = path.replace('\\', "/");
    let backslash = path.replace('/', "\\");

    // Check exact path match first
    let mut stmt = conn.prepare(
        "SELECT file_id, path, language, content_hash, content_bytes, line_count, indexed_at
         FROM files
         WHERE path = ?1 OR path = ?2
         LIMIT 1",
    )?;

    let mut rows = stmt.query(params![normalized, backslash])?;
    if let Some(row) = rows.next()? {
        return Ok(Some(FileFact {
            file_id: row.get(0)?,
            path: row.get(1)?,
            language: row.get(2)?,
            content_hash: row.get(3)?,
            content_bytes: row.get(4)?,
            line_count: row.get(5)?,
            indexed_at: row.get(6)?,
        }));
    }

    // Fallback: boundary match only if exact match is absent
    let mut fallback_stmt = conn.prepare(
        "SELECT file_id, path, language, content_hash, content_bytes, line_count, indexed_at
         FROM files
         WHERE path LIKE '%/' || ?1
         LIMIT 1",
    )?;

    let mut f_rows = fallback_stmt.query(params![normalized])?;
    if let Some(row) = f_rows.next()? {
        Ok(Some(FileFact {
            file_id: row.get(0)?,
            path: row.get(1)?,
            language: row.get(2)?,
            content_hash: row.get(3)?,
            content_bytes: row.get(4)?,
            line_count: row.get(5)?,
            indexed_at: row.get(6)?,
        }))
    } else {
        Ok(None)
    }
}

/// Load all symbols declared inside a specific file.
pub fn load_file_symbols(conn: &Connection, file_path: &str) -> Result<Vec<Symbol>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
         FROM symbols
         WHERE path = ?1 OR path = ?2
         ORDER BY start_line ASC, start_column ASC",
    )?;

    // Normalizing slashes for path matching
    let normalized = file_path.replace('\\', "/");
    let backslash = file_path.replace('/', "\\");

    let rows = stmt
        .query_map(params![normalized, backslash], map_symbol)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Search symbols by name query, kind filter, and test flag.
pub fn search_symbols(
    conn: &Connection,
    query: &str,
    kind_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<Symbol>, QueryError> {
    let pattern = format!("%{query}%");

    let mut sql = String::from(
        "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
         FROM symbols
         WHERE (name = ?1 OR name LIKE ?2)",
    );

    if !include_tests {
        sql.push_str(" AND is_test = 0 AND test_container = 0");
    }

    if kind_filter.is_some() {
        sql.push_str(" AND kind = ?3");
    }

    sql.push_str(" ORDER BY (name = ?1) DESC, length(name) ASC, path ASC LIMIT ");
    sql.push_str(&limit.to_string());

    let mut stmt = conn.prepare(&sql)?;

    let rows = if let Some(k) = kind_filter {
        stmt.query_map(params![query, pattern, k], map_symbol)?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![query, pattern], map_symbol)?
            .collect::<Result<Vec<_>, _>>()?
    };

    if rows.is_empty()
        && let Ok(fts_matches) = fts_search_symbols(conn, query, kind_filter, include_tests, limit)
        && !fts_matches.is_empty()
    {
        return Ok(fts_matches.into_iter().map(|m| m.symbol).collect());
    }

    Ok(rows)
}

/// Sanitizes a free-form user query into `(and_query, or_query)` formatted for SQLite FTS5.
/// Each alphanumeric/underscore token is quoted and given a prefix wildcard: `"token"*`.
pub fn sanitize_fts5_query(query: &str) -> (String, String) {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{s}\"*"))
        .collect();

    if words.is_empty() {
        return (String::new(), String::new());
    }

    let and_query = words.join(" ");
    let or_query = words.join(" OR ");
    (and_query, or_query)
}

/// Conceptual full-text search over symbol names, signatures, and docstrings using FTS5 (BM25).
/// Evaluates multi-token AND matching first, falling back to OR ranking if AND yields 0 results.
pub fn fts_search_symbols(
    conn: &Connection,
    query: &str,
    kind_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<SymbolSearchResult>, QueryError> {
    let (and_q, or_q) = sanitize_fts5_query(query);
    if and_q.is_empty() {
        return Ok(Vec::new());
    }

    let fts_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='symbols_fts'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !fts_exists {
        let pattern = format!("%{query}%");
        let mut sql = String::from(
            "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                    visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                    start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                    body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                    is_test, test_container
             FROM symbols
             WHERE (name = ?1 OR name LIKE ?2)",
        );
        if !include_tests {
            sql.push_str(" AND is_test = 0 AND test_container = 0");
        }
        if kind_filter.is_some() {
            sql.push_str(" AND kind = ?3");
        }
        sql.push_str(" ORDER BY (name = ?1) DESC, length(name) ASC, path ASC LIMIT ");
        sql.push_str(&limit.to_string());

        let mut stmt = conn.prepare(&sql)?;
        let rows = if let Some(k) = kind_filter {
            stmt.query_map(params![query, pattern, k], map_symbol)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![query, pattern], map_symbol)?
                .collect::<Result<Vec<_>, _>>()?
        };

        return Ok(rows
            .into_iter()
            .map(|s| SymbolSearchResult {
                symbol: s,
                score: 0.0,
                snippet: None,
            })
            .collect());
    }

    let execute_search = |match_clause: &str| -> Result<Vec<SymbolSearchResult>, QueryError> {
        let mut sql = String::from(
            "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                    s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
                    s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
                    s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
                    s.is_test, s.test_container,
                    bm25(symbols_fts, 10.0, 5.0, 1.0) AS rank_score,
                    snippet(symbols_fts, 2, '[', ']', '...', 12) AS doc_snippet,
                    snippet(symbols_fts, 1, '[', ']', '...', 12) AS sig_snippet,
                    snippet(symbols_fts, 0, '[', ']', '...', 12) AS name_snippet
             FROM symbols_fts
             JOIN symbols s ON s.rowid = symbols_fts.rowid
             WHERE symbols_fts MATCH ?1",
        );

        if !include_tests {
            sql.push_str(" AND s.is_test = 0 AND s.test_container = 0");
        }

        if kind_filter.is_some() {
            sql.push_str(" AND s.kind = ?2");
        }

        sql.push_str(" ORDER BY rank_score ASC LIMIT ");
        sql.push_str(&limit.to_string());

        let mut stmt = conn.prepare(&sql)?;

        let map_fn = |row: &Row| -> rusqlite::Result<SymbolSearchResult> {
            let symbol = map_symbol(row)?;
            let score: f64 = row.get("rank_score")?;
            let doc_snip: Option<String> = row.get("doc_snippet").ok();
            let sig_snip: Option<String> = row.get("sig_snippet").ok();
            let name_snip: Option<String> = row.get("name_snippet").ok();

            // Pick the snippet containing match highlight brackets
            let snippet = if doc_snip.as_ref().map(|s| s.contains('[')).unwrap_or(false) {
                doc_snip
            } else if sig_snip.as_ref().map(|s| s.contains('[')).unwrap_or(false) {
                sig_snip
            } else if name_snip.as_ref().map(|s| s.contains('[')).unwrap_or(false) {
                name_snip
            } else {
                doc_snip.or(sig_snip).or(name_snip)
            };

            Ok(SymbolSearchResult {
                symbol,
                score,
                snippet,
            })
        };

        let rows = if let Some(k) = kind_filter {
            stmt.query_map(params![match_clause, k], map_fn)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            stmt.query_map(params![match_clause], map_fn)?
                .collect::<Result<Vec<_>, _>>()?
        };

        Ok(rows)
    };

    let mut results = execute_search(&and_q)?;
    if results.is_empty() && and_q != or_q {
        results = execute_search(&or_q)?;
    }

    Ok(results)
}


/// Find a specific symbol by name, with an optional path filter for disambiguation.
pub fn get_symbol_by_name(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> Result<Option<Symbol>, QueryError> {
    get_symbol_by_name_internal(conn, name, path_filter, false)
}

/// Find a specific symbol by name, requiring exact path match (used for atomic edits).
pub fn get_symbol_by_name_exact(
    conn: &Connection,
    name: &str,
    exact_path: &str,
) -> Result<Option<Symbol>, QueryError> {
    get_symbol_by_name_internal(conn, name, Some(exact_path), true)
}

fn get_symbol_by_name_internal(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
    exact_path: bool,
) -> Result<Option<Symbol>, QueryError> {
    // Check if name is qualified like `Struct::method` or `Class.method`
    let (parent_name, terminal_name) = if let Some(idx) = name.rfind("::") {
        let parent = &name[..idx];
        let term = &name[idx + 2..];
        let immediate_parent = if let Some(p_idx) = parent.rfind("::") {
            &parent[p_idx + 2..]
        } else {
            parent
        };
        (Some(immediate_parent), term)
    } else if let Some(idx) = name.rfind('.') {
        let parent = &name[..idx];
        let term = &name[idx + 1..];
        let immediate_parent = if let Some(p_idx) = parent.rfind('.') {
            &parent[p_idx + 1..]
        } else {
            parent
        };
        (Some(immediate_parent), term)
    } else {
        (None, name)
    };

    let sql = "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
                s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
                s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
                s.is_test, s.test_container
         FROM symbols s
         LEFT JOIN symbols p ON s.parent_symbol_id = p.symbol_id
         WHERE (s.name = :name OR (s.name = :term AND (:parent IS NULL OR p.name = :parent)))
           AND (:path IS NULL OR s.path = :path OR (:exact = 0 AND s.path LIKE '%/' || :path))
         ORDER BY (s.name = :name) DESC, s.is_test ASC LIMIT 10";

    let mut stmt = conn.prepare(sql)?;
    let normalized_path = path_filter.map(|p| p.replace('\\', "/"));

    let mut rows = stmt.query(rusqlite::named_params! {
        ":name": name,
        ":term": terminal_name,
        ":parent": parent_name,
        ":path": normalized_path.as_deref(),
        ":exact": if exact_path { 1 } else { 0 },
    })?;

    let mut matches: Vec<Symbol> = Vec::new();
    while let Some(row) = rows.next()? {
        matches.push(map_symbol(row)?);
    }

    if matches.is_empty() {
        return Ok(None);
    }

    if matches.len() == 1 {
        return Ok(Some(matches.remove(0)));
    }

    // Check if there's an exact match on full name
    let exact_name_matches: Vec<_> = matches.iter().filter(|s| s.name == name).cloned().collect();
    if exact_name_matches.len() == 1 {
        return Ok(Some(exact_name_matches.into_iter().next().unwrap()));
    }

    // If path_filter was given and there's an exact path match
    if let Some(ref p) = normalized_path {
        let exact_path_matches: Vec<_> = matches.iter().filter(|s| s.path == *p).cloned().collect();
        if exact_path_matches.len() == 1 {
            return Ok(Some(exact_path_matches.into_iter().next().unwrap()));
        }
    }

    // Ambiguity detected
    let mut candidate_list = String::new();
    for s in &matches {
        candidate_list.push_str(&format!("- {} `{}` in {}:{}\n", s.kind, s.name, s.path, s.start_line));
    }

    Err(QueryError::AmbiguousSymbol(name.to_string(), matches.len(), candidate_list))
}

/// Find callers or callees of a symbol.
pub fn find_references(
    conn: &Connection,
    symbol_name: &str,
    direction: &str,
    limit: usize,
) -> Result<Vec<ReferenceSite>, QueryError> {
    let mut results = Vec::new();

    if direction == "callers" {
        // Find callers: references pointing to target symbol
        let mut stmt = conn.prepare(
            "SELECT s_from.name AS from_name,
                    r.from_symbol_id,
                    s_to.name AS to_name,
                    r.kind,
                    r.path,
                    r.start_line,
                    r.start_column
             FROM relationships r
             JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
             JOIN symbols s_to ON r.to_symbol_id = s_to.symbol_id
             WHERE s_to.name = ?1
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![symbol_name, limit as i64], |row| {
            Ok(ReferenceSite {
                from_symbol_name: row.get(0)?,
                from_symbol_id: row.get(1)?,
                to_symbol_name: row.get(2)?,
                kind: row.get(3)?,
                path: row.get(4)?,
                start_line: row.get::<_, Option<i64>>(5)?.map(|v| v as usize),
                start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
            })
        })?;

        for r in rows {
            results.push(r?);
        }

        // Also query pending_relationships for callers
        if results.len() < limit {
            let remaining = limit - results.len();
            let mut pending_stmt = conn.prepare(
                "SELECT s_from.name AS from_name,
                        p.from_symbol_id,
                        p.target_terminal_name AS to_name,
                        p.kind,
                        p.path,
                        p.start_line,
                        p.start_column
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 WHERE p.target_terminal_name = ?1
                 LIMIT ?2",
            )?;

            let p_rows = pending_stmt.query_map(params![symbol_name, remaining as i64], |row| {
                Ok(ReferenceSite {
                    from_symbol_name: row.get(0)?,
                    from_symbol_id: row.get(1)?,
                    to_symbol_name: row.get(2)?,
                    kind: row.get(3)?,
                    path: row.get(4)?,
                    start_line: Some(row.get::<_, i64>(5)? as usize),
                    start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                })
            })?;

            for r in p_rows {
                results.push(r?);
            }
        }
    } else {
        // Find callees: symbols called by target symbol
        let mut stmt = conn.prepare(
            "SELECT s_from.name AS from_name,
                    r.from_symbol_id,
                    s_to.name AS to_name,
                    r.kind,
                    r.path,
                    r.start_line,
                    r.start_column
             FROM relationships r
             JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
             JOIN symbols s_to ON r.to_symbol_id = s_to.symbol_id
             WHERE s_from.name = ?1
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![symbol_name, limit as i64], |row| {
            Ok(ReferenceSite {
                from_symbol_name: row.get(0)?,
                from_symbol_id: row.get(1)?,
                to_symbol_name: row.get(2)?,
                kind: row.get(3)?,
                path: row.get(4)?,
                start_line: row.get::<_, Option<i64>>(5)?.map(|v| v as usize),
                start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
            })
        })?;

        for r in rows {
            results.push(r?);
        }

        // Also query pending_relationships for callees
        if results.len() < limit {
            let remaining = limit - results.len();
            let mut pending_stmt = conn.prepare(
                "SELECT s_from.name AS from_name,
                        p.from_symbol_id,
                        p.target_terminal_name AS to_name,
                        p.kind,
                        p.path,
                        p.start_line,
                        p.start_column
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 WHERE s_from.name = ?1
                 LIMIT ?2",
            )?;

            let p_rows = pending_stmt.query_map(params![symbol_name, remaining as i64], |row| {
                Ok(ReferenceSite {
                    from_symbol_name: row.get(0)?,
                    from_symbol_id: row.get(1)?,
                    to_symbol_name: row.get(2)?,
                    kind: row.get(3)?,
                    path: row.get(4)?,
                    start_line: Some(row.get::<_, i64>(5)? as usize),
                    start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                })
            })?;

            for r in p_rows {
                results.push(r?);
            }
        }
    }

    Ok(results)
}

/// Find structural facts by category (e.g. route, query, model, config).
pub fn find_structural_facts(
    conn: &Connection,
    category: &str,
    limit: usize,
) -> Result<Vec<StructuralFact>, QueryError> {
    let pattern = format!("%{category}%");
    let mut stmt = conn.prepare(
        "SELECT sf.structural_fact_id, sf.path, sf.language, sf.pattern_id,
                sf.capture_name, sf.node_kind, s.name AS containing_symbol_name,
                sf.start_line, sf.end_line, sf.confidence
         FROM structural_facts sf
         LEFT JOIN symbols s ON sf.containing_symbol_id = s.symbol_id
         WHERE sf.pattern_id LIKE ?1 OR sf.capture_name LIKE ?1 OR sf.node_kind LIKE ?1
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        Ok(StructuralFact {
            structural_fact_id: row.get(0)?,
            path: row.get(1)?,
            language: row.get(2)?,
            pattern_id: row.get(3)?,
            capture_name: row.get(4)?,
            node_kind: row.get(5)?,
            containing_symbol_name: row.get(6)?,
            start_line: row.get::<_, i64>(7)? as usize,
            end_line: row.get::<_, i64>(8)? as usize,
            confidence: row.get(9)?,
        })
    })?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}

/// Find literals (endpoints, SQL queries, configs) matching category.
pub fn find_literals(
    conn: &Connection,
    category: &str,
    limit: usize,
) -> Result<Vec<LiteralFact>, QueryError> {
    let pattern = format!("%{category}%");
    let mut stmt = conn.prepare(
        "SELECT l.literal_id, l.path, l.literal_text, l.kind, l.carrier,
                l.start_line, s.name AS containing_symbol_name
         FROM literals l
         LEFT JOIN symbols s ON l.containing_symbol_id = s.symbol_id
         WHERE l.kind LIKE ?1 OR l.literal_text LIKE ?1
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(params![pattern, limit as i64], |row| {
        Ok(LiteralFact {
            literal_id: row.get(0)?,
            path: row.get(1)?,
            literal_text: row.get(2)?,
            kind: row.get(3)?,
            carrier: row.get(4)?,
            start_line: row.get::<_, i64>(5)? as usize,
            containing_symbol_name: row.get(6)?,
        })
    })?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}

/// Find type facts for a symbol.
pub fn find_type_facts(
    conn: &Connection,
    symbol_id: &str,
) -> Result<Vec<TypeFact>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT type_fact_id, symbol_id, language, resolved_type, generic_params_json
         FROM type_facts
         WHERE symbol_id = ?1",
    )?;

    let rows = stmt.query_map(params![symbol_id], |row| {
        Ok(TypeFact {
            type_fact_id: row.get(0)?,
            symbol_id: row.get(1)?,
            language: row.get(2)?,
            resolved_type: row.get(3)?,
            generic_params: row.get(4)?,
        })
    })?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{ensure_fts_index, open_read_write};

    #[test]
    fn test_sanitize_fts5_query() {
        let (and_q, or_q) = sanitize_fts5_query("parse tokens");
        assert_eq!(and_q, "\"parse\"* \"tokens\"*");
        assert_eq!(or_q, "\"parse\"* OR \"tokens\"*");

        let (and_q, or_q) = sanitize_fts5_query("  Option<T>  ");
        assert_eq!(and_q, "\"Option\"* \"T\"*");
        assert_eq!(or_q, "\"Option\"* OR \"T\"*");

        let (and_q, or_q) = sanitize_fts5_query("   ");
        assert!(and_q.is_empty());
        assert!(or_q.is_empty());
    }

    #[test]
    fn test_fts_search_symbols_and_porter_stemming() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let conn = open_read_write(temp.path()).unwrap();

        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                file_id TEXT,
                path TEXT,
                language TEXT,
                name TEXT,
                kind TEXT,
                signature TEXT,
                doc_comment TEXT,
                visibility TEXT,
                parent_symbol_id TEXT,
                start_line INTEGER,
                start_column INTEGER,
                end_line INTEGER,
                end_column INTEGER,
                start_byte INTEGER,
                end_byte INTEGER,
                body_start_line INTEGER,
                body_start_column INTEGER,
                body_end_line INTEGER,
                body_end_column INTEGER,
                body_start_byte INTEGER,
                body_end_byte INTEGER,
                body_hash TEXT,
                semantic_group TEXT,
                is_test INTEGER,
                test_container INTEGER
            );
            INSERT INTO symbols VALUES (
                's1', 'f1', 'src/payment.rs', 'rust', 'PaymentGateway', 'trait',
                'pub trait PaymentGateway', 'Core payment provider interface for transactions',
                'pub', NULL, 10, 0, 20, 1, 100, 250, 12, 4, 19, 1, 120, 240, 'hash1', 'type', 0, 0
            );
            INSERT INTO symbols VALUES (
                's2', 'f1', 'src/payment.rs', 'rust', 'StripeClient', 'struct',
                'pub struct StripeClient', 'Handles HTTP requests to stripe payment API',
                'pub', NULL, 25, 0, 35, 1, 300, 450, 27, 4, 34, 1, 320, 440, 'hash2', 'type', 0, 0
            );
            INSERT INTO symbols VALUES (
                's3', 'f2', 'src/parser.rs', 'rust', 'parse_tokens', 'function',
                'pub fn parse_tokens(stream: &TokenStream) -> Result<Vec<Token>>', 'Parses syntax tokens from stream',
                'pub', NULL, 5, 0, 15, 1, 50, 200, 7, 4, 14, 1, 70, 190, 'hash3', 'function', 0, 0
            );
            INSERT INTO symbols VALUES (
                's4', 'f3', 'tests/payment_test.rs', 'rust', 'test_payment_flow', 'function',
                'fn test_payment_flow()', 'Tests payment charge workflow',
                NULL, NULL, 5, 0, 15, 1, 50, 200, 7, 4, 14, 1, 70, 190, 'hash4', 'function', 1, 0
            );",
        )
        .unwrap();

        ensure_fts_index(&conn).unwrap();

        // 1. Porter stemming match: 'parsing' matches 'parse_tokens' and 'Parses' docstring
        let results = fts_search_symbols(&conn, "parsing tokens", None, false, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol.name, "parse_tokens");
        assert!(results[0].snippet.is_some());

        // 2. Docstring conceptual search: 'transactions' matches 'PaymentGateway'
        let results = fts_search_symbols(&conn, "transactions", None, false, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol.name, "PaymentGateway");

        // 3. Test filter: searching 'payment' with include_tests=false ignores 'test_payment_flow'
        let results = fts_search_symbols(&conn, "payment", None, false, 10).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| !r.symbol.is_test));

        // 4. Test filter: searching 'payment' with include_tests=true includes 'test_payment_flow'
        let results = fts_search_symbols(&conn, "payment", None, true, 10).unwrap();
        assert_eq!(results.len(), 3);

        // 5. Fallback OR matching: multi-term where only some match
        let results = fts_search_symbols(&conn, "stripe kafka redis", None, false, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol.name, "StripeClient");
    }
}

