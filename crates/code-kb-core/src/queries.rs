use rusqlite::{Connection, Row, params};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

use crate::models::{
    BlastRadiusResult, FileFact, ImpactedSymbol, LiteralFact, ReferenceSite, StructuralFact,
    Symbol, SymbolSearchResult, TestTarget, TypeFact,
};

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("Database query error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Symbol '{0}' not found")]
    SymbolNotFound(String),
    #[error("Symbol '{0}' not found. Did you mean one of:\n{1}")]
    SymbolNotFoundWithSuggestions(String, String),
    #[error(
        "Ambiguous symbol '{0}': found {1} matching candidates. Specify file_path or qualified name to disambiguate:\n{2}"
    )]
    AmbiguousSymbol(String, usize, String),
    #[error("Invalid direction '{0}': must be 'callers' or 'callees'")]
    InvalidDirection(String),
}

fn map_symbol(row: &Row) -> rusqlite::Result<Symbol> {
    Ok(Symbol {
        symbol_id: row.get("symbol_id")?,
        file_id: row.get("file_id")?,
        path: row.get::<_, String>("path")?.replace('\\', "/"),
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
        body_start_line: row
            .get::<_, Option<i64>>("body_start_line")?
            .map(|v| v as usize),
        body_start_column: row
            .get::<_, Option<i64>>("body_start_column")?
            .map(|v| v as usize),
        body_end_line: row
            .get::<_, Option<i64>>("body_end_line")?
            .map(|v| v as usize),
        body_end_column: row
            .get::<_, Option<i64>>("body_end_column")?
            .map(|v| v as usize),
        body_start_byte: row
            .get::<_, Option<i64>>("body_start_byte")?
            .map(|v| v as usize),
        body_end_byte: row
            .get::<_, Option<i64>>("body_end_byte")?
            .map(|v| v as usize),
        body_hash: row.get("body_hash")?,
        semantic_group: row.get("semantic_group")?,
        is_test: row.get::<_, i64>("is_test")? != 0,
        test_container: row.get::<_, i64>("test_container")? != 0,
    })
}

