use rusqlite::{params, Connection, Row};
use thiserror::Error;

use crate::models::{FileFact, LiteralFact, ReferenceSite, StructuralFact, Symbol, TypeFact};

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("Database query error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Symbol '{0}' not found")]
    SymbolNotFound(String),
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
    let mut stmt = conn.prepare(
        "SELECT file_id, path, language, content_hash, content_bytes, line_count, indexed_at
         FROM files
         ORDER BY path ASC",
    )?;

    let files = stmt
        .query_map([], |row| {
            Ok(FileFact {
                file_id: row.get(0)?,
                path: row.get(1)?,
                language: row.get(2)?,
                content_hash: row.get(3)?,
                content_bytes: row.get(4)?,
                line_count: row.get(5)?,
                indexed_at: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(files)
}

/// Lookup single file metadata by path.
pub fn get_file(conn: &Connection, path: &str) -> Result<Option<FileFact>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT file_id, path, language, content_hash, content_bytes, line_count, indexed_at
         FROM files
         WHERE path = ?1 OR path LIKE '%' || ?1
         LIMIT 1",
    )?;

    let mut rows = stmt.query(params![path])?;
    if let Some(row) = rows.next()? {
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
         WHERE path = ?1 OR path = ?2 OR path LIKE '%' || ?1
         ORDER BY start_line ASC, start_column ASC",
    )?;

    // Normalizing slashes for path matching
    let normalized = file_path.replace('\\', "/");
    let backslash = file_path.replace('/', "\\");

    let rows = stmt
        .query_map(params![normalized, backslash], |row| map_symbol(row))?
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
        stmt.query_map(params![query, pattern, k], |row| map_symbol(row))?
            .collect::<Result<Vec<_>, _>>()?
    } else {
        stmt.query_map(params![query, pattern], |row| map_symbol(row))?
            .collect::<Result<Vec<_>, _>>()?
    };

    Ok(rows)
}

/// Find a specific symbol by name, with an optional path filter for disambiguation.
pub fn get_symbol_by_name(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> Result<Option<Symbol>, QueryError> {
    // Check if name is qualified like `Struct::method` or `Class.method`
    let terminal_name = if let Some(idx) = name.rfind("::") {
        &name[idx + 2..]
    } else if let Some(idx) = name.rfind('.') {
        &name[idx + 1..]
    } else {
        name
    };

    let mut sql = String::from(
        "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
         FROM symbols
         WHERE (name = ?1 OR name = ?2)",
    );

    if path_filter.is_some() {
        sql.push_str(" AND (path = ?3 OR path LIKE '%' || ?3)");
    }

    sql.push_str(" ORDER BY (name = ?1) DESC, is_test ASC LIMIT 1");

    let mut stmt = conn.prepare(&sql)?;

    let mut rows = if let Some(p) = path_filter {
        let normalized = p.replace('\\', "/");
        stmt.query(params![name, terminal_name, normalized])?
    } else {
        stmt.query(params![name, terminal_name])?
    };

    if let Some(row) = rows.next()? {
        Ok(Some(map_symbol(row)?))
    } else {
        Ok(None)
    }
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