pub(crate) fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Retrieve indexed files optionally scoped by path filter, pushed down to SQLite.
pub fn load_scoped_files(
    conn: &Connection,
    path_filter: Option<&str>,
) -> Result<Vec<FileFact>, QueryError> {
    let norm = path_filter
        .map(|p| p.replace('\\', "/").trim_matches('/').to_string())
        .filter(|p| !p.is_empty());
    let norm_bs = norm.as_ref().map(|p| p.replace('/', "\\"));
    let prefix = norm.as_ref().map(|path| format!("{}/%", escape_like(path)));
    let prefix_bs = norm_bs
        .as_ref()
        .map(|path| format!("{}\\\\%", escape_like(path)));

    let sql = "SELECT file_id, path, language, content_hash, content_bytes, line_count, indexed_at
               FROM files
               WHERE (:path IS NULL
                  OR path = :path COLLATE NOCASE
                  OR path = :path_bs COLLATE NOCASE
                  OR path LIKE :path_prefix ESCAPE '\\'
                  OR path LIKE :path_prefix_bs ESCAPE '\\')
               ORDER BY (:path IS NOT NULL AND (path = :path OR path = :path_bs)) DESC, path ASC";

    let mut stmt = conn.prepare(sql)?;
    let files = stmt
        .query_map(
            rusqlite::named_params! {
                ":path": norm.as_deref(),
                ":path_bs": norm_bs.as_deref(),
                ":path_prefix": prefix.as_deref(),
                ":path_prefix_bs": prefix_bs.as_deref(),
            },
            |row| {
                Ok(FileFact {
                    file_id: row.get(0)?,
                    path: row.get::<_, String>(1)?.replace('\\', "/"),
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
    let norm = path_filter
        .map(|p| p.replace('\\', "/").trim_matches('/').to_string())
        .filter(|p| !p.is_empty());
    let norm_bs = norm.as_ref().map(|p| p.replace('/', "\\"));
    let prefix = norm.as_ref().map(|path| format!("{}/%", escape_like(path)));
    let prefix_bs = norm_bs
        .as_ref()
        .map(|path| format!("{}\\\\%", escape_like(path)));

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
        WITH bounded_files AS (
            SELECT path FROM files
            WHERE (:path IS NULL
               OR path = :path COLLATE NOCASE
               OR path = :path_bs COLLATE NOCASE
               OR path LIKE :path_prefix ESCAPE '\\'
               OR path LIKE :path_prefix_bs ESCAPE '\\')
            ORDER BY path ASC
            LIMIT 1000
        ),
        ranked AS (
            SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                   s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
                   s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
                   s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
                   s.is_test, s.test_container,
                   ROW_NUMBER() OVER (PARTITION BY s.path ORDER BY s.start_line ASC) as rn
            FROM symbols s
            JOIN bounded_files bf ON (s.path = bf.path COLLATE NOCASE OR replace(s.path, '\\', '/') = replace(bf.path, '\\', '/') COLLATE NOCASE)
            WHERE (length(s.path) - length(replace(replace(s.path, '/', ''), '\\', '')) <= :max_slashes)
              AND s.kind IN ('function', 'method', 'struct', 'enum', 'trait', 'class', 'interface', 'type')
              AND s.parent_symbol_id IS NULL
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
        ":path_bs": norm_bs.as_deref(),
        ":path_prefix": prefix.as_deref(),
        ":path_prefix_bs": prefix_bs.as_deref(),
        ":max_slashes": max_slashes,
        ":limit": limit_per_file as i64,
    })?;

    let mut symbols_by_file: HashMap<String, Vec<Symbol>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let sym = map_symbol(row)?;
        symbols_by_file
            .entry(sym.path.clone())
            .or_default()
            .push(sym);
    }

    Ok(symbols_by_file)
}

/// Lookup single file metadata by path with slash-boundary matching.
pub fn get_file(conn: &Connection, path: &str) -> Result<Option<FileFact>, QueryError> {
    let normalized = path.replace('\\', "/");
    let backslash = path.replace('/', "\\");

    // Check exact path match first, prioritizing exact case before case-insensitive fallback
    let mut stmt = conn.prepare(
        "SELECT file_id, path, language, content_hash, content_bytes, line_count, indexed_at
         FROM files
         WHERE (path = ?1 COLLATE NOCASE OR path = ?2 COLLATE NOCASE)
         ORDER BY (path = ?1 OR path = ?2) DESC
         LIMIT 1",
    )?;

    let mut rows = stmt.query(params![normalized, backslash])?;
    if let Some(row) = rows.next()? {
        Ok(Some(FileFact {
            file_id: row.get(0)?,
            path: row.get::<_, String>(1)?.replace('\\', "/"),
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
    // Normalizing slashes for path matching
    let normalized = file_path.replace('\\', "/");
    let backslash = file_path.replace('/', "\\");

    // Try exact case matching first to avoid conflating sibling files on case-sensitive filesystems
    let mut stmt = conn.prepare(
        "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
         FROM symbols
         WHERE (path = ?1 OR path = ?2)
         ORDER BY start_line ASC, start_column ASC",
    )?;

    let rows = stmt
        .query_map(params![&normalized, &backslash], map_symbol)?
        .collect::<Result<Vec<_>, _>>()?;

    if !rows.is_empty() {
        return Ok(rows);
    }

    // Fall back to case-insensitive match (for Windows or case-variant requests)
    let mut stmt = conn.prepare(
        "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
         FROM symbols
         WHERE (path = ?1 COLLATE NOCASE OR path = ?2 COLLATE NOCASE)
         ORDER BY start_line ASC, start_column ASC",
    )?;

    let rows = stmt
        .query_map(params![normalized, backslash], map_symbol)?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(rows)
}

/// Normalizes common symbol kind aliases to their canonical database representation.
pub fn normalize_kind(kind: &str) -> String {
    let lower = kind.trim().to_lowercase();
    match lower.as_str() {
        "fn" | "func" | "function" => "function".to_string(),
        "method" => "method".to_string(),
        "struct" => "struct".to_string(),
        "class" => "class".to_string(),
        "enum" => "enum".to_string(),
        "trait" => "trait".to_string(),
        "interface" => "interface".to_string(),
        "type" | "typedef" => "type".to_string(),
        "mod" | "module" => "module".to_string(),
        "const" | "constant" => "constant".to_string(),
        "var" | "variable" => "variable".to_string(),
        _ => lower,
    }
}

/// Search symbols by name query, kind filter, and test flag.
pub fn search_symbols(
    conn: &Connection,
    query: &str,
    kind_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<Symbol>, QueryError> {
    search_symbols_scoped(conn, query, kind_filter, None, include_tests, limit)
}

/// Search symbols with optional path scoping filter.
pub fn search_symbols_scoped(
    conn: &Connection,
    query: &str,
    kind_filter: Option<&str>,
    path_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<Symbol>, QueryError> {
    // Try get_symbol_by_name first for qualified queries (e.g. McpServer::new, Class.method)
    if (query.contains("::") || query.contains('.'))
        && let Ok(Some(sym)) = get_symbol_by_name(conn, query, path_filter)
    {
        return Ok(vec![sym]);
    }

    let pattern = format!("%{}%", escape_like(query));
    let normalized_path = path_filter.map(|p| {
        p.replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    });
    let escaped_path = normalized_path.as_deref().map(escape_like);
    let norm_kind = kind_filter.map(normalize_kind);

    let mut sql = String::from(
        "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
         FROM symbols
         WHERE (name = :query OR name LIKE :pattern ESCAPE '\\')
           AND (:kind IS NULL OR kind = :kind)
           AND (:path IS NULL OR replace(path, '\\', '/') = :path COLLATE NOCASE OR replace(path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
    );

    if !include_tests {
        sql.push_str(" AND is_test = 0 AND test_container = 0");
    }

    sql.push_str(
        " ORDER BY (name = :query) DESC, (kind IN ('function', 'struct', 'class', 'trait', 'method', 'enum', 'interface', 'type')) DESC, length(name) ASC, path ASC LIMIT ",
    );
    sql.push_str(&limit.to_string());

    let mut stmt = conn.prepare(&sql)?;

    let path_val = normalized_path.as_deref();
    let path_like = escaped_path.as_deref();
    let kind_val = norm_kind.as_deref();
    let rows = stmt
        .query_map(
            rusqlite::named_params! {
                ":query": query,
                ":pattern": pattern,
                ":kind": kind_val,
                ":path": path_val,
                ":path_like": path_like,
            },
            map_symbol,
        )?
        .collect::<Result<Vec<_>, _>>()?;

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

/// Conceptual full-text search with optional path scoping filter.
pub fn fts_search_symbols_scoped(
    conn: &Connection,
    query: &str,
    kind_filter: Option<&str>,
    path_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<SymbolSearchResult>, QueryError> {
    let (and_q, or_q) = sanitize_fts5_query(query);
    if and_q.is_empty() {
        return Ok(Vec::new());
    }

    let normalized_path = path_filter.map(|p| {
        p.replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    });
    let norm_kind = kind_filter.map(normalize_kind);

    let fts_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='symbols_fts'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if !fts_exists {
        let pattern = format!("%{}%", escape_like(query));
        let escaped_path = normalized_path.as_deref().map(escape_like);
        let mut sql = String::from(
            "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                    visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                    start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                    body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                    is_test, test_container
              FROM symbols
              WHERE (name = :query OR name LIKE :pattern ESCAPE '\\')
                AND (:kind IS NULL OR kind = :kind)
                AND (:path IS NULL OR replace(path, '\\', '/') = :path COLLATE NOCASE OR replace(path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
        );
        if !include_tests {
            sql.push_str(" AND is_test = 0 AND test_container = 0");
        }
        sql.push_str(
            " ORDER BY (name = :query) DESC, (kind IN ('function', 'struct', 'class', 'trait', 'method', 'enum', 'interface', 'type')) DESC, length(name) ASC, path ASC LIMIT ",
        );
        sql.push_str(&limit.to_string());

        let mut stmt = conn.prepare(&sql)?;
        let path_val = normalized_path.as_deref();
        let path_like = escaped_path.as_deref();
        let kind_val = norm_kind.as_deref();
        let rows = stmt
            .query_map(
                rusqlite::named_params! {
                    ":query": query,
                    ":pattern": pattern,
                    ":kind": kind_val,
                    ":path": path_val,
                    ":path_like": path_like,
                },
                map_symbol,
            )?
            .collect::<Result<Vec<_>, _>>()?;

        return Ok(rows
            .into_iter()
            .map(|s| SymbolSearchResult {
                symbol: s,
                score: 0.0,
                snippet: None,
            })
            .collect());
    }

    let escaped_path = normalized_path.as_deref().map(escape_like);
    let execute_search = |match_clause: &str| -> Result<Vec<SymbolSearchResult>, QueryError> {
        let mut sql = String::from(
            "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                    s.visibility, s.parent_symbol_id, s.start_line, s.start_column, end_line, end_column,
                    start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                    body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                    is_test, test_container,
                    bm25(symbols_fts, 10.0, 5.0, 1.0) AS rank_score,
                    snippet(symbols_fts, 2, '[', ']', '...', 12) AS doc_snippet,
                    snippet(symbols_fts, 1, '[', ']', '...', 12) AS sig_snippet,
                    snippet(symbols_fts, 0, '[', ']', '...', 12) AS name_snippet
             FROM symbols_fts
             CROSS JOIN symbols s ON s.rowid = symbols_fts.rowid
             WHERE symbols_fts MATCH :match
               AND (:kind IS NULL OR s.kind = :kind)
               AND (:path IS NULL OR replace(s.path, '\\', '/') = :path COLLATE NOCASE OR replace(s.path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(s.path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
        );

        if !include_tests {
            sql.push_str(" AND s.is_test = 0 AND s.test_container = 0");
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

        let path_val = normalized_path.as_deref();
        let path_like = escaped_path.as_deref();
        let kind_val = norm_kind.as_deref();
        let rows = stmt
            .query_map(
                rusqlite::named_params! {
                    ":match": match_clause,
                    ":kind": kind_val,
                    ":path": path_val,
                    ":path_like": path_like,
                },
                map_fn,
            )?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    };

    let mut results = execute_search(&and_q)?;
    if results.is_empty() && and_q != or_q {
        results = execute_search(&or_q)?;
    }

    Ok(results)
}

/// Find tests related to a target symbol by caller relationships, naming pattern, or FTS matching.
pub fn find_related_tests(
    conn: &Connection,
    target_symbol: &Symbol,
    limit: usize,
) -> Result<Vec<Symbol>, QueryError> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let mut tests = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    // 1. Direct callers / references that are marked as test or located in test files
    let callers_sql = "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
            s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
            s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
            s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
            s.is_test, s.test_container
     FROM symbols s
     JOIN relationships r ON r.from_symbol_id = s.symbol_id
     WHERE r.to_symbol_id = ?1 AND (s.is_test = 1 OR s.test_container = 1)
     LIMIT ?2";

    if let Ok(mut stmt) = conn.prepare(callers_sql)
        && let Ok(rows) = stmt.query_map(params![target_symbol.symbol_id, limit as i64], map_symbol)
    {
        for row in rows.flatten() {
            if seen_ids.insert(row.symbol_id.clone()) {
                tests.push(row);
                if tests.len() >= limit {
                    return Ok(tests);
                }
            }
        }
    }

    // 2. Name-matching tests in SQLite
    let remaining = limit - tests.len();
    let name_sql = "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
            s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
            s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
            s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
            s.is_test, s.test_container
     FROM symbols s
     WHERE (s.is_test = 1 OR s.test_container = 1)
       AND (s.name LIKE '%' || ?1 || '%' OR s.signature LIKE '%' || ?1 || '%')
     ORDER BY (s.name LIKE '%' || ?1 || '%') DESC
     LIMIT ?2";

    if let Ok(mut stmt) = conn.prepare(name_sql)
        && let Ok(rows) = stmt.query_map(
            params![target_symbol.name, (remaining * 2) as i64],
            map_symbol,
        )
    {
        for row in rows.flatten() {
            if seen_ids.insert(row.symbol_id.clone()) {
                tests.push(row);
                if tests.len() >= limit {
                    return Ok(tests);
                }
            }
        }
    }

    // 3. FTS5 search restricted to tests
    let remaining = limit - tests.len();
    let fts_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='symbols_fts'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if remaining > 0 && fts_exists {
        let fts_sql = "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
                s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
                s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
                s.is_test, s.test_container
         FROM symbols_fts
         CROSS JOIN symbols s ON s.rowid = symbols_fts.rowid
         WHERE symbols_fts MATCH ?1 AND (s.is_test = 1 OR s.test_container = 1)
         LIMIT ?2";

        let (and_q, _or_q) = sanitize_fts5_query(&target_symbol.name);
        if !and_q.is_empty()
            && let Ok(mut stmt) = conn.prepare(fts_sql)
            && let Ok(rows) = stmt.query_map(params![and_q, (remaining * 2) as i64], map_symbol)
        {
            for row in rows.flatten() {
                if seen_ids.insert(row.symbol_id.clone()) {
                    tests.push(row);
                    if tests.len() >= limit {
                        break;
                    }
                }
            }
        }
    }

    Ok(tests)
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
           AND (:path IS NULL OR s.path = :path COLLATE NOCASE OR s.path = :path_bs COLLATE NOCASE OR (:exact = 0 AND (s.path LIKE '%/' || :path_like ESCAPE '\\' OR s.path LIKE '%\\\\' || :path_like_bs ESCAPE '\\')))
         ORDER BY (s.kind != 'import') DESC,
                  (s.kind IN ('function', 'struct', 'class', 'trait', 'method', 'enum', 'interface', 'type')) DESC,
                  (s.name = :name) DESC,
                  (:path IS NOT NULL AND (s.path = :path COLLATE NOCASE OR s.path = :path_bs COLLATE NOCASE)) DESC,
                  s.is_test ASC
         LIMIT 25";

    let mut stmt = conn.prepare(sql)?;
    let normalized_path = path_filter.map(|p| p.replace('\\', "/").trim_matches('/').to_string());
    let backslash_path = normalized_path.as_deref().map(|p| p.replace('/', "\\"));
    let path_like = normalized_path.as_deref().map(escape_like);
    let path_like_bs = backslash_path.as_deref().map(escape_like);

    let mut rows = stmt.query(rusqlite::named_params! {
        ":name": name,
        ":term": terminal_name,
        ":parent": parent_name,
        ":path": normalized_path.as_deref(),
        ":path_bs": backslash_path.as_deref(),
        ":path_like": path_like.as_deref(),
        ":path_like_bs": path_like_bs.as_deref(),
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

    // Exclude imports if non-import candidates exist
    let candidates: Vec<Symbol> = if matches.iter().any(|s| s.kind != "import") {
        matches.into_iter().filter(|s| s.kind != "import").collect()
    } else {
        matches
    };

    if candidates.len() == 1 {
        return Ok(Some(candidates.into_iter().next().unwrap()));
    }

    // Check if there's an exact match on full name among candidates
    let exact_name_matches: Vec<_> = candidates
        .iter()
        .filter(|s| s.name == name)
        .cloned()
        .collect();
    if exact_name_matches.len() == 1 {
        return Ok(Some(exact_name_matches.into_iter().next().unwrap()));
    }

    let definition_candidates = if exact_name_matches.is_empty() {
        &candidates
    } else {
        &exact_name_matches
    };
    let def_matches: Vec<_> = definition_candidates
        .iter()
        .filter(|s| {
            matches!(
                s.kind.as_str(),
                "function"
                    | "struct"
                    | "class"
                    | "trait"
                    | "method"
                    | "enum"
                    | "interface"
                    | "type"
            )
        })
        .cloned()
        .collect();
    if def_matches.len() == 1 {
        return Ok(Some(def_matches.into_iter().next().unwrap()));
    }

    let active_pool = if !def_matches.is_empty() {
        def_matches
    } else if !exact_name_matches.is_empty() {
        exact_name_matches
    } else {
        candidates
    };

    // If path_filter was given and there's an exact path match
    if let Some(ref p) = normalized_path {
        let exact_path_matches: Vec<_> = active_pool
            .iter()
            .filter(|s| s.path == *p)
            .cloned()
            .collect();
        if exact_path_matches.len() == 1 {
            return Ok(Some(exact_path_matches.into_iter().next().unwrap()));
        }
    }

    if active_pool.len() == 1 {
        return Ok(Some(active_pool.into_iter().next().unwrap()));
    }

    // Ambiguity detected
    let mut candidate_list = String::new();
    for s in &active_pool {
        candidate_list.push_str(&format!(
            "- {} `{}` in {}:{}\n",
            s.kind, s.name, s.path, s.start_line
        ));
    }

    Err(QueryError::AmbiguousSymbol(
        name.to_string(),
        active_pool.len(),
        candidate_list,
    ))
}

/// Find callers or callees of a symbol (filters unresolved external stdlib/runtime primitives by default).
pub fn find_references(
    conn: &Connection,
    symbol_name: &str,
    direction: &str,
    limit: usize,
) -> Result<Vec<ReferenceSite>, QueryError> {
    find_references_ext(conn, symbol_name, direction, limit, false)
}

/// Find callers or callees with option to include external runtime/stdlib primitives.
pub fn find_references_ext(
    conn: &Connection,
    symbol_name: &str,
    direction: &str,
    limit: usize,
    include_external: bool,
) -> Result<Vec<ReferenceSite>, QueryError> {
    find_references_scoped(conn, symbol_name, direction, limit, include_external, None)
}

/// Find callers or callees with optional file path disambiguation filter and external symbols toggle.
pub fn find_references_scoped(
    conn: &Connection,
    symbol_name: &str,
    direction: &str,
    limit: usize,
    include_external: bool,
    path_filter: Option<&str>,
) -> Result<Vec<ReferenceSite>, QueryError> {
    if direction != "callers" && direction != "callees" {
        return Err(QueryError::InvalidDirection(direction.to_string()));
    }

    match get_symbol_by_name(conn, symbol_name, path_filter)? {
        Some(target) => find_references_internal(
            conn,
            &target.name,
            direction,
            limit,
            Some(&target.symbol_id),
            include_external,
        ),
        None => {
            let suggestions = search_symbols_scoped(conn, symbol_name, None, path_filter, false, 3)
                .unwrap_or_default();
            if suggestions.is_empty() {
                Err(QueryError::SymbolNotFound(symbol_name.to_string()))
            } else {
                let list = suggestions
                    .into_iter()
                    .map(|s| format!("  - {} `{}` ({}:{})", s.kind, s.name, s.path, s.start_line))
                    .collect::<Vec<_>>()
                    .join("\n");
                Err(QueryError::SymbolNotFoundWithSuggestions(
                    symbol_name.to_string(),
                    list,
                ))
            }
        }
    }
}

pub fn find_references_for_symbol(
    conn: &Connection,
    symbol_name: &str,
    direction: &str,
    limit: usize,
    symbol_id: &str,
) -> Result<Vec<ReferenceSite>, QueryError> {
    find_references_internal(conn, symbol_name, direction, limit, Some(symbol_id), false)
}

fn has_pending_namespace_column(conn: &Connection) -> bool {
    let has_ns: bool = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('pending_relationships') WHERE name = 'target_namespace_json'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    let has_display: bool = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('pending_relationships') WHERE name = 'target_display_name'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    let has_receiver: bool = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('pending_relationships') WHERE name = 'target_receiver'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    has_ns && has_display && has_receiver
}

fn find_references_internal(
    conn: &Connection,
    symbol_name: &str,
    direction: &str,
    limit: usize,
    symbol_id: Option<&str>,
    include_external: bool,
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
             WHERE s_to.name = ?1 AND (?3 IS NULL OR r.to_symbol_id = ?3)
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![symbol_name, limit as i64, symbol_id], |row| {
            Ok(ReferenceSite {
                from_symbol_name: row.get(0)?,
                from_symbol_id: row.get(1)?,
                to_symbol_name: row.get(2)?,
                kind: row.get(3)?,
                path: row.get::<_, String>(4)?.replace('\\', "/"),
                start_line: row.get::<_, Option<i64>>(5)?.map(|v| v as usize),
                start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
            })
        })?;

        for r in rows {
            results.push(r?);
        }

        // Also query pending_relationships for callers if results < limit
        if results.len() < limit {
            let remaining = limit - results.len();
            if has_pending_namespace_column(conn) {
                if let Some(sid) = symbol_id {
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
                         JOIN symbols s_target ON s_target.symbol_id = ?3
                         LEFT JOIN symbols s_target_parent ON s_target.parent_symbol_id = s_target_parent.symbol_id
                          WHERE p.target_terminal_name = ?1
                            AND (
                                (
                                    s_target.parent_symbol_id IS NOT NULL
                                    AND s_target_parent.name IS NOT NULL
                                    AND (
                                        EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = s_target_parent.name)
                                        OR (EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = 'Self')
                                            AND s_from.parent_symbol_id = s_target.parent_symbol_id)
                                        OR (p.target_receiver IS NOT NULL AND p.target_receiver != '' AND s_target_parent.name = p.target_receiver)
                                    )
                                    AND NOT EXISTS (
                                        SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                                        WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super', 'self', 'Self', s_target_parent.name)
                                          AND ('/' || replace(s_target.path, '\\', '/')) NOT LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                                          AND ('/' || replace(s_target.path, '\\', '/')) NOT LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '/%' ESCAPE '\\'
                                    )
                                )
                                OR (
                                    (p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')
                                    AND (p.target_receiver IS NULL OR p.target_receiver = '')
                                    AND (s_target.parent_symbol_id IS NULL OR s_from.parent_symbol_id = s_target.parent_symbol_id)
                                )
                                OR (
                                    s_target.parent_symbol_id IS NULL
                                    AND EXISTS (
                                        SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                                        WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super')
                                          AND ('/' || replace(s_target.path, '\\', '/')) LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                                    )
                                )
                            )
                          LIMIT ?2",
                    )?;

                    let p_rows = pending_stmt.query_map(
                        params![symbol_name, remaining as i64, sid],
                        |row| {
                            Ok(ReferenceSite {
                                from_symbol_name: row.get(0)?,
                                from_symbol_id: row.get(1)?,
                                to_symbol_name: row.get(2)?,
                                kind: row.get(3)?,
                                path: row.get::<_, String>(4)?.replace('\\', "/"),
                                start_line: Some(row.get::<_, i64>(5)? as usize),
                                start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                            })
                        },
                    )?;
                    for r in p_rows {
                        results.push(r?);
                    }
                } else {
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
                           AND (
                               (p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')
                               OR EXISTS (
                                   SELECT 1 FROM symbols s_any
                                   JOIN symbols s_any_parent ON s_any.parent_symbol_id = s_any_parent.symbol_id
                                   WHERE s_any.name = p.target_terminal_name
                                     AND EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = s_any_parent.name)
                               )
                           )
                         LIMIT ?2",
                    )?;

                    let p_rows =
                        pending_stmt.query_map(params![symbol_name, remaining as i64], |row| {
                            Ok(ReferenceSite {
                                from_symbol_name: row.get(0)?,
                                from_symbol_id: row.get(1)?,
                                to_symbol_name: row.get(2)?,
                                kind: row.get(3)?,
                                path: row.get::<_, String>(4)?.replace('\\', "/"),
                                start_line: Some(row.get::<_, i64>(5)? as usize),
                                start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                            })
                        })?;
                    for r in p_rows {
                        results.push(r?);
                    }
                }
            } else {
                let is_nested = if let Some(sid) = symbol_id {
                    conn.query_row(
                        "SELECT 1 FROM symbols WHERE symbol_id = ?1 AND parent_symbol_id IS NOT NULL",
                        params![sid],
                        |_| Ok(true),
                    )
                    .unwrap_or(false)
                } else {
                    false
                };

                if !is_nested {
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

                    let p_rows =
                        pending_stmt.query_map(params![symbol_name, remaining as i64], |row| {
                            Ok(ReferenceSite {
                                from_symbol_name: row.get(0)?,
                                from_symbol_id: row.get(1)?,
                                to_symbol_name: row.get(2)?,
                                kind: row.get(3)?,
                                path: row.get::<_, String>(4)?.replace('\\', "/"),
                                start_line: Some(row.get::<_, i64>(5)? as usize),
                                start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                            })
                        })?;

                    for r in p_rows {
                        results.push(r?);
                    }
                }
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
             WHERE s_from.name = ?1 AND (?3 IS NULL OR r.from_symbol_id = ?3)
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![symbol_name, limit as i64, symbol_id], |row| {
            Ok(ReferenceSite {
                from_symbol_name: row.get(0)?,
                from_symbol_id: row.get(1)?,
                to_symbol_name: row.get(2)?,
                kind: row.get(3)?,
                path: row.get::<_, String>(4)?.replace('\\', "/"),
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
            let p_rows: Vec<ReferenceSite> = if has_pending_namespace_column(conn) {
                let sql = if include_external {
                    "SELECT DISTINCT s_from.name AS from_name,
                            p.from_symbol_id,
                            COALESCE(NULLIF(p.target_display_name, ''), p.target_terminal_name) AS to_name,
                            p.kind,
                            p.path,
                            p.start_line,
                            p.start_column
                     FROM pending_relationships p
                     JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                     WHERE s_from.name = ?1 AND (?3 IS NULL OR p.from_symbol_id = ?3)
                     LIMIT ?2"
                } else {
                    "SELECT DISTINCT s_from.name AS from_name,
                            p.from_symbol_id,
                            p.target_terminal_name AS to_name,
                            p.kind,
                            p.path,
                            p.start_line,
                            p.start_column
                     FROM pending_relationships p
                     JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                     WHERE s_from.name = ?1 AND (?3 IS NULL OR p.from_symbol_id = ?3)
                       AND EXISTS (
                           SELECT 1 FROM symbols s_to
                           LEFT JOIN symbols s_to_parent ON s_to.parent_symbol_id = s_to_parent.symbol_id
                           WHERE s_to.name = p.target_terminal_name
                             AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                             AND (
                                 (
                                  (
                                      s_to.parent_symbol_id IS NOT NULL
                                      AND s_to_parent.name IS NOT NULL
                                      AND (
                                          EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = s_to_parent.name)
                                          OR (EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = 'Self')
                                              AND s_from.parent_symbol_id = s_to.parent_symbol_id)
                                          OR (p.target_receiver IS NOT NULL AND p.target_receiver != '' AND s_to_parent.name = p.target_receiver)
                                      )
                                      AND NOT EXISTS (
                                          SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                                          WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super', 'self', 'Self', s_to_parent.name)
                                            AND ('/' || replace(s_to.path, '\\', '/')) NOT LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                                            AND ('/' || replace(s_to.path, '\\', '/')) NOT LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '/%' ESCAPE '\\'
                                      )
                                  )
                                  OR (
                                      (p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')
                                      AND (p.target_receiver IS NULL OR p.target_receiver = '')
                                      AND (s_to.parent_symbol_id IS NULL OR s_from.parent_symbol_id = s_to.parent_symbol_id)
                                  )
                                  OR (
                                      s_to.parent_symbol_id IS NULL
                                      AND EXISTS (
                                          SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                                          WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super')
                                            AND ('/' || replace(s_to.path, '\\', '/')) LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                                      )
                                  )
                                 )
                             )
                       )
                     LIMIT ?2"
                };
                let mut pending_stmt = conn.prepare(sql)?;
                let rows = pending_stmt.query_map(
                    params![symbol_name, remaining as i64, symbol_id],
                    |row| {
                        Ok(ReferenceSite {
                            from_symbol_name: row.get(0)?,
                            from_symbol_id: row.get(1)?,
                            to_symbol_name: row.get(2)?,
                            kind: row.get(3)?,
                            path: row.get::<_, String>(4)?.replace('\\', "/"),
                            start_line: Some(row.get::<_, i64>(5)? as usize),
                            start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                        })
                    },
                )?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                out
            } else {
                let sql = if include_external {
                    "SELECT DISTINCT s_from.name AS from_name,
                            p.from_symbol_id,
                            p.target_terminal_name AS to_name,
                            p.kind,
                            p.path,
                            p.start_line,
                            p.start_column
                     FROM pending_relationships p
                     JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                     WHERE s_from.name = ?1 AND (?3 IS NULL OR p.from_symbol_id = ?3)
                     LIMIT ?2"
                } else {
                    "SELECT DISTINCT s_from.name AS from_name,
                            p.from_symbol_id,
                            p.target_terminal_name AS to_name,
                            p.kind,
                            p.path,
                            p.start_line,
                            p.start_column
                     FROM pending_relationships p
                     JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                     WHERE s_from.name = ?1 AND (?3 IS NULL OR p.from_symbol_id = ?3)
                       AND EXISTS (SELECT 1 FROM symbols s_to WHERE s_to.name = p.target_terminal_name)
                     LIMIT ?2"
                };
                let mut pending_stmt = conn.prepare(sql)?;

                let rows = pending_stmt.query_map(
                    params![symbol_name, remaining as i64, symbol_id],
                    |row| {
                        Ok(ReferenceSite {
                            from_symbol_name: row.get(0)?,
                            from_symbol_id: row.get(1)?,
                            to_symbol_name: row.get(2)?,
                            kind: row.get(3)?,
                            path: row.get::<_, String>(4)?.replace('\\', "/"),
                            start_line: Some(row.get::<_, i64>(5)? as usize),
                            start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                        })
                    },
                )?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                out
            };

            for r in p_rows {
                results.push(r);
            }
        }
    }

    Ok(results)
}

/// Resolve callee signatures directly in a single joined query, avoiding N+1 queries
/// and preserving ambiguous methods across types. Prioritizes functions/methods over enum variants.
pub fn find_callee_signatures(
    conn: &Connection,
    symbol_name: &str,
    symbol_id: &str,
    limit: usize,
    include_external: bool,
) -> Result<Vec<String>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT s_to.name, s_to.signature, s_to.path, s_to.start_line, s_to.kind
         FROM relationships r
         JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
         JOIN symbols s_to ON r.to_symbol_id = s_to.symbol_id
         WHERE s_from.name = ?1 AND r.from_symbol_id = ?2
         LIMIT ?3",
    )?;

    let rows = stmt.query_map(params![symbol_name, symbol_id, (limit * 2) as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?.replace('\\', "/"),
            row.get::<_, Option<i64>>(3)?.unwrap_or(1) as usize,
            row.get::<_, String>(4)?,
        ))
    })?;

    let mut signatures = Vec::new();
    let mut variants = Vec::new();

    for r in rows.flatten() {
        let (name, sig_opt, path, line, kind) = r;
        let sig = sig_opt.unwrap_or(name);
        let entry = format!("{sig} ({path}:{line})");
        if kind == "variant" {
            if !variants.contains(&entry) {
                variants.push(entry);
            }
        } else if !signatures.contains(&entry) {
            signatures.push(entry);
        }
    }

    if signatures.len() < limit {
        let remaining = (limit - signatures.len()) * 2;
        let p_rows: Vec<(String, Option<String>, String, usize, String)> =
            if has_pending_namespace_column(conn) {
                let mut p_stmt = conn.prepare(
                "SELECT DISTINCT s_to.name, s_to.signature, s_to.path, s_to.start_line, s_to.kind
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 JOIN symbols s_to ON s_to.name = p.target_terminal_name
                 LEFT JOIN symbols s_parent ON s_to.parent_symbol_id = s_parent.symbol_id
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                    AND (
                        (
                            s_to.parent_symbol_id IS NOT NULL
                            AND s_parent.name IS NOT NULL
                            AND (
                                EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = s_parent.name)
                                OR (EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = 'Self')
                                    AND s_from.parent_symbol_id = s_to.parent_symbol_id)
                                OR (p.target_receiver IS NOT NULL AND p.target_receiver != '' AND s_parent.name = p.target_receiver)
                            )
                        )
                        OR (
                            (p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')
                            AND (p.target_receiver IS NULL OR p.target_receiver = '')
                            AND (s_to.parent_symbol_id IS NULL OR s_from.parent_symbol_id = s_to.parent_symbol_id)
                        )
                        OR (
                            s_to.parent_symbol_id IS NULL
                            AND EXISTS (
                                SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                                WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super')
                                  AND ('/' || replace(s_to.path, '\\', '/')) LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                            )
                        )
                    )
                 LIMIT ?3",
            )?;

                let rows =
                    p_stmt.query_map(params![symbol_name, symbol_id, remaining as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?.replace('\\', "/"),
                            row.get::<_, Option<i64>>(3)?.unwrap_or(1) as usize,
                            row.get::<_, String>(4)?,
                        ))
                    })?;
                rows.flatten().collect()
            } else {
                let mut p_stmt = conn.prepare(
                "SELECT DISTINCT s_to.name, s_to.signature, s_to.path, s_to.start_line, s_to.kind
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 JOIN symbols s_to ON s_to.name = p.target_terminal_name
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                 LIMIT ?3",
            )?;

                let rows =
                    p_stmt.query_map(params![symbol_name, symbol_id, remaining as i64], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?.replace('\\', "/"),
                            row.get::<_, Option<i64>>(3)?.unwrap_or(1) as usize,
                            row.get::<_, String>(4)?,
                        ))
                    })?;
                rows.flatten().collect()
            };

        for r in p_rows {
            let (name, sig_opt, path, line, kind) = r;
            let sig = sig_opt.unwrap_or(name);
            let entry = format!("{sig} ({path}:{line})");
            if kind == "variant" {
                if !variants.contains(&entry) {
                    variants.push(entry);
                }
            } else if !signatures.contains(&entry) {
                signatures.push(entry);
            }
        }
    }

    if include_external && signatures.len() < limit {
        let remaining = (limit - signatures.len()) * 2;
        let ext_rows: Vec<(String, String, usize)> = if has_pending_namespace_column(conn) {
            let mut ext_stmt = conn.prepare(
                "SELECT DISTINCT COALESCE(NULLIF(p.target_display_name, ''), p.target_terminal_name), p.path, p.start_line
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM symbols s_to
                       LEFT JOIN symbols s_parent ON s_to.parent_symbol_id = s_parent.symbol_id
                       WHERE s_to.name = p.target_terminal_name
                         AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                         AND (
                             (
                                 s_to.parent_symbol_id IS NOT NULL
                                 AND s_parent.name IS NOT NULL
                                 AND (
                                     EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = s_parent.name)
                                     OR (EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = 'Self')
                                         AND s_from.parent_symbol_id = s_to.parent_symbol_id)
                                     OR (p.target_receiver IS NOT NULL AND p.target_receiver != '' AND s_parent.name = p.target_receiver)
                                 )
                                 AND NOT EXISTS (
                                     SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                                     WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super', 'self', 'Self', s_parent.name)
                                       AND ('/' || replace(s_to.path, '\\', '/')) NOT LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                                       AND ('/' || replace(s_to.path, '\\', '/')) NOT LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '/%' ESCAPE '\\'
                                 )
                             )
                             OR (
                                 (p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')
                                 AND (p.target_receiver IS NULL OR p.target_receiver = '')
                                 AND (s_to.parent_symbol_id IS NULL OR s_from.parent_symbol_id = s_to.parent_symbol_id)
                             )
                             OR (
                                 s_to.parent_symbol_id IS NULL
                                 AND EXISTS (
                                     SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                                     WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super')
                                       AND ('/' || replace(s_to.path, '\\', '/')) LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                                 )
                             )
                         )
                   )
                 LIMIT ?3",
            )?;

            let rows =
                ext_stmt.query_map(params![symbol_name, symbol_id, remaining as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?.replace('\\', "/"),
                        row.get::<_, Option<i64>>(2)?.unwrap_or(1) as usize,
                    ))
                })?;
            rows.flatten().collect()
        } else {
            let mut ext_stmt = conn.prepare(
                "SELECT DISTINCT p.target_terminal_name, p.path, p.start_line
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM symbols s_to
                       WHERE s_to.name = p.target_terminal_name
                         AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                   )
                 LIMIT ?3",
            )?;

            let rows =
                ext_stmt.query_map(params![symbol_name, symbol_id, remaining as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?.replace('\\', "/"),
                        row.get::<_, Option<i64>>(2)?.unwrap_or(1) as usize,
                    ))
                })?;
            rows.flatten().collect()
        };

        for r in ext_rows {
            let (name, path, line) = r;
            let entry = format!("{name} ({path}:{line})");
            if !signatures.contains(&entry) {
                signatures.push(entry);
            }
        }
    }

    for v in variants {
        if signatures.len() >= limit {
            break;
        }
        if !signatures.contains(&v) {
            signatures.push(v);
        }
    }

    signatures.truncate(limit);
    Ok(signatures)
}

/// Find structural facts by category (e.g. route, query, model, config), optionally scoped by path.
pub fn find_structural_facts_scoped(
    conn: &Connection,
    category: &str,
    path_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<StructuralFact>, QueryError> {
    let norm_path = path_filter
        .map(|p| {
            p.replace('\\', "/")
                .trim_start_matches("./")
                .trim_matches('/')
                .to_string()
        })
        .filter(|p| !p.is_empty());
    let dir_prefix = norm_path
        .as_deref()
        .map(|p| format!("{}/%", escape_like(p)));
    let cat_pattern = format!("%{}%", escape_like(category));

    let cat_lower = category.trim().to_ascii_lowercase();
    let cat_clause = match cat_lower.as_str() {
        "config" => {
            "(sf.pattern_id LIKE '%.key_value.%' OR sf.pattern_id LIKE '%config%' OR sf.capture_name LIKE '%config%' OR sf.node_kind LIKE '%config%')"
        }
        "route" | "routes" => {
            "(sf.pattern_id LIKE '%.route%' OR sf.pattern_id LIKE '%route%' OR sf.capture_name LIKE '%route%')"
        }
        "query" | "queries" | "sql" => {
            "(sf.pattern_id LIKE '%.sql.%' OR sf.pattern_id LIKE '%query%')"
        }
        "model" | "models" => "sf.pattern_id LIKE '%.model%'",
        _ => {
            "(sf.pattern_id LIKE :cat ESCAPE '\\' OR sf.capture_name LIKE :cat ESCAPE '\\' OR sf.node_kind LIKE :cat ESCAPE '\\')"
        }
    };

    let sql = format!(
        "SELECT sf.structural_fact_id, sf.path, sf.language, sf.pattern_id,
                sf.capture_name, sf.node_kind, s.name AS containing_symbol_name,
                sf.start_line, sf.end_line, sf.confidence
         FROM structural_facts sf
         LEFT JOIN symbols s ON sf.containing_symbol_id = s.symbol_id
         WHERE (:cat IS NOT NULL AND {cat_clause})
           AND (:path IS NULL OR replace(sf.path, '\\', '/') = :path COLLATE NOCASE OR replace(sf.path, '\\', '/') LIKE :dir_prefix ESCAPE '\\')
         ORDER BY sf.path ASC, sf.start_line ASC
         LIMIT :limit"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::named_params! {
            ":cat": cat_pattern,
            ":path": norm_path.as_deref(),
            ":dir_prefix": dir_prefix.as_deref(),
            ":limit": limit as i64,
        },
        |row| {
            Ok(StructuralFact {
                structural_fact_id: row.get(0)?,
                path: row.get::<_, String>(1)?.replace('\\', "/"),
                language: row.get(2)?,
                pattern_id: row.get(3)?,
                capture_name: row.get(4)?,
                node_kind: row.get(5)?,
                containing_symbol_name: row.get(6)?,
                start_line: row.get::<_, i64>(7)? as usize,
                end_line: row.get::<_, i64>(8)? as usize,
                confidence: row.get(9)?,
            })
        },
    )?;

    let mut results = Vec::new();
    for r in rows {
        results.push(r?);
    }
    Ok(results)
}

/// Find structural facts by category (e.g. route, query, model, config).
pub fn find_structural_facts(
    conn: &Connection,
    category: &str,
    limit: usize,
) -> Result<Vec<StructuralFact>, QueryError> {
    find_structural_facts_scoped(conn, category, None, limit)
}

/// Find literals (endpoints, SQL queries, configs) matching category, optionally scoped by path.
pub fn find_literals_scoped(
    conn: &Connection,
    category: &str,
    path_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<LiteralFact>, QueryError> {
    let norm_path = path_filter
        .map(|p| {
            p.replace('\\', "/")
                .trim_start_matches("./")
                .trim_matches('/')
                .to_string()
        })
        .filter(|p| !p.is_empty());
    let dir_prefix = norm_path
        .as_deref()
        .map(|p| format!("{}/%", escape_like(p)));
    let cat_pattern = format!("%{}%", escape_like(category));

    let cat_lower = category.trim().to_ascii_lowercase();
    let cat_clause = match cat_lower.as_str() {
        "config" => {
            "(l.kind LIKE '%config%' OR l.kind LIKE '%toml%' OR l.kind LIKE '%json%' OR l.kind LIKE '%yaml%')"
        }
        "route" | "routes" => "l.kind LIKE '%route%'",
        "query" | "queries" | "sql" => "(l.kind LIKE '%sql%' OR l.kind LIKE '%query%')",
        "model" | "models" => "l.kind LIKE '%model%'",
        _ => "(l.kind LIKE :cat ESCAPE '\\' OR l.literal_text LIKE :cat ESCAPE '\\')",
    };

    let sql = format!(
        "SELECT l.literal_id, l.path, l.literal_text, l.kind, l.carrier,
                l.start_line, s.name AS containing_symbol_name
         FROM literals l
         LEFT JOIN symbols s ON l.containing_symbol_id = s.symbol_id
         WHERE (:cat IS NOT NULL AND {cat_clause})
           AND (:path IS NULL OR replace(l.path, '\\', '/') = :path COLLATE NOCASE OR replace(l.path, '\\', '/') LIKE :dir_prefix ESCAPE '\\')
         ORDER BY l.path ASC, l.start_line ASC
         LIMIT :limit"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::named_params! {
            ":cat": cat_pattern,
            ":path": norm_path.as_deref(),
            ":dir_prefix": dir_prefix.as_deref(),
            ":limit": limit as i64,
        },
        |row| {
            Ok(LiteralFact {
                literal_id: row.get(0)?,
                path: row.get::<_, String>(1)?.replace('\\', "/"),
                literal_text: row.get(2)?,
                kind: row.get(3)?,
                carrier: row.get(4)?,
                start_line: row.get::<_, i64>(5)? as usize,
                containing_symbol_name: row.get(6)?,
            })
        },
    )?;

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
    find_literals_scoped(conn, category, None, limit)
}

/// List available structural fact and literal categories with counts, optionally scoped by path.
pub fn list_structural_fact_categories_scoped(
    conn: &Connection,
    path_filter: Option<&str>,
) -> Result<Vec<(String, usize)>, QueryError> {
    let norm_path = path_filter
        .map(|p| {
            p.replace('\\', "/")
                .trim_start_matches("./")
                .trim_matches('/')
                .to_string()
        })
        .filter(|p| !p.is_empty());
    let dir_prefix = norm_path
        .as_deref()
        .map(|p| format!("{}/%", escape_like(p)));

    let mut categories = Vec::new();

    let sql = "SELECT pattern_id, COUNT(*) AS cnt FROM structural_facts
               WHERE (:path IS NULL OR replace(path, '\\', '/') = :path COLLATE NOCASE OR replace(path, '\\', '/') LIKE :dir_prefix ESCAPE '\\')
               GROUP BY pattern_id ORDER BY cnt DESC";
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(
        rusqlite::named_params! {
            ":path": norm_path.as_deref(),
            ":dir_prefix": dir_prefix.as_deref(),
        },
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize)),
    )?;
    for r in rows {
        categories.push(r?);
    }

    let lit_sql = "SELECT kind, COUNT(*) AS cnt FROM literals
                   WHERE (:path IS NULL OR replace(path, '\\', '/') = :path COLLATE NOCASE OR replace(path, '\\', '/') LIKE :dir_prefix ESCAPE '\\')
                   GROUP BY kind ORDER BY cnt DESC";
    let mut lit_stmt = conn.prepare(lit_sql)?;
    let lit_rows = lit_stmt.query_map(
        rusqlite::named_params! {
            ":path": norm_path.as_deref(),
            ":dir_prefix": dir_prefix.as_deref(),
        },
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize)),
    )?;
    for r in lit_rows {
        categories.push(r?);
    }

    Ok(categories)
}

/// List all available structural fact and literal categories with counts.
pub fn list_structural_fact_categories(
    conn: &Connection,
) -> Result<Vec<(String, usize)>, QueryError> {
    list_structural_fact_categories_scoped(conn, None)
}

/// Find type facts for a symbol.
pub fn find_type_facts(conn: &Connection, symbol_id: &str) -> Result<Vec<TypeFact>, QueryError> {
    let has_table: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='type_facts'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if !has_table {
        return Ok(Vec::new());
    }

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

/// Helper to determine if a relative path looks like a test file across ecosystems.
pub fn is_test_path(path: &str) -> bool {
    let p = path.to_lowercase().replace('\\', "/");
    p.contains("/test/")
        || p.contains("/tests/")
        || p.contains("/__tests__/")
        || p.contains("_test.")
        || p.contains(".test.")
        || p.contains(".spec.")
        || p.ends_with("test.rs")
        || p.ends_with("tests.rs")
        || p.ends_with("tests.cs")
        || p.ends_with("test.go")
        || p.starts_with("test_")
}

/// Compute blast radius and likely tests for given seed symbols or seed file paths.
/// Recursively walks reverse reachability (transitive callers) up to `max_depth` in SQLite.
pub fn compute_blast_radius_scoped(
    conn: &Connection,
    seed_symbols: &[&str],
    symbol_path_filter: Option<&str>,
    seed_paths: &[&str],
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, QueryError> {
    let max_depth = max_depth.min(5);
    let resolved_seed_symbols = seed_symbols
        .iter()
        .map(|name| {
            get_symbol_by_name(conn, name, symbol_path_filter)?
                .ok_or_else(|| QueryError::SymbolNotFound((*name).to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut seeds = Vec::new();
    let seed_type = if !seed_symbols.is_empty() && !seed_paths.is_empty() {
        for s in seed_symbols {
            seeds.push(s.to_string());
        }
        for p in seed_paths {
            seeds.push(p.to_string());
        }
        "mixed".to_string()
    } else if !seed_symbols.is_empty() {
        for s in seed_symbols {
            seeds.push(s.to_string());
        }
        "symbol".to_string()
    } else if !seed_paths.is_empty() {
        for p in seed_paths {
            seeds.push(p.to_string());
        }
        "file".to_string()
    } else {
        return Ok(BlastRadiusResult {
            seed_type: "none".to_string(),
            seeds: Vec::new(),
            likely_tests: Vec::new(),
            impacted_symbols: Vec::new(),
            traversal_ceiling_reached: false,
        });
    };

    let mut where_clauses = Vec::new();
    let mut params_vec: Vec<rusqlite::types::Value> = Vec::new();

    if !resolved_seed_symbols.is_empty() {
        let placeholders: Vec<String> = (1..=resolved_seed_symbols.len())
            .map(|i| format!("?{}", i))
            .collect();
        where_clauses.push(format!("symbol_id IN ({})", placeholders.join(", ")));
        for symbol in &resolved_seed_symbols {
            params_vec.push(rusqlite::types::Value::Text(symbol.symbol_id.clone()));
        }
    }

    if !seed_paths.is_empty() {
        let mut path_conds = Vec::new();
        for p in seed_paths.iter() {
            let raw = p
                .replace('\\', "/")
                .trim_start_matches("./")
                .trim_matches('/')
                .to_string();
            let exact_idx = params_vec.len() + 1;
            params_vec.push(rusqlite::types::Value::Text(raw.clone()));
            let dir_pattern = format!("{}/%", escape_like(&raw));
            let like_idx = params_vec.len() + 1;
            params_vec.push(rusqlite::types::Value::Text(dir_pattern));
            path_conds.push(format!(
                "replace(path, '\\', '/') = ?{exact_idx} COLLATE NOCASE OR replace(path, '\\', '/') LIKE ?{like_idx} ESCAPE '\\'"
            ));
        }
        where_clauses.push(format!("({})", path_conds.join(" OR ")));
    }

    let seed_condition = where_clauses.join(" OR ");
    let max_depth_idx = params_vec.len() + 1;
    params_vec.push(rusqlite::types::Value::Integer(max_depth as i64));

    let mut traversal_ceiling_reached = false;

    let has_relationships: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='relationships'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    let has_pending: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='pending_relationships'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    let mut likely_tests = Vec::new();
    let mut impacted_symbols = Vec::new();
    let mut seen_test_keys = HashSet::new();

    let mut recursive_branches = Vec::new();

    if has_relationships {
        recursive_branches.push(format!(
            "SELECT r.from_symbol_id, iw.depth + 1
             FROM relationships r
             JOIN impact_walk iw ON r.to_symbol_id = iw.symbol_id
             JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
             WHERE iw.depth < ?{max_depth_idx}
               AND s_from.kind NOT IN ('import','variable','parameter','field','property','module','namespace')"
        ));
    }

    if has_pending {
        let (parent_join, ns_condition) = if conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('pending_relationships') WHERE name='target_namespace_json'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false)
        {
            (
                "LEFT JOIN symbols s_target_parent ON s_target.parent_symbol_id = s_target_parent.symbol_id
            LEFT JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id",
                "AND (
                    (
                        s_target.parent_symbol_id IS NOT NULL
                        AND s_target_parent.name IS NOT NULL
                        AND (
                            EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = s_target_parent.name)
                            OR (EXISTS (SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END) WHERE value = 'Self')
                                AND s_from.parent_symbol_id = s_target.parent_symbol_id)
                            OR (p.target_receiver IS NOT NULL AND p.target_receiver != '' AND s_target_parent.name = p.target_receiver)
                        )
                        AND NOT EXISTS (
                            SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                            WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super', 'self', 'Self', s_target_parent.name)
                              AND ('/' || replace(s_target.path, '\\', '/')) NOT LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                              AND ('/' || replace(s_target.path, '\\', '/')) NOT LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '/%' ESCAPE '\\'
                        )
                    )
                    OR (
                        (p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')
                        AND (p.target_receiver IS NULL OR p.target_receiver = '')
                        AND (s_target.parent_symbol_id IS NULL OR s_from.parent_symbol_id = s_target.parent_symbol_id)
                    )
                    OR (
                        s_target.parent_symbol_id IS NULL
                        AND EXISTS (
                            SELECT 1 FROM json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)
                            WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super')
                              AND ('/' || replace(s_target.path, '\\', '/')) LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
                        )
                    )
                )",
            )
        } else {
            ("", "")
        };

        recursive_branches.push(format!(
            "SELECT p.from_symbol_id, iw.depth + 1
             FROM pending_relationships p
             JOIN symbols s_target ON p.target_terminal_name = s_target.name
             JOIN impact_walk iw ON s_target.symbol_id = iw.symbol_id
             {parent_join}
             WHERE iw.depth < ?{max_depth_idx}
               AND s_target.kind NOT IN ('import','variable','parameter','field','property','module','namespace')
               {ns_condition}"
        ));
    }

    if !recursive_branches.is_empty() {
        let recursive_sql = recursive_branches.join("\n UNION \n");
        let sql = format!(
            "WITH RECURSIVE impact_walk(symbol_id, depth) AS (
                SELECT symbol_id, 0
                FROM symbols
                WHERE ({seed_condition})
                  AND kind NOT IN ('import','variable','parameter','field','property','module','namespace')

                UNION

                {recursive_sql}
            )
            SELECT s.symbol_id, s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container, MIN(iw.depth) as min_depth
            FROM impact_walk iw
            CROSS JOIN symbols s ON iw.symbol_id = s.symbol_id
            WHERE s.kind NOT IN ('import','variable','parameter','field','property','module','namespace')
            GROUP BY s.symbol_id, s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container
            HAVING MIN(iw.depth) > 0
            ORDER BY min_depth ASC, s.path ASC, s.name ASC
            LIMIT 200"
        );

        let mut stmt = conn.prepare(&sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> = params_vec
            .iter()
            .map(|v| v as &dyn rusqlite::ToSql)
            .collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)? as usize,
                row.get::<_, bool>(5)?,
                row.get::<_, bool>(6)?,
                row.get::<_, i64>(7)? as usize,
            ))
        })?;

        let mut row_count = 0;
        for r in rows {
            row_count += 1;
            let (_sym_id, name, kind, raw_path, line, is_test, test_container, depth) = r?;
            let path = raw_path.replace('\\', "/");
            let is_test_target = is_test || test_container || is_test_path(&path);

            if is_test_target {
                let key = format!("{}:{}", path, line);
                if seen_test_keys.insert(key) {
                    likely_tests.push(TestTarget {
                        name,
                        path,
                        line,
                        reason: format!("transitive caller [depth {depth}]"),
                    });
                }
            } else {
                impacted_symbols.push(ImpactedSymbol {
                    name,
                    kind,
                    path,
                    line,
                    depth,
                });
            }
        }
        traversal_ceiling_reached = row_count >= 200;
    }

    // 2. Discover stem-matched test files in the workspace
    let mut file_stems = Vec::new();
    for p in seed_paths {
        if let Some(stem) = std::path::Path::new(p).file_stem().and_then(|s| s.to_str())
            && stem.len() >= 3
            && !file_stems.contains(&stem.to_string())
        {
            file_stems.push(stem.to_string());
        }
    }
    for symbol in &resolved_seed_symbols {
        if let Some(stem) = std::path::Path::new(&symbol.path)
            .file_stem()
            .and_then(|s| s.to_str())
            && stem.len() >= 3
            && !file_stems.contains(&stem.to_string())
        {
            file_stems.push(stem.to_string());
        }
    }

    let has_files: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='files'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if has_files {
        let mut test_files_stmt = conn.prepare(
            "SELECT DISTINCT path FROM files WHERE (path LIKE '%test%' OR path LIKE '%spec%') AND path LIKE ?1 ESCAPE '\\' LIMIT 10",
        )?;
        for stem in file_stems {
            let stem_pattern = format!("%{}%", escape_like(&stem));
            let t_rows =
                test_files_stmt.query_map([stem_pattern], |row| row.get::<_, String>(0))?;
            for p in t_rows.flatten() {
                let p = p.replace('\\', "/");
                let key = format!("{}:1", p);
                if seen_test_keys.insert(key) {
                    likely_tests.push(TestTarget {
                        name: p.clone(),
                        path: p,
                        line: 1,
                        reason: "stem-matched test file".to_string(),
                    });
                }
            }
        }
    }

    // Truncate to limit
    if likely_tests.len() > limit {
        likely_tests.truncate(limit);
    }
    if impacted_symbols.len() > limit {
        impacted_symbols.truncate(limit);
    }

    Ok(BlastRadiusResult {
        seed_type,
        seeds,
        likely_tests,
        impacted_symbols,
        traversal_ceiling_reached,
    })
}

/// Compute blast radius and likely tests for given seed symbols or seed file paths.
pub fn compute_blast_radius(
    conn: &Connection,
    seed_symbols: &[&str],
    seed_paths: &[&str],
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, QueryError> {
    compute_blast_radius_scoped(conn, seed_symbols, None, seed_paths, max_depth, limit)
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
    fn search_symbols_treats_like_wildcards_as_literals() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("search_symbols_treats_like_wildcards.db");
        let conn = open_read_write(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            INSERT INTO symbols VALUES (
                's', 'f', 'src/lib.rs', 'rust', 'ordinary', 'function', NULL, NULL, NULL, NULL,
                1, 0, 1, 0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            INSERT INTO symbols VALUES (
                'p', 'f', 'src/lib.rs', 'rust', 'literal%name', 'function', NULL, NULL, NULL, NULL,
                1, 0, 1, 0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            INSERT INTO symbols VALUES (
                'u', 'f', 'src/lib.rs', 'rust', 'literal_name', 'function', NULL, NULL, NULL, NULL,
                1, 0, 1, 0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0
            );
            CREATE TABLE files (
                file_id TEXT, path TEXT, language TEXT, content_hash TEXT,
                content_bytes INTEGER, line_count INTEGER, indexed_at TEXT
            );
            INSERT INTO files VALUES ('f1', 'src/literal_path/lib.rs', 'rust', 'hash', 0, 0, 'now');
            INSERT INTO files VALUES ('f2', 'src/literalXpath/lib.rs', 'rust', 'hash', 0, 0, 'now'
            );",
        )
        .unwrap();

        assert_eq!(
            search_symbols(&conn, "%", None, false, 10).unwrap()[0].name,
            "literal%name"
        );
        assert_eq!(
            search_symbols(&conn, "_", None, false, 10).unwrap()[0].name,
            "literal_name"
        );
        assert_eq!(
            load_scoped_files(&conn, Some("src/literal_path"))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn find_references_for_symbol_limits_callees_by_symbol_id() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("find_references_for_symbol.db");
        let conn = open_read_write(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            CREATE TABLE relationships (
                from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT, path TEXT,
                start_line INTEGER, start_column INTEGER
            );
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, start_column INTEGER
            );
            INSERT INTO symbols VALUES
                ('wanted', 'f', 'a.rs', 'rust', 'new', 'method', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
                ('other', 'f', 'b.rs', 'rust', 'new', 'method', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
                ('wanted-callee', 'f', 'a.rs', 'rust', 'wanted_dep', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
                ('other-callee', 'f', 'b.rs', 'rust', 'other_dep', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
            INSERT INTO relationships VALUES
                ('other', 'other-callee', 'calls', 'b.rs', 1, 0),
                ('wanted', 'wanted-callee', 'calls', 'a.rs', 1, 0);",
        )
        .unwrap();

        let references = find_references_for_symbol(&conn, "new", "callees", 1, "wanted").unwrap();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].to_symbol_name, "wanted_dep");
    }

    #[test]
    fn test_fts_search_symbols_and_porter_stemming() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("fts_search_symbols.db");
        let conn = open_read_write(&db_path).unwrap();

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
        let results =
            fts_search_symbols_scoped(&conn, "parsing tokens", None, None, false, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol.name, "parse_tokens");
        assert!(results[0].snippet.is_some());

        // 2. Docstring conceptual search: 'transactions' matches 'PaymentGateway'
        let results =
            fts_search_symbols_scoped(&conn, "transactions", None, None, false, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol.name, "PaymentGateway");

        // 3. Test filter: searching 'payment' with include_tests=false ignores 'test_payment_flow'
        let results = fts_search_symbols_scoped(&conn, "payment", None, None, false, 10).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|r| !r.symbol.is_test));

        // 4. Test filter: searching 'payment' with include_tests=true includes 'test_payment_flow'
        let results = fts_search_symbols_scoped(&conn, "payment", None, None, true, 10).unwrap();
        assert_eq!(results.len(), 3);

        // 5. Fallback OR matching: multi-term where only some match
        let results =
            fts_search_symbols_scoped(&conn, "stripe kafka redis", None, None, false, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].symbol.name, "StripeClient");
    }

    #[test]
    fn test_queries_nocase_and_path_normalization() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                language TEXT,
                content_hash TEXT,
                content_bytes INTEGER,
                line_count INTEGER,
                indexed_at INTEGER
            );
            CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                file_id TEXT,
                path TEXT NOT NULL,
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
            -- Insert with backslashes and mixed casing to verify defensive normalization and COLLATE NOCASE
            INSERT INTO files VALUES ('f1', 'src\\Payment.rs', 'rust', 'hash1', 100, 10, '2026-09-14T00:00:00Z');
            INSERT INTO symbols VALUES (
                's1', 'f1', 'src\\Payment.rs', 'rust', 'ProcessPayment', 'function',
                'pub fn ProcessPayment()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
                2, 4, 4, 1, 10, 45, 'bhash', 'function', 0, 0
            );",
        )
        .unwrap();

        // 1. get_file: query with uppercase, lowercase, and forward slashes
        let file = get_file(&conn, "SRC/PAYMENT.RS")
            .unwrap()
            .expect("File should be found");
        assert_eq!(
            file.path, "src/Payment.rs",
            "Path should be normalized to forward slashes"
        );

        let file2 = get_file(&conn, "src/payment.rs")
            .unwrap()
            .expect("File should be found");
        assert_eq!(file2.path, "src/Payment.rs");

        // 2. load_file_symbols: query with uppercase and forward slashes
        let syms = load_file_symbols(&conn, "SRC/PAYMENT.RS").unwrap();
        assert_eq!(syms.len(), 1);
        assert_eq!(
            syms[0].path, "src/Payment.rs",
            "Symbol path should be normalized to forward slashes"
        );

        // 3. get_symbol_by_name with path filter
        let sym = get_symbol_by_name(&conn, "ProcessPayment", Some("SRC/PAYMENT.RS"))
            .unwrap()
            .expect("Symbol should be found with case-insensitive path filter");
        assert_eq!(sym.path, "src/Payment.rs");
    }

    #[test]
    fn test_exact_case_prioritized_over_nocase() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE files (
                file_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                language TEXT,
                content_hash TEXT,
                content_bytes INTEGER,
                line_count INTEGER,
                indexed_at TEXT
            );
            CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                file_id TEXT,
                path TEXT NOT NULL,
                language TEXT,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
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
            INSERT INTO files VALUES ('f1', 'src/Payment.rs', 'rust', 'h1', 100, 10, '2026-09-14T00:00:00Z');
            INSERT INTO files VALUES ('f2', 'src/payment.rs', 'rust', 'h2', 100, 10, '2026-09-14T00:00:00Z');
            INSERT INTO symbols VALUES (
                's1', 'f1', 'src/Payment.rs', 'rust', 'pay', 'function',
                'pub fn pay()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
                2, 4, 4, 1, 10, 45, 'b1', 'function', 0, 0
            );
            INSERT INTO symbols VALUES (
                's2', 'f2', 'src/payment.rs', 'rust', 'pay', 'function',
                'pub fn pay()', NULL, 'pub', NULL, 1, 0, 5, 0, 0, 50,
                2, 4, 4, 1, 10, 45, 'b2', 'function', 0, 0
            );",
        )
        .unwrap();

        // Exact match should return exact file, not conflate with sibling differing only by case
        let f_lower = get_file(&conn, "src/payment.rs").unwrap().unwrap();
        assert_eq!(f_lower.path, "src/payment.rs");
        assert_eq!(f_lower.file_id, "f2");

        let f_upper = get_file(&conn, "src/Payment.rs").unwrap().unwrap();
        assert_eq!(f_upper.path, "src/Payment.rs");
        assert_eq!(f_upper.file_id, "f1");

        let syms_lower = load_file_symbols(&conn, "src/payment.rs").unwrap();
        assert_eq!(syms_lower.len(), 1);
        assert_eq!(syms_lower[0].file_id, "f2");

        let syms_upper = load_file_symbols(&conn, "src/Payment.rs").unwrap();
        assert_eq!(syms_upper.len(), 1);
        assert_eq!(syms_upper[0].file_id, "f1");
    }

    #[test]
    fn test_conservative_pending_resolution_ignores_unmatched_namespace() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("conservative_resolution.db");
        let conn = open_read_write(&db_path).unwrap();

        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT,
                name TEXT, kind TEXT, signature TEXT, doc_comment TEXT,
                visibility TEXT, parent_symbol_id TEXT, start_line INTEGER,
                start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            CREATE TABLE relationships (
                from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT, path TEXT,
                start_line INTEGER, start_column INTEGER
            );
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, start_column INTEGER,
                target_receiver TEXT, target_namespace_json TEXT, target_display_name TEXT
            );
            -- Workspace struct Workspace and method Workspace::new
            INSERT INTO symbols VALUES
                ('s_ws', 'f1', 'src/workspace.rs', 'rust', 'Workspace', 'struct', 'pub struct Workspace', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'struct', 0, 0),
                ('s_ws_new', 'f1', 'src/workspace.rs', 'rust', 'new', 'method', 'pub fn new() -> Workspace', NULL, 'pub', 's_ws', 2, 4, 4, 5, 20, 50, 2, 4, 4, 5, 20, 50, 'h1', 'method', 0, 0),
                ('s_caller', 'f2', 'src/caller.rs', 'rust', 'my_func', 'function', 'pub fn my_func()', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'function', 0, 0);

            -- my_func calls Vec::new() (external namespace 'Vec')
            INSERT INTO pending_relationships (from_symbol_id, target_terminal_name, kind, path, start_line, start_column, target_receiver, target_namespace_json, target_display_name) VALUES
                ('s_caller', 'new', 'calls', 'src/caller.rs', 3, 8, NULL, '[\"Vec\"]', 'Vec::new');",
        )
        .unwrap();

        // When include_external is false, calling Vec::new() should NOT resolve to Workspace::new()
        let sigs = find_callee_signatures(&conn, "my_func", "s_caller", 10, false).unwrap();
        assert!(sigs.is_empty(), "Expected 0 signatures, got: {:?}", sigs);

        let refs = find_references_for_symbol(&conn, "my_func", "callees", 10, "s_caller").unwrap();
        assert!(refs.is_empty(), "Expected 0 references, got: {:?}", refs);

        // Caller references for Workspace::new should NOT list my_func
        let callers = find_references_for_symbol(&conn, "new", "callers", 10, "s_ws_new").unwrap();
        assert!(
            callers.is_empty(),
            "Expected 0 callers for Workspace::new, got: {:?}",
            callers
        );

        // Blast radius for Workspace::new should NOT impact my_func (which only called Vec::new)
        let blast = compute_blast_radius(&conn, &["new"], &["src/workspace.rs"], 2, 20).unwrap();
        assert!(
            !blast.impacted_symbols.iter().any(|s| s.name == "my_func"),
            "my_func should not be impacted before calling Workspace::new: {:?}",
            blast.impacted_symbols
        );

        // Now add a call to Workspace::new()
        conn.execute(
            "INSERT INTO pending_relationships (from_symbol_id, target_terminal_name, kind, path, start_line, start_column, target_receiver, target_namespace_json, target_display_name) VALUES ('s_caller', 'new', 'calls', 'src/caller.rs', 5, 8, NULL, '[\"Workspace\"]', 'Workspace::new')",
            [],
        )
        .unwrap();

        let sigs2 = find_callee_signatures(&conn, "my_func", "s_caller", 10, false).unwrap();
        assert_eq!(
            sigs2.len(),
            1,
            "Expected 1 signature for Workspace::new, got: {:?}",
            sigs2
        );
        assert!(sigs2[0].contains("pub fn new() -> Workspace"));

        // Blast radius for Workspace::new should now include my_func
        let blast2 = compute_blast_radius(&conn, &["new"], &["src/workspace.rs"], 2, 20).unwrap();
        assert!(
            blast2.impacted_symbols.iter().any(|s| s.name == "my_func"),
            "my_func should be impacted after calling Workspace::new: {:?}",
            blast2.impacted_symbols
        );

        // Add a bare call to new() from an unrelated caller s_other
        conn.execute(
            "INSERT INTO symbols VALUES
                ('s_other', 'f3', 'src/other.rs', 'rust', 'other_func', 'function', 'pub fn other_func()', NULL, 'pub', NULL, 1, 0, 10, 0, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 'function', 0, 0);",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pending_relationships (from_symbol_id, target_terminal_name, kind, path, start_line, start_column, target_receiver, target_namespace_json, target_display_name) VALUES ('s_other', 'new', 'calls', 'src/other.rs', 2, 8, NULL, NULL, 'new')",
            [],
        )
        .unwrap();

        // Bare call from unrelated function should NOT resolve to Workspace::new
        let sigs_other = find_callee_signatures(&conn, "other_func", "s_other", 10, false).unwrap();
        assert!(
            sigs_other.is_empty(),
            "Bare call to new() from outside Workspace should not resolve to Workspace::new: {:?}",
            sigs_other
        );

        // A sibling method inside Workspace calling bare new() SHOULD resolve to Workspace::new
        conn.execute(
            "INSERT INTO symbols VALUES
                ('s_ws_helper', 'f1', 'src/workspace.rs', 'rust', 'helper', 'method', 'pub fn helper()', NULL, 'pub', 's_ws', 5, 4, 7, 5, 60, 90, 5, 4, 7, 5, 60, 90, 'h2', 'method', 0, 0);",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pending_relationships (from_symbol_id, target_terminal_name, kind, path, start_line, start_column, target_receiver, target_namespace_json, target_display_name) VALUES ('s_ws_helper', 'new', 'calls', 'src/workspace.rs', 6, 8, NULL, NULL, 'new')",
            [],
        )
        .unwrap();

        let sigs_sibling =
            find_callee_signatures(&conn, "helper", "s_ws_helper", 10, false).unwrap();
        assert_eq!(
            sigs_sibling.len(),
            1,
            "Sibling method calling bare new() should resolve to Workspace::new: {:?}",
            sigs_sibling
        );

        // With include_external: true, external calls should be returned
        let ext_sigs = find_callee_signatures(&conn, "my_func", "s_caller", 10, true).unwrap();
        assert!(
            ext_sigs.iter().any(|s| s.contains("Vec")),
            "include_external: true should include external Vec::new: {:?}",
            ext_sigs
        );
    }

    #[test]
    fn test_find_structural_facts_and_literals_scoped() {
        let dir = crate::safe_tempdir();
        let db_path = dir.path().join("facts_test.db");
        let conn = open_read_write(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            CREATE TABLE structural_facts (
                structural_fact_id TEXT PRIMARY KEY, file_id TEXT, path TEXT NOT NULL, language TEXT,
                pattern_id TEXT, capture_name TEXT, node_kind TEXT, containing_symbol_id TEXT,
                start_line INTEGER, end_line INTEGER, confidence REAL
            );
            CREATE TABLE literals (
                literal_id TEXT PRIMARY KEY, file_id TEXT, path TEXT NOT NULL, language TEXT,
                kind TEXT, literal_text TEXT, carrier TEXT, containing_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER
            );
            INSERT INTO structural_facts VALUES
                ('sf_toml', 'f1', 'Cargo.toml', 'toml', 'toml.key_value.v1', 'package.name', 'table', NULL, 1, 2, 1.0),
                ('sf_route', 'f2', 'src/routes/api.rs', 'rust', 'http.route.v1', 'get_users', 'function', NULL, 10, 20, 1.0),
                ('sf_sql', 'f3', 'src/db/queries.rs', 'rust', 'db.sql.select', 'select_users', 'function', NULL, 30, 40, 1.0),
                ('sf_model', 'f4', 'src/models/user.rs', 'rust', 'orm.model.entity', 'User', 'struct', NULL, 50, 60, 1.0),
                ('sf_custom', 'f5', 'src/custom.rs', 'rust', 'my_custom_pattern', 'custom_name', 'item', NULL, 70, 80, 1.0);
            INSERT INTO literals VALUES
                ('lit_toml', 'f1', 'Cargo.toml', 'toml', 'toml_key', '\"version\"', 'key', NULL, 3, 0, 3, 9, 20, 29),
                ('lit_route', 'f2', 'src/routes/api.rs', 'rust', 'http_route', '\"/api/v1/users\"', 'string', NULL, 12, 0, 12, 15, 100, 115),
                ('lit_sql', 'f3', 'src/db/queries.rs', 'rust', 'sql_query', '\"SELECT * FROM users\"', 'string', NULL, 32, 0, 32, 21, 200, 221),
                ('lit_model', 'f4', 'src/models/user.rs', 'rust', 'model_table', '\"users_table\"', 'string', NULL, 52, 0, 52, 13, 300, 313);",
        )
        .unwrap();

        // 1. "config" alias
        let facts_config = find_structural_facts_scoped(&conn, "config", None, 10).unwrap();
        assert_eq!(facts_config.len(), 1);
        assert_eq!(facts_config[0].pattern_id, "toml.key_value.v1");
        let lits_config = find_literals_scoped(&conn, "config", None, 10).unwrap();
        assert_eq!(lits_config.len(), 1);
        assert_eq!(lits_config[0].kind, "toml_key");

        // 2. "route" and "routes" aliases
        let facts_route = find_structural_facts_scoped(&conn, "route", None, 10).unwrap();
        assert_eq!(facts_route.len(), 1);
        assert_eq!(facts_route[0].pattern_id, "http.route.v1");
        let facts_routes = find_structural_facts_scoped(&conn, "routes", None, 10).unwrap();
        assert_eq!(facts_routes.len(), 1);
        let lits_route = find_literals_scoped(&conn, "route", None, 10).unwrap();
        assert_eq!(lits_route.len(), 1);
        assert_eq!(lits_route[0].kind, "http_route");

        // 3. "query", "queries", "sql" aliases
        for q in &["query", "queries", "sql"] {
            let facts = find_structural_facts_scoped(&conn, q, None, 10).unwrap();
            assert_eq!(facts.len(), 1, "Failed for {}", q);
            assert_eq!(facts[0].pattern_id, "db.sql.select");
            let lits = find_literals_scoped(&conn, q, None, 10).unwrap();
            assert_eq!(lits.len(), 1, "Failed for {}", q);
            assert_eq!(lits[0].kind, "sql_query");
        }

        // 4. "model" and "models" aliases
        for m in &["model", "models"] {
            let facts = find_structural_facts_scoped(&conn, m, None, 10).unwrap();
            assert_eq!(facts.len(), 1, "Failed for {}", m);
            assert_eq!(facts[0].pattern_id, "orm.model.entity");
            let lits = find_literals_scoped(&conn, m, None, 10).unwrap();
            assert_eq!(lits.len(), 1, "Failed for {}", m);
            assert_eq!(lits[0].kind, "model_table");
        }

        // 5. Custom / unknown category
        let facts_custom = find_structural_facts_scoped(&conn, "custom_pattern", None, 10).unwrap();
        assert_eq!(facts_custom.len(), 1);
        assert_eq!(facts_custom[0].pattern_id, "my_custom_pattern");

        // 6. Path filter: exact file match
        let facts_exact =
            find_structural_facts_scoped(&conn, "config", Some("Cargo.toml"), 10).unwrap();
        assert_eq!(facts_exact.len(), 1);
        let facts_miss =
            find_structural_facts_scoped(&conn, "config", Some("src/routes/api.rs"), 10).unwrap();
        assert_eq!(facts_miss.len(), 0);

        // 7. Path filter: directory prefix
        let facts_dir =
            find_structural_facts_scoped(&conn, "route", Some("src/routes"), 10).unwrap();
        assert_eq!(facts_dir.len(), 1);
        let facts_dir_miss =
            find_structural_facts_scoped(&conn, "route", Some("src/db"), 10).unwrap();
        assert_eq!(facts_dir_miss.len(), 0);

        // 8. Delegating find_structural_facts and find_literals
        let f_del = find_structural_facts(&conn, "config", 10).unwrap();
        assert_eq!(f_del.len(), 1);
        let l_del = find_literals(&conn, "config", 10).unwrap();
        assert_eq!(l_del.len(), 1);
    }
}
