use rusqlite::{Connection, Row, ToSql, params};
use rust_stemmers::{Algorithm, Stemmer};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

use crate::db::local_variable_predicate;
use crate::models::{
    BlastRadiusResult, FileFact, ImpactedSymbol, LiteralFact, ReferenceSite, SearchExplain,
    StructuralFact, Symbol, SymbolSearchResult, TestTarget, TypeFact,
};

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("Database query error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Symbol '{name}' not found in {workspace}. {hint}")]
    SymbolNotFound {
        name: String,
        workspace: String,
        hint: String,
    },
    #[error(
        "Ambiguous symbol '{0}': found {1} matching candidates. Specify file_path or qualified name to disambiguate:\n{2}"
    )]
    AmbiguousSymbol(String, usize, String),
    #[error("Invalid direction '{0}': must be 'callers' or 'callees'")]
    InvalidDirection(String),
    #[error("Result limit must be between 0 and {MAX_RESULT_LIMIT}, got {0}")]
    InvalidResultLimit(usize),
}

pub const MAX_RESULT_LIMIT: usize = 200;

pub fn validate_result_limit(limit: usize) -> Result<(), QueryError> {
    if limit > MAX_RESULT_LIMIT {
        return Err(QueryError::InvalidResultLimit(limit));
    }
    Ok(())
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

/// The largest number of files one answer is measured against.
pub const BASELINE_PATH_CAP: usize = 20;

/// Sums the indexed byte size of up to `BASELINE_PATH_CAP` distinct paths.
/// Returns 0 when no path is indexed, so telemetry never fails a tool call.
pub fn file_sizes_for_paths(conn: &Connection, paths: &[String]) -> usize {
    let mut seen = std::collections::HashSet::new();
    let distinct: Vec<&String> = paths
        .iter()
        .filter(|path| seen.insert(path.as_str()))
        .take(BASELINE_PATH_CAP)
        .collect();
    if distinct.is_empty() {
        return 0;
    }

    let placeholders = vec!["?"; distinct.len()].join(", ");
    let sql =
        format!("SELECT COALESCE(SUM(content_bytes), 0) FROM files WHERE path IN ({placeholders})");
    conn.query_row(&sql, rusqlite::params_from_iter(distinct), |row| {
        row.get::<_, i64>(0)
    })
    .map(|bytes| bytes.max(0) as usize)
    .unwrap_or(0)
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

/// Count parse diagnostics recorded for a file, returning 0 when the index has none.
/// Number of indexed symbols in one file, without loading the rows. Like `load_file_symbols`,
/// an exact-case path wins and the case-insensitive match is only the fallback.
pub fn count_file_symbols(conn: &Connection, path: &str) -> usize {
    let count = |sql: &str| {
        conn.query_row(
            sql,
            params![path.replace('\\', "/"), path.replace('/', "\\")],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count.max(0) as usize)
        .unwrap_or(0)
    };
    match count("SELECT COUNT(*) FROM symbols WHERE path = ?1 OR path = ?2") {
        0 => count(
            "SELECT COUNT(*) FROM symbols
             WHERE path = ?1 COLLATE NOCASE OR path = ?2 COLLATE NOCASE",
        ),
        exact => exact,
    }
}

pub fn count_parse_diagnostics(conn: &Connection, path: &str) -> usize {
    conn.query_row(
        "SELECT COUNT(*) FROM parse_diagnostics
         WHERE path = ?1 COLLATE NOCASE OR path = ?2 COLLATE NOCASE",
        params![path.replace('\\', "/"), path.replace('/', "\\")],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count as usize)
    .unwrap_or(0)
}

/// Count files julie could not parse under a path, returning 0 when the index has none.
pub fn count_unsupported_files(conn: &Connection, path_filter: Option<&str>) -> usize {
    let norm = path_filter
        .map(|p| p.replace('\\', "/").trim_matches('/').to_string())
        .filter(|p| !p.is_empty());
    let norm_bs = norm.as_ref().map(|p| p.replace('/', "\\"));
    let prefix = norm.as_ref().map(|p| format!("{}/%", escape_like(p)));
    let prefix_bs = norm_bs.as_ref().map(|p| format!("{}\\\\%", escape_like(p)));

    conn.query_row(
        "SELECT COUNT(*) FROM files
         WHERE status = 'unsupported'
           AND (:path IS NULL
             OR path = :path COLLATE NOCASE
             OR path = :path_bs COLLATE NOCASE
             OR path LIKE :path_prefix ESCAPE '\\'
             OR path LIKE :path_prefix_bs ESCAPE '\\')",
        rusqlite::named_params! {
            ":path": norm.as_deref(),
            ":path_bs": norm_bs.as_deref(),
            ":path_prefix": prefix.as_deref(),
            ":path_prefix_bs": prefix_bs.as_deref(),
        },
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count as usize)
    .unwrap_or(0)
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

/// Search symbols with optional path scoping filter. Locals and parameters are left out unless
/// the caller passes `kind = "variable"` or names one explicitly as a qualified name such as
/// `open_conn::conn`. With `kind = "variable"` they match by name only, because they are not in
/// the full-text index.
/// The lookup statement. The exact-name bypass of the test filter is written as `+name` so
/// SQLite cannot plan it as a multi-index OR, which scans every non-test row through the
/// test-path predicate (about 8 ms on this repository's index against 1 ms).
fn search_symbols_sql(variables_wanted: bool, include_tests: bool, limit: usize) -> String {
    let mut sql = String::from(
        "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
         FROM symbols s
         WHERE (name = :query OR name LIKE :pattern ESCAPE '\\')
           AND (:kind IS NULL OR kind = :kind)
           AND (:path IS NULL OR replace(path, '\\', '/') = :path COLLATE NOCASE OR replace(path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
    );

    if !variables_wanted {
        sql.push_str(&format!(" AND NOT {}", local_variable_predicate("s")));
    }

    if !include_tests {
        sql.push_str(&format!(
            " AND (+name = :query OR (is_test = 0 AND test_container = 0 AND NOT {}))",
            test_path_predicate("s")
        ));
    }

    sql.push_str(
        " ORDER BY (name = :query) DESC, (kind IN ('function', 'struct', 'class', 'trait', 'method', 'enum', 'interface', 'type')) DESC, length(name) ASC, path ASC LIMIT ",
    );
    sql.push_str(&limit.to_string());
    sql
}

pub fn search_symbols_scoped(
    conn: &Connection,
    query: &str,
    kind_filter: Option<&str>,
    path_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<Symbol>, QueryError> {
    validate_result_limit(limit)?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let norm_kind = kind_filter.map(normalize_kind);
    if (query.contains("::") || query.contains('.'))
        && let Some(sym) = get_symbol_by_name(conn, query, path_filter)?
    {
        let kind_matches = norm_kind.as_deref().is_none_or(|kind| sym.kind == kind);
        return Ok(if kind_matches { vec![sym] } else { Vec::new() });
    }

    let pattern = format!("%{}%", escape_like(query));
    let normalized_path = path_filter.map(|p| {
        p.replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    });
    let escaped_path = normalized_path.as_deref().map(escape_like);

    let sql = search_symbols_sql(
        norm_kind.as_deref() == Some("variable"),
        include_tests,
        limit,
    );
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
/// Identifiers are split at case boundaries too (`parseHTTPResponse` -> `parse HTTP Response`),
/// so a camelCase query finds a snake_case symbol. Each split identifier keeps its unsplit form
/// as an alternative inside its own required term, so a camelCase symbol, which FTS5 indexes as
/// one token, still matches. A query of two or three words also tries their concatenation.
/// English stop words are dropped unless the whole query is stop words, and tokens under three
/// characters get no prefix wildcard.
pub fn sanitize_fts5_query(query: &str) -> (String, String) {
    let raw_words = query_words(query);
    let split: Vec<(Vec<&str>, Option<&str>)> = raw_words
        .iter()
        .map(|raw| {
            let parts = split_identifier(raw);
            let whole = (parts.len() > 1).then_some(*raw);
            (parts, whole)
        })
        .collect();
    let any_content = split
        .iter()
        .any(|(parts, _)| parts.iter().any(|p| !is_stop_word(p)));

    let mut and_groups: Vec<String> = Vec::new();
    let mut or_terms: Vec<String> = Vec::new();
    for (parts, whole) in split {
        let parts: Vec<String> = parts
            .into_iter()
            .filter(|p| !any_content || !is_stop_word(p))
            .map(fts5_term)
            .collect();
        let whole = whole.map(fts5_term);
        let group = match (parts.is_empty(), whole.as_deref()) {
            (true, None) => continue,
            (true, Some(w)) => w.to_string(),
            (false, None) => parts.join(" "),
            (false, Some(w)) => format!("(({}) OR {w})", parts.join(" ")),
        };
        and_groups.push(group);
        or_terms.extend(parts);
        or_terms.extend(whole);
    }

    if and_groups.is_empty() {
        return (String::new(), String::new());
    }

    let mut and_query = and_groups.join(" AND ");
    if (2..=3).contains(&raw_words.len()) {
        let all: String = raw_words.concat();
        if all.len() <= 64 {
            let all = fts5_term(&all);
            and_query = format!("({and_query}) OR {all}");
            or_terms.push(all);
        }
    }
    (and_query, or_terms.join(" OR "))
}

const STOP_WORDS: &[&str] = &[
    "a", "about", "after", "again", "all", "already", "also", "always", "an", "and", "another",
    "any", "are", "as", "at", "be", "because", "been", "before", "being", "between", "both", "but",
    "by", "can", "could", "did", "do", "does", "each", "either", "else", "ever", "every", "for",
    "from", "had", "has", "have", "here", "how", "if", "in", "instead", "is", "it", "its",
    "itself", "just", "many", "may", "might", "more", "most", "much", "must", "neither", "never",
    "no", "nor", "not", "of", "on", "once", "one", "only", "or", "other", "our", "per", "rather",
    "same", "should", "since", "so", "some", "still", "such", "than", "that", "the", "their",
    "them", "then", "there", "these", "they", "this", "those", "through", "to", "too", "two",
    "until", "very", "via", "was", "we", "were", "what", "when", "where", "whether", "which",
    "while", "who", "whom", "why", "will", "with", "within", "without", "would", "yet", "you",
    "your",
];

fn is_stop_word(word: &str) -> bool {
    STOP_WORDS.contains(&word.to_ascii_lowercase().as_str())
}

/// Splits a query at every character that is not alphanumeric or `_`.
fn query_words(query: &str) -> Vec<&str> {
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .collect()
}

/// Lowercase terms of three or more characters for the trigram name index: every query word
/// and every identifier part of it (`collapse_name` -> `collapse_name`, `collapse`, `name`),
/// deduplicated. Stop words are dropped unless every term is a stop word. Empty when no term
/// qualifies.
fn trigram_name_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in query_words(query) {
        for term in std::iter::once(word).chain(split_identifier(word)) {
            let lower = term.to_lowercase();
            if lower.chars().count() >= 3 && !terms.contains(&lower) {
                terms.push(lower);
            }
        }
    }
    let any_content = terms.iter().any(|t| !is_stop_word(t));
    terms.retain(|t| !any_content || !is_stop_word(t));
    terms
}

/// Quotes one token for FTS5 with a prefix wildcard, except for tokens under three characters.
fn fts5_term(token: &str) -> String {
    if token.chars().count() < 3 {
        format!("\"{token}\"")
    } else {
        format!("\"{token}\"*")
    }
}

/// FTS5 query for a symbol name as typed: no case splitting, no stop words.
/// `isReady` -> `"isReady"*`, so related-test lookup stays as strict as the name.
fn name_prefix_query(name: &str) -> String {
    name.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| format!("\"{s}\"*"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Splits one identifier into words at `_`, digit runs, and case boundaries.
/// `parseHTTPResponse2` -> `["parse", "HTTP", "Response", "2"]`.
fn split_identifier(word: &str) -> Vec<&str> {
    let mut out = Vec::new();
    split_identifier_into(word, &mut out);
    out
}

/// `split_identifier` that appends to a caller-owned vector, so tokenizing a whole doc
/// comment costs one allocation instead of one per word.
fn split_identifier_into<'a>(word: &'a str, out: &mut Vec<&'a str>) {
    let mut chars = word.char_indices().peekable();
    let Some((_, first)) = chars.next() else {
        return;
    };
    let mut prev = char_class(first);
    let mut start = 0;
    while let Some((idx, c)) = chars.next() {
        let cur = char_class(c);
        let next = chars.peek().map_or(OTHER, |(_, n)| char_class(*n));
        if identifier_boundary(prev, cur, next) {
            push_piece(out, &word[start..idx]);
            start = idx;
        }
        prev = cur;
    }
    push_piece(out, &word[start..]);
}

const OTHER: u8 = 0;
const UNDERSCORE: u8 = 1;
const UPPER: u8 = 2;
const LOWER: u8 = 3;
const DIGIT: u8 = 4;

fn char_class(c: char) -> u8 {
    if c == '_' {
        UNDERSCORE
    } else if c.is_uppercase() {
        UPPER
    } else if c.is_lowercase() {
        LOWER
    } else if c.is_ascii_digit() {
        DIGIT
    } else {
        OTHER
    }
}

fn byte_class(b: u8) -> u8 {
    match b {
        b'_' => UNDERSCORE,
        b'A'..=b'Z' => UPPER,
        b'a'..=b'z' => LOWER,
        b'0'..=b'9' => DIGIT,
        _ => OTHER,
    }
}

/// The identifier split rule over character classes: `_` on either side, lower/digit to
/// upper, the last upper of an acronym before a lower (`HTTPResponse`), and digit runs.
fn identifier_boundary(prev: u8, cur: u8, next: u8) -> bool {
    cur == UNDERSCORE
        || prev == UNDERSCORE
        || (cur == UPPER && (prev == LOWER || prev == DIGIT))
        || (cur == UPPER && prev == UPPER && next == LOWER)
        || ((cur == DIGIT) != (prev == DIGIT))
}

fn push_piece<'a>(out: &mut Vec<&'a str>, piece: &'a str) {
    if !piece.is_empty() && piece != "_" {
        out.push(piece);
    }
}

/// Tokens of a signature or doc comment: the text split at non-word characters, each word
/// split like an identifier. ASCII text is walked byte by byte in one pass; other text takes
/// the char path with the same boundary rule.
fn text_tokens_into<'a>(text: &'a str, out: &mut Vec<&'a str>) {
    if !text.is_ascii() {
        for word in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
            split_identifier_into(word, out);
        }
        return;
    }
    let bytes = text.as_bytes();
    let mut start: Option<usize> = None;
    let mut prev = OTHER;
    for (i, &b) in bytes.iter().enumerate() {
        let cur = byte_class(b);
        if cur == OTHER {
            if let Some(s) = start.take() {
                push_piece(out, &text[s..i]);
            }
            continue;
        }
        match start {
            None => start = Some(i),
            Some(s) => {
                let next = bytes.get(i + 1).map_or(OTHER, |n| byte_class(*n));
                if identifier_boundary(prev, cur, next) {
                    push_piece(out, &text[s..i]);
                    start = Some(i);
                }
            }
        }
        prev = cur;
    }
    if let Some(s) = start {
        push_piece(out, &text[s..]);
    }
}

/// One admitted search row with the recall branches that reached it.
/// `result.score` is the word-branch BM25 for word rows and `0.0` otherwise until the
/// rerank replaces it.
pub(crate) struct Candidate {
    pub result: SymbolSearchResult,
    pub bm25: Option<f64>,
    pub exact_name: bool,
    pub word_match: bool,
    pub name_match: bool,
    pub name_terms: Vec<String>,
    pub documentation: bool,
}

fn candidate_columns(conn: &Connection) -> String {
    format!(
        "s.rowid AS row_id, s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind,
                s.signature, s.doc_comment, s.visibility, s.parent_symbol_id, s.start_line,
                s.start_column, s.end_line, s.end_column, s.start_byte, s.end_byte,
                s.body_start_line, s.body_start_column, s.body_end_line, s.body_end_column,
                s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group, s.is_test,
                s.test_container,
                (s.language IN ({doc_langs}) OR NOT ({not_doc})) AS documentation",
        doc_langs = documentation_language_list(),
        not_doc = not_documentation(conn, "s")
    )
}

fn candidate_filters(searching_variables: bool, include_tests: bool) -> String {
    let mut sql = String::from(
        " AND (:kind IS NULL OR s.kind = :kind)
          AND (:path IS NULL OR replace(s.path, '\\', '/') = :path COLLATE NOCASE OR replace(s.path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(s.path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
    );
    if !searching_variables {
        sql.push_str(&format!(" AND NOT {}", local_variable_predicate("s")));
    }
    if !include_tests {
        // Unary `+` keeps the planner off the test-flag indexes: without ANALYZE statistics it
        // would otherwise prefer them over the name index and walk nearly every row.
        sql.push_str(" AND +s.is_test = 0 AND +s.test_container = 0");
        sql.push_str(&format!(" AND NOT {}", test_path_predicate("s")));
    }
    sql
}

/// Runs the word, trigram-name, and exact-name branches with the same filters and merges
/// them by `rowid`. The word branch admits the rows that match every query word first and
/// then fills its cap with rows that match any word, so one full match never hides a
/// better partial one. Word BM25 exists only for word rows and never orders across branches.
/// A `variable` kind filter adds locals and parameters by name, because the FTS tables
/// exclude them.
pub(crate) fn collect_search_candidates(
    conn: &Connection,
    query: &str,
    kind_filter: Option<&str>,
    path_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<Candidate>, QueryError> {
    let (and_q, or_q) = sanitize_fts5_query(query);
    let terms = trigram_name_terms(query);
    let normalized_path = path_filter.map(|p| {
        p.replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    });
    let escaped_path = normalized_path.as_deref().map(escape_like);
    let norm_kind = kind_filter.map(normalize_kind);
    let searching_variables = norm_kind.as_deref() == Some("variable");
    let path_val = normalized_path.as_deref();
    let path_like = escaped_path.as_deref();
    let kind_val = norm_kind.as_deref();
    let columns = candidate_columns(conn);
    let filters = candidate_filters(searching_variables, include_tests);
    let word_cap = (limit * 4).clamp(40, 160);
    let name_cap = (limit * 2).clamp(20, 40);

    let new_candidate = |row: &Row| -> rusqlite::Result<(i64, Candidate)> {
        let symbol = map_symbol(row)?;
        let lower_name = symbol.name.to_lowercase();
        let name_terms = terms
            .iter()
            .filter(|t| lower_name.contains(t.as_str()))
            .cloned()
            .collect();
        let candidate = Candidate {
            result: SymbolSearchResult {
                symbol,
                score: 0.0,
                snippet: None,
                explain: None,
            },
            bm25: None,
            exact_name: false,
            word_match: false,
            name_match: false,
            name_terms,
            documentation: row.get::<_, Option<i64>>("documentation")? == Some(1),
        };
        Ok((row.get("row_id")?, candidate))
    };

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut by_rowid: HashMap<i64, usize> = HashMap::new();
    let mut admit = |rowid: i64, incoming: Candidate| match by_rowid.get(&rowid).copied() {
        Some(i) => {
            let existing = &mut candidates[i];
            existing.exact_name |= incoming.exact_name;
            existing.word_match |= incoming.word_match;
            existing.name_match |= incoming.name_match;
            if incoming.bm25.is_some() && existing.bm25.is_none() {
                existing.bm25 = incoming.bm25;
                existing.result = incoming.result;
            }
        }
        None => {
            by_rowid.insert(rowid, candidates.len());
            candidates.push(incoming);
        }
    };

    let has_trigram = has_table(conn, "symbol_names_tri");
    let exact_query = query.trim();
    let exact_phrase = format!("\"{}\"", exact_query.replace('"', "\"\""));
    let exact_via_trigram = has_trigram && exact_query.chars().count() >= 3;
    let exact_sql = if exact_via_trigram {
        format!(
            "SELECT {columns} FROM symbol_names_tri
             CROSS JOIN symbols s ON s.rowid = symbol_names_tri.rowid
             WHERE symbol_names_tri MATCH :exact AND length(s.name) = length(:query) {filters}
             ORDER BY s.path ASC, s.start_line ASC LIMIT {MAX_RESULT_LIMIT}"
        )
    } else {
        format!(
            "SELECT {columns} FROM symbols s WHERE s.name = :query {filters}
             ORDER BY s.path ASC, s.start_line ASC LIMIT {MAX_RESULT_LIMIT}"
        )
    };
    let mut exact_params: Vec<(&str, &dyn ToSql)> = vec![
        (":query", &exact_query),
        (":kind", &kind_val),
        (":path", &path_val),
        (":path_like", &path_like),
    ];
    if exact_via_trigram {
        exact_params.push((":exact", &exact_phrase));
    }
    let exact_rows = conn
        .prepare(&exact_sql)?
        .query_map(exact_params.as_slice(), new_candidate)?
        .collect::<Result<Vec<_>, _>>()?;
    for (rowid, mut candidate) in exact_rows {
        candidate.exact_name = true;
        admit(rowid, candidate);
    }

    if searching_variables {
        let pattern = format!("%{}%", escape_like(exact_query));
        let local_sql = format!(
            "SELECT {columns} FROM symbols s
             WHERE {local} AND (s.name = :query OR s.name LIKE :pattern ESCAPE '\\') {filters}
             ORDER BY (s.name = :query) DESC, length(s.name) ASC, s.path ASC LIMIT {limit}",
            local = local_variable_predicate("s")
        );
        let local_rows = conn
            .prepare(&local_sql)?
            .query_map(
                rusqlite::named_params! {
                    ":query": exact_query,
                    ":pattern": pattern,
                    ":kind": kind_val,
                    ":path": path_val,
                    ":path_like": path_like,
                },
                new_candidate,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        for (rowid, mut candidate) in local_rows {
            candidate.exact_name = candidate.result.symbol.name == exact_query;
            candidate.name_match = true;
            admit(rowid, candidate);
        }
    }

    let word_sql = format!(
        "SELECT {columns},
                bm25(symbols_fts, 10.0, 5.0, 1.0) AS rank_score,
                snippet(symbols_fts, 2, '[', ']', '...', 12) AS doc_snippet,
                snippet(symbols_fts, 1, '[', ']', '...', 12) AS sig_snippet,
                snippet(symbols_fts, 0, '[', ']', '...', 12) AS name_snippet
         FROM symbols_fts
         CROSS JOIN symbols s ON s.rowid = symbols_fts.rowid
         WHERE symbols_fts MATCH :match {filters}
         ORDER BY (s.kind = 'import') ASC, (s.language IN ({doc_langs})) ASC, {not_doc} DESC, (s.name = :query COLLATE NOCASE) DESC, rank_score ASC LIMIT {word_cap}",
        doc_langs = documentation_language_list(),
        not_doc = not_documentation(conn, "s")
    );
    let word_rows = |match_clause: &str| -> Result<Vec<(i64, Candidate)>, QueryError> {
        let map_fn = |row: &Row| -> rusqlite::Result<(i64, Candidate)> {
            let (rowid, mut candidate) = new_candidate(row)?;
            let score: f64 = row.get("rank_score")?;
            let doc_snip: Option<String> = row.get("doc_snippet").ok();
            let sig_snip: Option<String> = row.get("sig_snippet").ok();
            let name_snip: Option<String> = row.get("name_snippet").ok();
            let highlighted = |s: &Option<String>| s.as_ref().is_some_and(|s| s.contains('['));
            candidate.result.snippet = if highlighted(&doc_snip) {
                doc_snip
            } else if highlighted(&sig_snip) {
                sig_snip
            } else if highlighted(&name_snip) {
                name_snip
            } else {
                doc_snip.or(sig_snip).or(name_snip)
            };
            candidate.result.score = score;
            candidate.bm25 = Some(score);
            candidate.word_match = true;
            Ok((rowid, candidate))
        };
        Ok(conn
            .prepare(&word_sql)?
            .query_map(
                rusqlite::named_params! {
                    ":match": match_clause,
                    ":query": query.trim(),
                    ":kind": kind_val,
                    ":path": path_val,
                    ":path_like": path_like,
                },
                map_fn,
            )?
            .collect::<Result<Vec<_>, _>>()?)
    };
    if !and_q.is_empty() {
        let and_rows = word_rows(&and_q)?;
        let mut word_admitted: HashSet<i64> = and_rows.iter().map(|(rowid, _)| *rowid).collect();
        for (rowid, candidate) in and_rows {
            admit(rowid, candidate);
        }
        if and_q != or_q {
            for (rowid, candidate) in word_rows(&or_q)? {
                if word_admitted.len() >= word_cap && !word_admitted.contains(&rowid) {
                    break;
                }
                word_admitted.insert(rowid);
                admit(rowid, candidate);
            }
        }
    }

    if !terms.is_empty() && has_trigram {
        let match_clause = terms
            .iter()
            .map(|t| format!("\"{t}\""))
            .collect::<Vec<_>>()
            .join(" OR ");
        let name_sql = format!(
            "SELECT {columns} FROM symbol_names_tri
             CROSS JOIN symbols s ON s.rowid = symbol_names_tri.rowid
             WHERE symbol_names_tri MATCH :match {filters}
             ORDER BY bm25(symbol_names_tri) ASC, length(s.name) ASC, s.path ASC LIMIT {name_cap}"
        );
        let name_rows = conn
            .prepare(&name_sql)?
            .query_map(
                rusqlite::named_params! {
                    ":match": match_clause,
                    ":kind": kind_val,
                    ":path": path_val,
                    ":path_like": path_like,
                },
                new_candidate,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        for (rowid, mut candidate) in name_rows {
            candidate.name_match = true;
            admit(rowid, candidate);
        }
    }

    Ok(candidates)
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
    fts_search_symbols_explained(
        conn,
        query,
        kind_filter,
        path_filter,
        include_tests,
        limit,
        false,
    )
}

/// Conceptual full-text search that also attaches the rerank breakdown to every row when
/// `explain` is true. Without it, `explain` stays `None` on every row.
pub fn fts_search_symbols_explained(
    conn: &Connection,
    query: &str,
    kind_filter: Option<&str>,
    path_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
    explain: bool,
) -> Result<Vec<SymbolSearchResult>, QueryError> {
    validate_result_limit(limit)?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let (and_q, _) = sanitize_fts5_query(query);
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
    let escaped_path = normalized_path.as_deref().map(escape_like);
    let searching_variables = norm_kind.as_deref() == Some("variable");

    let name_search = |local_clause: &str| -> Result<Vec<SymbolSearchResult>, QueryError> {
        let pattern = format!("%{}%", escape_like(query));
        let mut sql = String::from(
            "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                    visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                    start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                    body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                    is_test, test_container
              FROM symbols s
              WHERE (name = :query OR name LIKE :pattern ESCAPE '\\')
                AND (:kind IS NULL OR kind = :kind)
                AND (:path IS NULL OR replace(path, '\\', '/') = :path COLLATE NOCASE OR replace(path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
        );
        sql.push_str(local_clause);
        if !include_tests {
            sql.push_str(" AND is_test = 0 AND test_container = 0");
            sql.push_str(&format!(" AND NOT {}", test_path_predicate("s")));
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

        Ok(rows
            .into_iter()
            .map(|s| SymbolSearchResult {
                symbol: s,
                score: 0.0,
                snippet: None,
                explain: None,
            })
            .collect())
    };

    if !has_table(conn, "symbols_fts") {
        let local_clause = if searching_variables {
            String::new()
        } else {
            format!(" AND NOT {}", local_variable_predicate("s"))
        };
        return name_search(&local_clause);
    }

    let candidates =
        collect_search_candidates(conn, query, kind_filter, path_filter, include_tests, limit)?;
    let candidate_count = candidates.len();
    let started = std::time::Instant::now();
    let idf = idf_weights(conn, &rerank_words(query));
    let ranked = rerank_with(candidates, query, include_tests, Some(&idf));
    let rerank_us = started.elapsed().as_micros();
    Ok(ranked
        .into_iter()
        .take(limit)
        .map(|(mut result, mut breakdown)| {
            if explain {
                breakdown.candidates = candidate_count;
                breakdown.rerank_us = rerank_us;
                result.explain = Some(breakdown);
            }
            result
        })
        .collect())
}

const W_NAME_WHOLE: f64 = 100.0;
const W_NAME_ALL_WORDS: f64 = 60.0;
const W_KIND_DEFINITION: f64 = 4.0;
const W_KIND_MEMBER: f64 = 0.0;
const W_KIND_IMPORT: f64 = -50.0;
const W_PATH_ROLE: f64 = -10.0;
const W_DOCUMENTATION_ROW: f64 = -200.0;
const W_TEST_INTENT: f64 = 5.0;
const W_TERMS: f64 = 52.0;
const MAX_TERM_CREDIT: f64 = 3.0;
const TEXT_CREDIT: f64 = 1.0;
const TEXT_HEAD_BYTES: usize = 400;

/// Symbol kinds that define a body: the kinds a touched-symbol or ranking rule prefers over locals.
pub const DEFINITION_KINDS: &[&str] = &[
    "function",
    "method",
    "class",
    "struct",
    "trait",
    "interface",
    "enum",
    "type",
];
const MEMBER_KINDS: &[&str] = &["enum_member", "field", "property", "constant", "variable"];
const DEMOTED_PATH_SEGMENTS: &[&str] = &["scripts", "examples", "benchmarks", "fixtures", "vendor"];
const TEST_INTENT_WORDS: &[&str] = &["test", "tests", "spec", "specs"];

struct QueryWord {
    word: String,
    stem: String,
}

/// Per-word hits of one candidate: which query words its name, signature, and capped doc
/// cover. The name carries a match strength per word, the others a plain hit.
struct Hits {
    name: Vec<u8>,
    signature: Vec<bool>,
    doc: Vec<bool>,
}

/// Symbols a query word may be counted in before it is called common: the count walks the
/// word's postings, so the cap bounds the cost of a word held by most of a million symbols.
const DF_CAP: usize = 20_000;

/// Global rarity of each lowercase rerank term: `ln(1 + N / (df + 1))`, where `df` is the
/// number of indexed symbols holding the term (capped at `DF_CAP`) and `N` an upper bound on
/// the index size. The count is an FTS5 `MATCH`, so the term is stemmed by the index's own
/// tokenizer and `news` finds the rows indexed as `new`.
fn idf_weights(conn: &Connection, words: &[String]) -> Vec<f64> {
    let n = conn
        .query_row("SELECT max(rowid) FROM symbols", [], |r| {
            r.get::<_, Option<i64>>(0)
        })
        .ok()
        .flatten()
        .unwrap_or(0) as f64;
    let mut count = conn
        .prepare(&format!(
            "SELECT count(*) FROM (SELECT rowid FROM symbols_fts WHERE symbols_fts MATCH ?1 LIMIT {DF_CAP})"
        ))
        .ok();
    words
        .iter()
        .map(|word| {
            let df = count
                .as_mut()
                .and_then(|stmt| document_frequency(stmt, &format!("\"{word}\"")))
                .unwrap_or(0);
            (1.0 + n / (df as f64 + 1.0)).ln()
        })
        .collect()
}

fn document_frequency(stmt: &mut rusqlite::Statement<'_>, term: &str) -> Option<i64> {
    stmt.query_row([term], |r| r.get(0)).ok()
}

/// The field that credits each query term and the credit it is worth: a name whole token 3,
/// a name stem 2, a signature or doc hit `TEXT_CREDIT`, a name substring 1, nothing 0.
fn term_credits(hits: &Hits, words: &[QueryWord]) -> Vec<(String, String, f64)> {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let (field, credit) = match hits.name[i] {
                3 => ("name", 3.0),
                2 => ("name", 2.0),
                _ if hits.signature[i] => ("signature", TEXT_CREDIT),
                _ if hits.doc[i] => ("doc", TEXT_CREDIT),
                1 => ("name", 1.0),
                _ => ("none", 0.0),
            };
            (w.word.clone(), field.to_string(), credit)
        })
        .collect()
}

fn term_score(terms: &[(String, String, f64)], weights: &[f64]) -> f64 {
    let total: f64 = weights.iter().sum();
    if total == 0.0 {
        return 0.0;
    }
    let credited: f64 = terms
        .iter()
        .zip(weights)
        .map(|((_, _, credit), weight)| credit * weight)
        .sum();
    W_TERMS * credited / (MAX_TERM_CREDIT * total)
}

/// Lowercase query words for the rerank: every `query_words` token is split like an
/// identifier, and stop words are dropped only when a content word remains.
fn rerank_words(query: &str) -> Vec<String> {
    let words: Vec<String> = query_words(query)
        .into_iter()
        .flat_map(split_identifier)
        .map(str::to_lowercase)
        .collect();
    let any_content = words.iter().any(|w| !is_stop_word(w));
    words
        .into_iter()
        .filter(|w| !any_content || !is_stop_word(w))
        .collect()
}

fn collapse(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn head_bytes(text: &str, bytes: usize) -> &str {
    let mut end = bytes.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn token_run_equals(tokens: &[String], word: &str) -> bool {
    (0..tokens.len()).any(|start| {
        let mut joined = String::new();
        for token in &tokens[start..] {
            joined.push_str(token);
            if joined.len() >= word.len() {
                return joined == word;
            }
        }
        false
    })
}

/// Match strength of a name per query word: `3` when a run of its tokens equals the word,
/// `2` when a token stem equals the word stem, `1` when the collapsed name contains the word,
/// `0` when nothing matches.
fn name_hits(name: &str, words: &[QueryWord], stemmer: &Stemmer) -> Vec<u8> {
    let tokens: Vec<String> = split_identifier(name)
        .into_iter()
        .map(str::to_lowercase)
        .collect();
    let stems: Vec<String> = tokens
        .iter()
        .map(|t| stemmer.stem(t).into_owned())
        .collect();
    let collapsed = collapse(name);
    words
        .iter()
        .map(|w| {
            if token_run_equals(&tokens, &w.word) {
                3
            } else if stems.contains(&w.stem) {
                2
            } else if w.word.chars().count() >= 3 && collapsed.contains(&w.word) {
                1
            } else {
                0
            }
        })
        .collect()
}

/// True when `token` lowercased starts with `prefix` (or equals it when `exact`).
/// `prefix` is already lowercase.
fn lowercase_prefix_match(token: &str, prefix: &str, exact: bool) -> bool {
    if token.is_ascii() && prefix.is_ascii() {
        let Some(head) = token.as_bytes().get(..prefix.len()) else {
            return false;
        };
        return head.eq_ignore_ascii_case(prefix.as_bytes())
            && (!exact || token.len() == prefix.len());
    }
    let mut lower = token.chars().flat_map(char::to_lowercase);
    for expected in prefix.chars() {
        if lower.next() != Some(expected) {
            return false;
        }
    }
    !exact || lower.next().is_none()
}

/// Token-level coverage of a signature or doc: a word is covered when some token equals it,
/// or starts with its stem or with the word itself (three or more characters), so `stemming`
/// covers `stemmer` and `stems` but not `system`.
fn text_hits<'a>(
    text: Option<&'a str>,
    words: &[QueryWord],
    tokens: &mut Vec<&'a str>,
) -> Vec<bool> {
    tokens.clear();
    text_tokens_into(text.unwrap_or(""), tokens);
    words
        .iter()
        .map(|w| {
            let stem_prefix = w.stem.chars().count() >= 3;
            let exact_word = w.word.chars().count() < 3;
            tokens.iter().any(|t| {
                lowercase_prefix_match(t, &w.word, exact_word)
                    || (stem_prefix && lowercase_prefix_match(t, &w.stem, false))
            })
        })
        .collect()
}

fn kind_prior(kind: &str) -> f64 {
    let kind = normalize_kind(kind);
    match kind.as_str() {
        "import" => W_KIND_IMPORT,
        k if DEFINITION_KINDS.contains(&k) => W_KIND_DEFINITION,
        k if MEMBER_KINDS.contains(&k) => W_KIND_MEMBER,
        _ => 0.0,
    }
}

fn path_role(path: &str, words: &[QueryWord], stemmer: &Stemmer) -> f64 {
    let Some(segment) = path.split(['/', '\\']).find(|seg| {
        DEMOTED_PATH_SEGMENTS
            .iter()
            .any(|d| d.eq_ignore_ascii_case(seg))
    }) else {
        return 0.0;
    };
    let segment = segment.to_lowercase();
    let segment_stem = stemmer.stem(&segment);
    let named = words.iter().any(|w| {
        w.word == segment || w.word == segment_stem || w.stem == segment || w.stem == segment_stem
    });
    if named { 0.0 } else { W_PATH_ROLE }
}

fn bracket_longest_term(name: &str, terms: &[String]) -> String {
    let lower = name.to_lowercase();
    if lower.len() != name.len() {
        return name.to_string();
    }
    let mut best: Option<(usize, usize)> = None;
    for term in terms {
        if let Some(start) = lower.find(term.as_str()) {
            let end = start + term.len();
            let longer = best.is_none_or(|(s, e)| end - start > e - s);
            if longer && name.is_char_boundary(start) && name.is_char_boundary(end) {
                best = Some((start, end));
            }
        }
    }
    match best {
        Some((start, end)) => {
            format!("{}[{}]{}", &name[..start], &name[start..end], &name[end..])
        }
        None => name.to_string(),
    }
}

fn branch_snippet(candidate: &Candidate) -> Option<String> {
    let name = &candidate.result.symbol.name;
    if candidate.word_match {
        candidate.result.snippet.clone()
    } else if candidate.exact_name {
        Some(name.clone())
    } else {
        Some(bracket_longest_term(name, &candidate.name_terms))
    }
}

/// Scores every admitted candidate and returns them best first. Each query term is credited
/// once from its strongest field and weighted by `idf`, the term's rarity across the index;
/// a weight vector of the wrong length, or none, makes every term count the same. Ties fall
/// to name strength, then word BM25 (rows without one last), then name length, path, name.
fn rerank_with(
    candidates: Vec<Candidate>,
    query: &str,
    include_tests: bool,
    idf: Option<&[f64]>,
) -> Vec<(SymbolSearchResult, SearchExplain)> {
    let stemmer = Stemmer::create(Algorithm::English);
    let words: Vec<QueryWord> = rerank_words(query)
        .into_iter()
        .map(|word| QueryWord {
            stem: stemmer.stem(&word).into_owned(),
            word,
        })
        .collect();
    let collapsed_query = collapse(query);
    let test_intent = include_tests
        && words
            .iter()
            .any(|w| TEST_INTENT_WORDS.contains(&w.word.as_str()));

    let mut tokens: Vec<&str> = Vec::new();
    let hits: Vec<Hits> = candidates
        .iter()
        .map(|candidate| {
            let symbol = &candidate.result.symbol;
            Hits {
                name: name_hits(&symbol.name, &words, &stemmer),
                signature: text_hits(
                    symbol
                        .signature
                        .as_deref()
                        .map(|signature| head_bytes(signature, TEXT_HEAD_BYTES)),
                    &words,
                    &mut tokens,
                ),
                doc: text_hits(
                    symbol
                        .doc_comment
                        .as_deref()
                        .map(|doc| head_bytes(doc, TEXT_HEAD_BYTES)),
                    &words,
                    &mut tokens,
                ),
            }
        })
        .collect();
    let term_weights: Vec<f64> = match idf {
        Some(weights) if weights.len() == words.len() => weights.to_vec(),
        _ => vec![1.0; words.len()],
    };
    let word_weights: Vec<(String, f64)> = words
        .iter()
        .zip(&term_weights)
        .map(|(w, weight)| (w.word.clone(), *weight))
        .collect();

    let mut scored: Vec<(SymbolSearchResult, SearchExplain)> = candidates
        .into_iter()
        .zip(hits)
        .map(|(candidate, hits)| {
            let symbol = &candidate.result.symbol;
            let name_strength: u32 = hits.name.iter().map(|s| u32::from(*s)).sum();
            let tier = if !collapsed_query.is_empty() && collapse(&symbol.name) == collapsed_query {
                "whole"
            } else if !hits.name.is_empty() && hits.name.iter().all(|s| *s >= 2) {
                "all"
            } else if hits.name.iter().any(|s| *s > 0) {
                "partial"
            } else {
                "none"
            };
            let terms = term_credits(&hits, &words);
            let explain = SearchExplain {
                bm25: candidate.bm25,
                branches: [
                    (candidate.exact_name, "exact"),
                    (candidate.word_match, "word"),
                    (candidate.name_match, "name"),
                ]
                .into_iter()
                .filter(|(hit, _)| *hit)
                .map(|(_, branch)| branch.to_string())
                .collect(),
                name_tier: tier.to_string(),
                name_strength,
                term_score: term_score(&terms, &term_weights),
                name_bonus: match tier {
                    "whole"
                        if DEFINITION_KINDS.contains(&normalize_kind(&symbol.kind).as_str()) =>
                    {
                        W_NAME_WHOLE
                    }
                    "whole" | "all" => W_NAME_ALL_WORDS,
                    _ => 0.0,
                },
                kind_prior: kind_prior(&symbol.kind),
                path_role: path_role(&symbol.path, &words, &stemmer),
                documentation: if candidate.documentation {
                    W_DOCUMENTATION_ROW
                } else {
                    0.0
                },
                test_intent: if test_intent && (symbol.is_test || symbol.test_container) {
                    W_TEST_INTENT
                } else {
                    0.0
                },
                terms,
                word_weights: word_weights.clone(),
                candidates: 0,
                rerank_us: 0,
            };
            let score = explain.term_score
                + explain.name_bonus
                + explain.kind_prior
                + explain.path_role
                + explain.documentation
                + explain.test_intent;
            let snippet = branch_snippet(&candidate);
            let mut result = candidate.result;
            result.score = score;
            result.snippet = snippet;
            (result, explain)
        })
        .collect();

    scored.sort_by(|(a, ea), (b, eb)| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| eb.name_strength.cmp(&ea.name_strength))
            .then_with(|| {
                a.symbol
                    .name
                    .starts_with('_')
                    .cmp(&b.symbol.name.starts_with('_'))
            })
            .then_with(|| ea.bm25.is_none().cmp(&eb.bm25.is_none()))
            .then_with(|| ea.bm25.unwrap_or(0.0).total_cmp(&eb.bm25.unwrap_or(0.0)))
            .then_with(|| a.symbol.name.len().cmp(&b.symbol.name.len()))
            .then_with(|| a.symbol.path.cmp(&b.symbol.path))
            .then_with(|| a.symbol.name.cmp(&b.symbol.name))
    });
    scored
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

    const COLUMNS: &str = "s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
            s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
            s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
            s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
            s.is_test, s.test_container";
    const IS_TEST: &str = "(s.is_test = 1 OR s.test_container = 1)";
    let not_documentation = not_documentation(conn, "s");

    let mut tests = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();

    let callers_sql = format!(
        "SELECT {COLUMNS}
     FROM symbols s
     JOIN relationships r ON r.from_symbol_id = s.symbol_id
     WHERE r.to_symbol_id = ?1 AND {IS_TEST} AND {not_documentation}
     LIMIT ?2"
    );

    if let Ok(mut stmt) = conn.prepare(&callers_sql)
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

    // julie resolves call edges inside one file only; every cross-file caller is a pending edge
    let remaining = limit - tests.len();
    if remaining > 0 && has_pending_namespace_column(conn) {
        let pending_sql = format!(
            "SELECT DISTINCT {COLUMNS}
     FROM pending_relationships p
     JOIN symbols s ON p.from_symbol_id = s.symbol_id
     JOIN symbols s_from ON s_from.symbol_id = s.symbol_id
     JOIN symbols s_target ON s_target.symbol_id = ?1
     LEFT JOIN symbols s_target_parent ON s_target.parent_symbol_id = s_target_parent.symbol_id
     WHERE p.target_terminal_name = s_target.name
       AND {IS_TEST}
       AND {not_documentation}
       AND {pred}
     LIMIT ?2",
            pred = pending_target_predicate("s_target", "s_target_parent")
        );

        if let Ok(mut stmt) = conn.prepare(&pending_sql)
            && let Ok(rows) = stmt.query_map(
                params![target_symbol.symbol_id, remaining as i64],
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
    }

    let remaining = limit - tests.len();
    let name_sql = format!(
        "SELECT {COLUMNS}
     FROM symbols s
     WHERE {IS_TEST}
       AND {not_documentation}
       AND (s.name LIKE '%' || ?1 || '%' OR s.signature LIKE '%' || ?1 || '%')
     ORDER BY (s.name LIKE '%' || ?1 || '%') DESC
     LIMIT ?2"
    );

    if let Ok(mut stmt) = conn.prepare(&name_sql)
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

    let remaining = limit - tests.len();
    let fts_exists: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='symbols_fts'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);

    if remaining > 0 && fts_exists {
        let fts_sql = format!(
            "SELECT {COLUMNS}
         FROM symbols_fts
         CROSS JOIN symbols s ON s.rowid = symbols_fts.rowid
         WHERE symbols_fts MATCH ?1 AND {IS_TEST} AND {not_documentation}
         LIMIT ?2"
        );

        let and_q = name_prefix_query(&target_symbol.name);
        if !and_q.is_empty()
            && let Ok(mut stmt) = conn.prepare(&fts_sql)
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

/// Split `Outer::Inner::run` or `Outer.Inner.run` into its ancestor segments, outermost first, and the terminal name.
fn split_qualified_name(name: &str) -> (Vec<&str>, &str) {
    let separator = if name.contains("::") {
        "::"
    } else if name.contains('.') {
        "."
    } else {
        return (Vec::new(), name);
    };
    let mut segments: Vec<&str> = name.split(separator).collect();
    let terminal = segments.pop().unwrap_or(name);
    (segments, terminal)
}

/// Names of the parents of `symbol_id`, innermost first.
fn ancestor_names(conn: &Connection, symbol_id: &str) -> Result<Vec<String>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT p.symbol_id, p.name FROM symbols s
         JOIN symbols p ON s.parent_symbol_id = p.symbol_id
         WHERE s.symbol_id = ?1",
    )?;
    let mut names = Vec::new();
    let mut current = symbol_id.to_string();
    for _ in 0..32 {
        let mut rows = stmt.query(params![current])?;
        let Some(row) = rows.next()? else { break };
        let parent_id: String = row.get(0)?;
        let parent_name: String = row.get(1)?;
        names.push(parent_name);
        current = parent_id;
    }
    Ok(names)
}

/// A qualified name with several ancestor segments is filtered in Rust after the SQL, so the SQL
/// must not truncate the candidate set the way the 25-row cap does for plain and one-segment names.
const ANCESTOR_WALK_ROW_CAP: i64 = 2000;

fn chain_contains(chain: &[String], wanted: &[&str]) -> bool {
    let mut remaining = chain.iter();
    wanted
        .iter()
        .all(|segment| remaining.any(|name| name == segment))
}

fn get_symbol_by_name_internal(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
    exact_path: bool,
) -> Result<Option<Symbol>, QueryError> {
    let (ancestor_segments, terminal_name) = split_qualified_name(name);
    let parent_name = ancestor_segments.last().copied();

    let sql = "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
                s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
                s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
                s.is_test, s.test_container
         FROM symbols s
         LEFT JOIN symbols p ON s.parent_symbol_id = p.symbol_id
         WHERE (s.name = :name OR (s.name = :term AND (:parent IS NULL OR p.name = :parent)))
           AND (:path IS NULL OR s.path = :path COLLATE NOCASE OR s.path = :path_bs COLLATE NOCASE OR (:exact = 0 AND (s.path LIKE '%/' || :path_like ESCAPE '\\' OR s.path LIKE '%\\\\' || :path_like_bs ESCAPE '\\' OR s.path LIKE :path_like || '/%' ESCAPE '\\' OR s.path LIKE :path_like_bs || '\\\\%' ESCAPE '\\')))
         ORDER BY (s.kind != 'import') DESC,
                  (s.kind IN ('function', 'struct', 'class', 'trait', 'method', 'enum', 'interface', 'type')) DESC,
                  (s.name = :name) DESC,
                  (:path IS NOT NULL AND (s.path = :path COLLATE NOCASE OR s.path = :path_bs COLLATE NOCASE)) DESC,
                  s.is_test ASC
         LIMIT :limit";

    let row_cap: i64 = if ancestor_segments.len() > 1 {
        ANCESTOR_WALK_ROW_CAP
    } else {
        25
    };
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
        ":limit": row_cap,
    })?;

    let mut matches: Vec<Symbol> = Vec::new();
    while let Some(row) = rows.next()? {
        matches.push(map_symbol(row)?);
    }
    drop(rows);

    if ancestor_segments.len() > 1 {
        let required: Vec<&str> = ancestor_segments.iter().rev().copied().collect();
        let mut kept = Vec::with_capacity(matches.len());
        for symbol in matches {
            if symbol.name == name
                || chain_contains(&ancestor_names(conn, &symbol.symbol_id)?, &required)
            {
                kept.push(symbol);
            }
        }
        matches = kept;
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

/// The name of the repository this index was built for, for not-found messages.
pub fn workspace_name(conn: &Connection) -> String {
    conn.query_row(
        "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
        [],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .map(|root| root.replace('\\', "/"))
    .and_then(|root| {
        root.trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    })
    .unwrap_or_else(|| "this workspace".to_string())
}

fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for (i, a) in left.iter().enumerate() {
        current[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let substitution = previous[j] + usize::from(a != b);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

/// Names within a small edit distance of `name`, recalled through the name trigram index.
fn near_names_by_trigram(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> Result<Vec<Symbol>, QueryError> {
    if !has_table(conn, "symbol_names_tri") {
        return Ok(Vec::new());
    }
    let lower = name.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let chunks: Vec<String> = chars
        .windows(3)
        .map(|window| window.iter().collect::<String>())
        .filter(|chunk| chunk.chars().all(|c| c.is_alphanumeric() || c == '_'))
        .collect();
    if chunks.is_empty() {
        return Ok(Vec::new());
    }
    let match_clause = chunks
        .iter()
        .map(|chunk| format!("\"{chunk}\""))
        .collect::<Vec<_>>()
        .join(" OR ");

    let normalized_path = path_filter.map(|p| {
        p.replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    });
    let escaped_path = normalized_path.as_deref().map(escape_like);
    let kind_val: Option<&str> = None;
    let columns = candidate_columns(conn);
    let filters = candidate_filters(false, false);
    let sql = format!(
        "SELECT {columns} FROM symbol_names_tri
         CROSS JOIN symbols s ON s.rowid = symbol_names_tri.rowid
         WHERE symbol_names_tri MATCH :match {filters}
         ORDER BY bm25(symbol_names_tri) ASC, length(s.name) ASC, s.path ASC LIMIT 20"
    );
    let rows = conn
        .prepare(&sql)?
        .query_map(
            rusqlite::named_params! {
                ":match": match_clause,
                ":kind": kind_val,
                ":path": normalized_path.as_deref(),
                ":path_like": escaped_path.as_deref(),
            },
            map_symbol,
        )?
        .collect::<Result<Vec<_>, _>>()?;

    let budget = (chars.len() / 4).max(2);
    let mut scored: Vec<(usize, Symbol)> = rows
        .into_iter()
        .map(|symbol| (edit_distance(&lower, &symbol.name.to_lowercase()), symbol))
        .filter(|(distance, _)| *distance <= budget)
        .collect();
    scored.sort_by_key(|(distance, symbol)| (*distance, symbol.name.chars().count()));
    Ok(scored
        .into_iter()
        .map(|(_, symbol)| symbol)
        .take(3)
        .collect())
}

/// Up to three indexed symbols whose names are close to `name`. Never fails.
pub fn suggest_symbol_names(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> Vec<Symbol> {
    let (ancestors, terminal) = split_qualified_name(name);
    let mut found =
        search_symbols_scoped(conn, terminal, None, path_filter, false, 3).unwrap_or_default();
    if found.is_empty() && terminal.chars().count() >= 4 {
        found = near_names_by_trigram(conn, terminal, path_filter).unwrap_or_default();
    }
    if let Some(parent) = ancestors.last().copied() {
        let mut ranked: Vec<(bool, Symbol)> = found
            .into_iter()
            .map(|symbol| {
                let shares_parent = ancestor_names(conn, &symbol.symbol_id)
                    .unwrap_or_default()
                    .first()
                    .is_some_and(|found_parent| found_parent == parent);
                (!shares_parent, symbol)
            })
            .collect();
        ranked.sort_by_key(|(demoted, _)| *demoted);
        found = ranked.into_iter().map(|(_, symbol)| symbol).collect();
    }
    found.truncate(3);
    found
}

/// Up to three indexed file paths close to `rel_path`. Never fails.
pub fn suggest_file_paths(conn: &Connection, rel_path: &str) -> Vec<String> {
    let wanted = rel_path.replace('\\', "/");
    let wanted = wanted.trim_start_matches("./").trim_matches('/');
    let basename = wanted.rsplit('/').next().unwrap_or(wanted).to_lowercase();
    if basename.is_empty() {
        return Vec::new();
    }
    let stem = basename.split('.').next().unwrap_or(&basename).to_string();
    let segments: Vec<&str> = wanted.split('/').collect();
    let tail = if segments.len() >= 2 {
        segments[segments.len() - 2..].join("/").to_lowercase()
    } else {
        basename.clone()
    };

    let path_expr = "replace(files.path, '\\', '/')";
    let file_name = format!(
        "lower(replace({path_expr}, rtrim({path_expr}, replace({path_expr}, '/', '')), ''))"
    );
    let rules = [
        (format!("{file_name} = :value"), basename.clone()),
        (
            format!("{file_name} LIKE :value ESCAPE '\\'"),
            format!("{}%", escape_like(&stem)),
        ),
        (
            format!("lower({path_expr}) LIKE :value ESCAPE '\\'"),
            format!("%{}", escape_like(&tail)),
        ),
    ];

    for (predicate, value) in rules {
        let sql = format!(
            "SELECT {path_expr} FROM files WHERE {predicate}
             ORDER BY length(files.path) ASC, files.path ASC LIMIT 3"
        );
        let found: Vec<String> = conn
            .prepare(&sql)
            .and_then(|mut stmt| {
                stmt.query_map(rusqlite::named_params! { ":value": value }, |row| {
                    row.get::<_, String>(0)
                })?
                .collect()
            })
            .unwrap_or_default();
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

/// The workspace name and the recovery hint for a symbol that is not indexed.
pub fn symbol_not_found_parts(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> (String, String) {
    let candidates = suggest_symbol_names(conn, name, path_filter);
    let hint = if candidates.is_empty() {
        "No similar name is indexed; check the workspace and spelling.".to_string()
    } else {
        let list = candidates
            .iter()
            .map(|s| format!("  - {} `{}` ({}:{})", s.kind, s.name, s.path, s.start_line))
            .collect::<Vec<_>>()
            .join("\n");
        format!("Did you mean one of:\n{list}")
    };
    (workspace_name(conn), hint)
}

/// The workspace name and the recovery hint for a file that is not indexed.
pub fn file_not_found_parts(conn: &Connection, rel_path: &str) -> (String, String) {
    let candidates = suggest_file_paths(conn, rel_path);
    let hint = if candidates.is_empty() {
        "No similar path is indexed; check the workspace and spelling.".to_string()
    } else {
        let list = candidates
            .iter()
            .map(|path| format!("  - {path}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("Did you mean one of:\n{list}")
    };
    (workspace_name(conn), hint)
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
    validate_result_limit(limit)?;
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
            let (workspace, hint) = symbol_not_found_parts(conn, symbol_name, path_filter);
            Err(QueryError::SymbolNotFound {
                name: symbol_name.to_string(),
                workspace,
                hint,
            })
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

/// SQL expression ranking a candidate path against the call site `p.path`:
/// 2 for the same file, 1 for the same directory, 0 otherwise.
fn call_site_proximity(candidate_path: &str) -> String {
    let normalized = format!("replace({candidate_path}, '\\', '/')");
    let call_site = "replace(p.path, '\\', '/')";
    format!(
        "CASE WHEN {normalized} = {call_site} THEN 2
              WHEN rtrim({normalized}, replace({normalized}, '/', '')) = rtrim({call_site}, replace({call_site}, '/', '')) THEN 1
              ELSE 0 END"
    )
}

/// SQL predicate that decides whether a pending call edge `p` (with caller `s_from`) points at
/// the candidate definition `target` (whose parent symbol is joined as `parent`).
fn pending_target_predicate(target: &str, parent: &str) -> String {
    let ns = "json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)";
    let target_path = format!("('/' || replace({target}.path, '\\', '/'))");
    let like_value = "replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_')";
    let closer_rank = call_site_proximity("closer.path");
    let target_rank = call_site_proximity(&format!("{target}.path"));
    format!(
        "(
            (
                {target}.parent_symbol_id IS NOT NULL
                AND {parent}.name IS NOT NULL
                AND (
                    EXISTS (SELECT 1 FROM {ns} WHERE value = {parent}.name)
                    OR (EXISTS (SELECT 1 FROM {ns} WHERE value = 'Self')
                        AND s_from.parent_symbol_id = {target}.parent_symbol_id)
                    OR (p.target_receiver IS NOT NULL AND p.target_receiver != '' AND {parent}.name = p.target_receiver)
                    OR EXISTS (
                        SELECT 1 FROM symbols receiver
                        JOIN type_facts receiver_type ON receiver_type.symbol_id = receiver.symbol_id
                        WHERE receiver.name = p.target_receiver
                          AND receiver.path = p.path
                          AND receiver_type.resolved_type = {parent}.name
                    )
                )
                AND NOT EXISTS (
                    SELECT 1 FROM {ns}
                    WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super', 'self', 'Self', {parent}.name)
                      AND NOT EXISTS (
                          WITH RECURSIVE ancestor(symbol_id, depth) AS (
                              SELECT {target}.parent_symbol_id, 0
                              UNION ALL
                              SELECT s.parent_symbol_id, ancestor.depth + 1
                              FROM symbols s JOIN ancestor ON s.symbol_id = ancestor.symbol_id
                              WHERE s.parent_symbol_id IS NOT NULL AND ancestor.depth < 32
                          )
                          SELECT 1 FROM ancestor JOIN symbols a ON a.symbol_id = ancestor.symbol_id
                          WHERE a.name = value
                      )
                      AND {target_path} NOT LIKE '%/' || {like_value} || '.%' ESCAPE '\\'
                      AND {target_path} NOT LIKE '%/' || {like_value} || '/%' ESCAPE '\\'
                )
            )
            OR (
                (p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')
                AND (p.target_receiver IS NULL OR p.target_receiver = '')
                AND ({target}.parent_symbol_id IS NULL OR s_from.parent_symbol_id = {target}.parent_symbol_id)
                AND ({target}.parent_symbol_id IS NOT NULL OR NOT EXISTS (
                    SELECT 1 FROM symbols closer
                    WHERE closer.name = {target}.name
                      AND closer.symbol_id != {target}.symbol_id
                      AND closer.parent_symbol_id IS NULL
                      AND closer.kind = {target}.kind
                      AND {closer_rank} > {target_rank}
                ))
            )
            OR (
                {target}.parent_symbol_id IS NULL
                AND EXISTS (
                    SELECT 1 FROM {ns}
                    WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super')
                      AND {target_path} LIKE '%/' || {like_value} || '.%' ESCAPE '\\'
                )
            )
        )"
    )
}

/// SQL predicate excluding rows julie marked as documentation, or the always-true `1 = 1` when
/// the column is absent, because a bare `1` in ORDER BY means the first result column in SQLite.
const DOCUMENTATION_LANGUAGES: &[&str] = &[
    "markdown", "yaml", "toml", "json", "html", "css", "xml", "ini", "text",
];

fn documentation_language_list() -> String {
    DOCUMENTATION_LANGUAGES
        .iter()
        .map(|l| format!("'{l}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn not_documentation(conn: &Connection, alias: &str) -> String {
    let has_content_type: bool = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('symbols') WHERE name = 'content_type'",
            [],
            |_| Ok(true),
        )
        .unwrap_or(false);
    if has_content_type {
        format!("({alias}.content_type IS NULL OR {alias}.content_type != 'documentation')")
    } else {
        "1 = 1".to_string()
    }
}

pub(crate) fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(true),
    )
    .unwrap_or(false)
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
                        &format!("SELECT s_from.name AS from_name,
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
                            AND {pred}
                          LIMIT ?2", pred = pending_target_predicate("s_target", "s_target_parent")),
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

        if results.len() < limit && has_table(conn, "identifiers") {
            let remaining = limit - results.len();
            let mut ident_stmt = conn.prepare(
                "SELECT COALESCE(s.name, ''),
                        COALESCE(i.containing_symbol_id, ''),
                        i.name,
                        i.kind,
                        i.path,
                        i.start_line,
                        i.start_column
                 FROM identifiers i
                 LEFT JOIN symbols s ON i.containing_symbol_id = s.symbol_id
                 WHERE i.name = ?1 AND i.kind IN ('type_usage', 'member_access')
                   AND COALESCE(s.kind, '') != 'import'
                   AND (?3 IS NULL OR NOT EXISTS (
                       SELECT 1 FROM symbols owner
                       JOIN symbols member ON member.parent_symbol_id = owner.symbol_id
                       WHERE owner.name = CASE WHEN json_valid(i.metadata_json) THEN json_extract(i.metadata_json, '$.receiver') END
                         AND member.name = i.name
                         AND owner.name IS NOT (SELECT parent.name FROM symbols target
                                                JOIN symbols parent ON parent.symbol_id = target.parent_symbol_id
                                                WHERE target.symbol_id = ?3)
                   ))
                 ORDER BY i.path, i.start_line
                 LIMIT ?2",
            )?;
            let rows =
                ident_stmt.query_map(params![symbol_name, remaining as i64, symbol_id], |row| {
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
                    String::from("SELECT DISTINCT s_from.name AS from_name,
                            p.from_symbol_id,
                            COALESCE(NULLIF(p.target_display_name, ''), p.target_terminal_name) AS to_name,
                            p.kind,
                            p.path,
                            p.start_line,
                            p.start_column
                     FROM pending_relationships p
                     JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                     WHERE s_from.name = ?1 AND (?3 IS NULL OR p.from_symbol_id = ?3)
                     LIMIT ?2")
                } else {
                    format!("SELECT DISTINCT s_from.name AS from_name,
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
                             AND {pred}
                       )
                     LIMIT ?2", pred = pending_target_predicate("s_to", "s_to_parent"))
                };
                let mut pending_stmt = conn.prepare(&sql)?;
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
                &format!("SELECT DISTINCT s_to.name, s_to.signature, s_to.path, s_to.start_line, s_to.kind
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 JOIN symbols s_to ON s_to.name = p.target_terminal_name
                 LEFT JOIN symbols s_parent ON s_to.parent_symbol_id = s_parent.symbol_id
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                    AND {pred}
                 LIMIT ?3", pred = pending_target_predicate("s_to", "s_parent")),
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
                &format!("SELECT DISTINCT COALESCE(NULLIF(p.target_display_name, ''), p.target_terminal_name), p.path, p.start_line
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM symbols s_to
                       LEFT JOIN symbols s_parent ON s_to.parent_symbol_id = s_parent.symbol_id
                       WHERE s_to.name = p.target_terminal_name
                         AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                         AND {pred}
                   )
                 LIMIT ?3", pred = pending_target_predicate("s_to", "s_parent")),
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

const SQL_FAMILIES: &[&str] = &["sql."];

const ROUTE_FAMILIES: &[&str] = &[
    ".route.",
    ".attribute_route.",
    ".scope_route.",
    ".file_route.",
    ".route_handler.",
    ".route_reference.",
    ".route_group.",
    ".route_prefix.",
    ".resource_route.",
    ".router_mount.",
    ".include_router.",
    ".server_route.",
    ".route_definition.",
];

const CONFIG_FAMILIES: &[&str] = &["toml.", "yaml.", "json.", "env."];

const MODEL_FAMILIES: &[&str] = &[
    "sql.table_definition.v1",
    "sql.column_definition.v1",
    "sql.constraint.v1",
    "sql.foreign_key.v1",
    "sql.index_definition.v1",
    "json.schema.v1",
];

const SIGNAL_FAMILIES: &[&str] = &[".signal_declaration."];

const IMPORT_FAMILIES: &[&str] = &[".import_statement.", ".import."];

const BINDING_FAMILIES: &[&str] = &[".binding."];

const COMPONENT_FAMILIES: &[&str] = &[".object_instantiation.", ".object_type."];

const MODULE_FAMILIES: &[&str] = &[".module."];

const PRAGMA_FAMILIES: &[&str] = &[".pragma."];

/// Category aliases mapped to the pattern-id families they name.
///
/// A rule that starts with a dot matches any pattern id that contains it, a rule that ends with a
/// dot matches any pattern id that starts with it, and any other rule matches one exact id.
pub const CATEGORY_ALIASES: &[(&str, &[&str])] = &[
    ("sql", SQL_FAMILIES),
    ("query", SQL_FAMILIES),
    ("queries", SQL_FAMILIES),
    ("route", ROUTE_FAMILIES),
    ("routes", ROUTE_FAMILIES),
    ("config", CONFIG_FAMILIES),
    ("model", MODEL_FAMILIES),
    ("models", MODEL_FAMILIES),
    ("signal", SIGNAL_FAMILIES),
    ("signals", SIGNAL_FAMILIES),
    ("import", IMPORT_FAMILIES),
    ("imports", IMPORT_FAMILIES),
    ("binding", BINDING_FAMILIES),
    ("bindings", BINDING_FAMILIES),
    ("component", COMPONENT_FAMILIES),
    ("components", COMPONENT_FAMILIES),
    ("module", MODULE_FAMILIES),
    ("modules", MODULE_FAMILIES),
    ("pragma", PRAGMA_FAMILIES),
];

fn category_families(category: &str) -> Option<&'static [&'static str]> {
    let wanted = category.trim().to_ascii_lowercase();
    CATEGORY_ALIASES
        .iter()
        .find(|(alias, _)| *alias == wanted)
        .map(|(_, families)| *families)
}

fn matches_family(pattern_id: &str, rule: &str) -> bool {
    if rule.starts_with('.') {
        pattern_id.contains(rule)
    } else if rule.ends_with('.') {
        pattern_id.starts_with(rule)
    } else {
        pattern_id == rule
    }
}

fn family_clause(column: &str, families: &[&str]) -> String {
    let alternatives: Vec<String> = families
        .iter()
        .map(|rule| {
            let escaped = escape_like(rule);
            if rule.starts_with('.') {
                format!("{column} LIKE '%{escaped}%' ESCAPE '\\'")
            } else if rule.ends_with('.') {
                format!("{column} LIKE '{escaped}%' ESCAPE '\\'")
            } else {
                format!("{column} = '{rule}'")
            }
        })
        .collect();
    format!("({})", alternatives.join(" OR "))
}

/// True when the category text names one of the aliases in `CATEGORY_ALIASES`.
pub fn is_category_alias(category: &str) -> bool {
    category_families(category).is_some()
}

/// Counts the patterns and facts each category alias reaches, given a category listing.
///
/// Only one alias per family list is reported, and an alias with no facts is left out.
pub fn alias_fact_counts(categories: &[(String, usize)]) -> Vec<(&'static str, usize, usize)> {
    let mut reported: Vec<&[&str]> = Vec::new();
    let mut counts = Vec::new();
    for (alias, families) in CATEGORY_ALIASES {
        if reported.contains(families) {
            continue;
        }
        reported.push(families);
        let mut patterns = 0;
        let mut facts = 0;
        for (pattern_id, count) in categories {
            if families.iter().any(|rule| matches_family(pattern_id, rule)) {
                patterns += 1;
                facts += count;
            }
        }
        if facts > 0 {
            counts.push((*alias, patterns, facts));
        }
    }
    counts
}

/// Find structural facts by category (e.g. route, query, model, config), optionally scoped by path.
pub fn find_structural_facts_scoped(
    conn: &Connection,
    category: &str,
    path_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<StructuralFact>, QueryError> {
    validate_result_limit(limit)?;
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

    let cat_clause = match category_families(category) {
        Some(families) => family_clause("sf.pattern_id", families),
        None => {
            "(sf.pattern_id LIKE :cat ESCAPE '\\' OR sf.capture_name LIKE :cat ESCAPE '\\' OR sf.node_kind LIKE :cat ESCAPE '\\')"
                .to_string()
        }
    };

    let sql = format!(
        "SELECT sf.structural_fact_id, sf.path, sf.language, sf.pattern_id,
                sf.capture_name, sf.node_kind, s.name AS containing_symbol_name,
                sf.start_line, sf.end_line, sf.confidence,
                COALESCE(
                    CASE WHEN json_extract(sf.metadata_json, '$.key_path') LIKE '$.%'
                         THEN substr(json_extract(sf.metadata_json, '$.key_path'), 3)
                         ELSE json_extract(sf.metadata_json, '$.key_path') END,
                    json_extract(sf.metadata_json, '$.key'),
                    json_extract(sf.metadata_json, '$.normalized_route_template')
                ) AS display_key
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
                key: row.get(10)?,
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
    validate_result_limit(limit)?;
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

/// True when a repository-relative path looks like a test file. Directory rules and file-name
/// rules are kept apart: a `test`, `tests`, `autotests`, or `__tests__` directory anywhere
/// including the repository root, or a file name that starts with Qt's `tst_`, starts with
/// `test_` in Python or Ruby, contains `_test.`,
/// `.test.`, or `.spec.`, is exactly `test.rs` or `tests.rs`, or ends with the C# `Tests.cs`
/// (case-sensitive, so `Contests.cs` is a production file).
pub fn is_test_path(path: &str) -> bool {
    let p = path.replace('\\', "/");
    let cut = p.rfind('/').map_or(0, |i| i + 1);
    let directories = format!("/{}/", p[..cut].to_lowercase());
    let file_name = &p[cut..];
    let lower_name = file_name.to_lowercase();
    directories.contains("/test/")
        || directories.contains("/tests/")
        || directories.contains("/autotests/")
        || directories.contains("/__tests__/")
        || lower_name.starts_with("tst_")
        || (lower_name.starts_with("test_")
            && (lower_name.ends_with(".py") || lower_name.ends_with(".rb")))
        || lower_name.contains("_test.")
        || lower_name.contains(".test.")
        || lower_name.contains(".spec.")
        || lower_name == "test.rs"
        || lower_name == "tests.rs"
        || file_name.ends_with("Tests.cs")
}

/// SQL boolean over `alias.path` that mirrors [`is_test_path`] rule for rule.
///
/// `test_path_rule_and_its_sql_mirror_agree_on_every_path` runs both forms over one path list so
/// the two cannot drift apart.
///
/// Every rule needs `test`, `spec`, or `tst_` in the path, so a cheap substring test guards the
/// rules and lets most rows skip the path split. Without the guard the split costs about seven
/// times more over a half-million rows.
pub(crate) fn test_path_predicate(alias: &str) -> String {
    let guard = format!(
        "(lower({alias}.path) LIKE '%test%' OR lower({alias}.path) LIKE '%spec%' OR lower({alias}.path) LIKE '%tst\\_%' ESCAPE '\\')"
    );
    let p = format!("replace({alias}.path, '\\', '/')");
    let directories = format!("'/' || lower(rtrim({p}, replace({p}, '/', ''))) || '/'");
    let file_name = format!("replace({p}, rtrim({p}, replace({p}, '/', '')), '')");
    let lower_name = format!("lower({file_name})");
    let like = |subject: &String, pattern: &str| format!("{subject} LIKE '{pattern}' ESCAPE '\\'");
    let clauses = [
        like(&directories, "%/test/%"),
        like(&directories, "%/tests/%"),
        like(&directories, "%/autotests/%"),
        like(&directories, "%/\\_\\_tests\\_\\_/%"),
        like(&lower_name, "tst\\_%"),
        like(&lower_name, "test\\_%.py"),
        like(&lower_name, "test\\_%.rb"),
        like(&lower_name, "%\\_test.%"),
        like(&lower_name, "%.test.%"),
        like(&lower_name, "%.spec.%"),
        format!("{lower_name} = 'test.rs'"),
        format!("{lower_name} = 'tests.rs'"),
        format!("{file_name} GLOB '*Tests.cs'"),
    ]
    .join(" OR ");
    format!("({guard} AND ({clauses}))")
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
    validate_result_limit(limit)?;
    let max_depth = max_depth.min(5);
    let resolved_seed_symbols = seed_symbols
        .iter()
        .map(|name| {
            get_symbol_by_name(conn, name, symbol_path_filter)?.ok_or_else(|| {
                let (workspace, hint) = symbol_not_found_parts(conn, name, symbol_path_filter);
                QueryError::SymbolNotFound {
                    name: (*name).to_string(),
                    workspace,
                    hint,
                }
            })
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
                format!("AND {pred}", pred = pending_target_predicate("s_target", "s_target_parent")),
            )
        } else {
            ("", String::new())
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
        let not_documentation = not_documentation(conn, "s");
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
              AND {not_documentation}
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
        let doc_file = format!(
            "EXISTS (SELECT 1 FROM symbols d WHERE d.path = files.path AND NOT {})",
            not_documentation(conn, "d")
        );
        let mut test_files_stmt = conn.prepare(&format!(
            "SELECT DISTINCT path FROM files
             WHERE (path LIKE '%test%' OR path LIKE '%spec%') AND path LIKE ?1 ESCAPE '\\'
               AND NOT {doc_file}
             LIMIT 10"
        ))?;
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
    #[test]
    fn result_limit_rejects_values_above_the_shared_ceiling() {
        assert!(validate_result_limit(MAX_RESULT_LIMIT).is_ok());
        assert!(matches!(
            validate_result_limit(usize::MAX),
            Err(QueryError::InvalidResultLimit(usize::MAX))
        ));
    }

    #[test]
    fn find_references_rejects_an_unbounded_limit_before_sql_execution() {
        let conn = Connection::open_in_memory().unwrap();

        assert!(matches!(
            find_references_scoped(&conn, "target", "callers", usize::MAX, false, None),
            Err(QueryError::InvalidResultLimit(usize::MAX))
        ));
    }

    use super::*;
    use crate::db::{ensure_fts_index, open_read_write};

    #[test]
    fn count_parse_diagnostics_counts_rows_for_one_file() {
        let dir = crate::safe_tempdir();
        let conn = open_read_write(&dir.path().join("parse_diagnostics.db")).unwrap();

        assert_eq!(count_parse_diagnostics(&conn, "src/lib.rs"), 0);

        conn.execute_batch(
            "CREATE TABLE parse_diagnostics (
                diagnostic_id TEXT, file_id TEXT, path TEXT, language TEXT, kind TEXT
            );
            INSERT INTO parse_diagnostics VALUES ('d1', 'f1', 'src/lib.rs', 'rust', 'error');
            INSERT INTO parse_diagnostics VALUES ('d2', 'f1', 'src/lib.rs', 'rust', 'error');
            INSERT INTO parse_diagnostics VALUES ('d3', 'f2', 'src/other.rs', 'rust', 'error');",
        )
        .unwrap();

        assert_eq!(count_parse_diagnostics(&conn, "src/lib.rs"), 2);
        assert_eq!(count_parse_diagnostics(&conn, "src\\lib.rs"), 2);
        assert_eq!(count_parse_diagnostics(&conn, "src/clean.rs"), 0);
    }

    #[test]
    fn test_sanitize_fts5_query() {
        let (and_q, or_q) = sanitize_fts5_query("parse tokens");
        assert_eq!(and_q, "(\"parse\"* AND \"tokens\"*) OR \"parsetokens\"*");
        assert_eq!(or_q, "\"parse\"* OR \"tokens\"* OR \"parsetokens\"*");

        let (and_q, or_q) = sanitize_fts5_query("  Option<T>  ");
        assert_eq!(and_q, "(\"Option\"* AND \"T\") OR \"OptionT\"*");
        assert_eq!(or_q, "\"Option\"* OR \"T\" OR \"OptionT\"*");

        let (and_q, or_q) = sanitize_fts5_query("   ");
        assert!(and_q.is_empty());
        assert!(or_q.is_empty());
    }

    #[test]
    fn sanitize_splits_case_boundaries_and_drops_stop_words() {
        let (and_q, or_q) = sanitize_fts5_query("ValidateSyntax");
        assert_eq!(
            and_q,
            "((\"Validate\"* \"Syntax\"*) OR \"ValidateSyntax\"*)"
        );
        assert_eq!(or_q, "\"Validate\"* OR \"Syntax\"* OR \"ValidateSyntax\"*");

        let (and_q, _) = sanitize_fts5_query("find tests related to a symbol");
        assert_eq!(
            and_q,
            "\"find\"* AND \"tests\"* AND \"related\"* AND \"symbol\"*"
        );

        let (and_q, or_q) = sanitize_fts5_query("parseHTTPResponse2");
        assert_eq!(
            and_q,
            "((\"parse\"* \"HTTP\"* \"Response\"* \"2\") OR \"parseHTTPResponse2\"*)"
        );
        assert!(or_q.ends_with("OR \"parseHTTPResponse2\"*"));

        let (and_q, _) = sanitize_fts5_query("validate_syntax");
        assert_eq!(
            and_q,
            "((\"validate\"* \"syntax\"*) OR \"validate_syntax\"*)"
        );

        let (and_q, _) = sanitize_fts5_query("isReady");
        assert_eq!(and_q, "((\"Ready\"*) OR \"isReady\"*)");

        let (and_q, _) = sanitize_fts5_query("before");
        assert_eq!(and_q, "\"before\"*");

        let (and_q, _) = sanitize_fts5_query("fooBar quux");
        assert_eq!(
            and_q,
            "(((\"foo\"* \"Bar\"*) OR \"fooBar\"*) AND \"quux\"*) OR \"fooBarquux\"*"
        );

        let (and_q, _) = sanitize_fts5_query("the for a");
        assert_eq!(and_q, "(\"the\"* AND \"for\"* AND \"a\") OR \"thefora\"*");
    }

    fn search_fixture(rows: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT,
                kind TEXT, signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER, content_type TEXT
            );
            INSERT INTO symbols VALUES {rows};"
        ))
        .unwrap();
        ensure_fts_index(&conn).unwrap();
        conn
    }

    fn code_row(id: &str, path: &str, language: &str, name: &str, doc: &str) -> String {
        format!(
            "('{id}', 'f_{id}', '{path}', '{language}', '{name}', 'function', 'fn {name}()', '{doc}', 'pub', NULL,
              10, 0, 20, 1, 100, 250, 12, 4, 19, 1, 120, 240, 'h_{id}', NULL, 0, 0, 'code')"
        )
    }

    fn doc_row(id: &str, name: &str, doc: &str) -> String {
        format!(
            "('{id}', 'f_{id}', 'docs/{id}.md', 'markdown', '{name}', 'module', '{name}', '{doc}', NULL, NULL,
              3, 0, 3, 1, 10, 40, NULL, NULL, NULL, NULL, NULL, NULL, 'h_{id}', NULL, 0, 0, 'documentation')"
        )
    }

    fn search_names(conn: &Connection, query: &str) -> Vec<String> {
        fts_search_symbols_scoped(conn, query, None, None, false, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.symbol.name)
            .collect()
    }

    const TEST_PATH_CASES: &[(&str, bool)] = &[
        ("tests/foo.py", true),
        ("tests/x.py", true),
        ("tests/tools/test_web.py", true),
        ("src/tests/x.rs", true),
        ("__tests__/a.ts", true),
        ("a/__tests__/b.ts", true),
        ("test/x.java", true),
        ("src/test/Helper.java", true),
        ("src/test_utils.py", true),
        ("lib/test_helper.rb", true),
        ("test_config.py", true),
        ("pkg/test_data/x.json", false),
        ("src/test_detection.rs", false),
        ("autotests/tst_pagerow.qml", true),
        ("autotests/helper.qml", true),
        ("src/autotests/columnview.cpp", true),
        ("tst_foo.qml", true),
        ("src/tst_columnview.qml", true),
        ("autotests\\tst_bar.qml", true),
        ("autotests_helper/x.rs", false),
        ("src/autotest.rs", false),
        ("src/tstamp.rs", false),
        ("src/tst.rs", false),
        ("crates/julie-index/src/analysis/test_quality.rs", false),
        ("x/foo_test.go", true),
        ("x/foo.test.ts", true),
        ("x/foo.spec.js", true),
        ("src/lib_test.rs", true),
        ("src/test.rs", true),
        ("tests.rs", true),
        ("src/tests.rs", true),
        ("Foo.Tests.cs", true),
        ("x/FooTests.cs", true),
        ("src/FooTests.cs", true),
        ("src/Foo.Tests.cs", true),
        ("tests/Foo.cs", true),
        ("x/parser.spec.ts", true),
        ("test_x.py", true),
        ("test.rs", true),
        ("tests\\x.py", true),
        ("src/protocol.spec.v1/parser.rs", false),
        ("pkg/test_support/runtime.py", false),
        ("src/Contests.cs", false),
        ("spec/x.rb", false),
        ("crates/x/src/impact/likely_tests.rs", false),
        ("x/foo_tests.rs", false),
        ("src/latest.rs", false),
        ("x/latest.go", false),
        ("x/manifest.rs", false),
        ("src/attest.rs", false),
        ("contest/x.py", false),
        ("src/testing.rs", false),
        ("src/main.rs", false),
        ("pkg/service.go", false),
    ];

    #[test]
    fn test_path_rule_and_its_sql_mirror_agree_on_every_path() {
        let conn = Connection::open_in_memory().unwrap();
        let sql = format!(
            "SELECT {} FROM (SELECT :path AS path) s",
            test_path_predicate("s")
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        for (path, expected) in TEST_PATH_CASES {
            assert_eq!(is_test_path(path), *expected, "rust rule: {path}");
            let from_sql: bool = stmt
                .query_row(rusqlite::named_params! { ":path": path }, |row| row.get(0))
                .unwrap();
            assert_eq!(from_sql, *expected, "sql mirror: {path}");
        }
    }

    #[test]
    fn unflagged_test_file_rows_are_hidden_unless_tests_are_included() {
        let conn = search_fixture(
            &[
                code_row(
                    "a",
                    "src/parser.rs",
                    "rust",
                    "parse_sidecar",
                    "Parse a sidecar.",
                ),
                code_row(
                    "b",
                    "src/tests/helpers.py",
                    "python",
                    "parse_sidecar_fixture",
                    "Parse a sidecar.",
                ),
            ]
            .join(", "),
        );

        let default_search: Vec<String> =
            fts_search_symbols_scoped(&conn, "parse sidecar", None, None, false, 10)
                .unwrap()
                .into_iter()
                .map(|r| r.symbol.name)
                .collect();
        assert_eq!(default_search, vec!["parse_sidecar".to_string()]);

        let with_tests: Vec<String> =
            fts_search_symbols_scoped(&conn, "parse sidecar", None, None, true, 10)
                .unwrap()
                .into_iter()
                .map(|r| r.symbol.name)
                .collect();
        assert!(with_tests.contains(&"parse_sidecar_fixture".to_string()));

        let default_lookup: Vec<String> =
            search_symbols_scoped(&conn, "parse_sidecar", None, None, false, 10)
                .unwrap()
                .into_iter()
                .map(|s| s.name)
                .collect();
        assert_eq!(default_lookup, vec!["parse_sidecar".to_string()]);

        let lookup_with_tests: Vec<String> =
            search_symbols_scoped(&conn, "parse_sidecar", None, None, true, 10)
                .unwrap()
                .into_iter()
                .map(|s| s.name)
                .collect();
        assert!(lookup_with_tests.contains(&"parse_sidecar_fixture".to_string()));
    }

    #[test]
    fn count_file_symbols_prefers_the_exact_case_path_like_the_loader() {
        let conn = search_fixture(
            &[
                code_row("a", "src/Foo.rs", "rust", "one", ""),
                code_row("b", "src/foo.rs", "rust", "two", ""),
                code_row("c", "src/foo.rs", "rust", "three", ""),
            ]
            .join(", "),
        );
        for path in ["src/Foo.rs", "src/foo.rs", "src/FOO.rs", "src\\foo.rs"] {
            assert_eq!(
                count_file_symbols(&conn, path),
                load_file_symbols(&conn, path).unwrap().len(),
                "{path}"
            );
        }
        assert_eq!(count_file_symbols(&conn, "src/Foo.rs"), 1);
        assert_eq!(count_file_symbols(&conn, "src/FOO.rs"), 3);
    }

    #[test]
    fn lookup_statement_is_not_planned_as_a_multi_index_or() {
        let conn = search_fixture(&code_row("a", "src/lib.rs", "rust", "needle", ""));
        conn.execute_batch(
            "CREATE INDEX idx_symbols_name_kind ON symbols(name, kind);
             CREATE INDEX idx_symbols_test_container ON symbols(test_container);
             CREATE INDEX idx_symbols_is_test ON symbols(is_test);",
        )
        .unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN {}",
            search_symbols_sql(false, false, 20)
        );
        let plan: Vec<String> = conn
            .prepare(&sql)
            .unwrap()
            .query_map(
                rusqlite::named_params! {
                    ":query": "needle",
                    ":pattern": "%needle%",
                    ":kind": None::<&str>,
                    ":path": None::<&str>,
                    ":path_like": None::<&str>,
                },
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            !plan.iter().any(|step| step.contains("MULTI-INDEX OR")),
            "{plan:?}"
        );
    }

    #[test]
    fn qualified_lookup_in_a_test_file_returns_the_named_row() {
        let conn = search_fixture(
            &[
                "('c', 'f_c', 'src/tests/helpers.py', 'python', 'Helpers', 'class', 'class Helpers', '', 'pub', NULL,
                  1, 0, 9, 1, 0, 90, 1, 0, 9, 1, 5, 88, 'h_c', NULL, 0, 0, 'code')".to_string(),
                "('d', 'f_d', 'src/tests/helpers.py', 'python', 'load_fixture', 'method', 'def load_fixture()', '', 'pub', 'c',
                  10, 0, 20, 1, 100, 250, 12, 4, 19, 1, 120, 240, 'h_d', NULL, 0, 0, 'code')".to_string(),
            ]
            .join(", "),
        );

        assert_eq!(
            search_symbols_scoped(&conn, "Helpers.load_fixture", None, None, false, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            search_symbols_scoped(&conn, "Helpers.load_fixture", None, None, true, 10)
                .unwrap()
                .len(),
            1
        );
        assert!(
            search_symbols_scoped(&conn, "load_fix", None, None, false, 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn concept_query_prefers_partial_code_match_over_full_doc_match() {
        let conn = search_fixture(
            &[
                doc_row(
                    "d1",
                    "Safety guarantees",
                    "Pre-flight syntax validation runs before the edit touches disk",
                ),
                doc_row(
                    "d2",
                    "Audit",
                    "The syntax validation before an edit is the invariant",
                ),
                code_row(
                    "c1",
                    "src/syntax.rs",
                    "rust",
                    "validate_syntax",
                    "Validate the syntax of a file",
                ),
                code_row(
                    "c2",
                    "src/edit.rs",
                    "rust",
                    "replace_symbol_body",
                    "Atomic edit with validation",
                ),
            ]
            .join(","),
        );

        let names = search_names(&conn, "syntax validation before edit");

        assert_eq!(names[0], "validate_syntax");
        assert!(names.contains(&"replace_symbol_body".to_string()));
        assert!(names.contains(&"Safety guarantees".to_string()));
    }

    #[test]
    fn camel_case_query_finds_snake_case_symbol_and_vice_versa() {
        let conn = search_fixture(
            &[
                code_row("c1", "src/syntax.rs", "rust", "validate_syntax", ""),
                code_row("c2", "src/syntax.ts", "typescript", "validateSyntax", ""),
            ]
            .join(","),
        );

        let mut camel = search_names(&conn, "ValidateSyntax");
        camel.sort();
        assert_eq!(camel, vec!["validateSyntax", "validate_syntax"]);
        let mut words = search_names(&conn, "validate syntax");
        words.sort();
        assert_eq!(words, vec!["validateSyntax", "validate_syntax"]);
    }

    #[test]
    fn stop_word_prefixed_camel_case_symbol_is_still_found() {
        let conn = search_fixture(
            &[
                code_row("c1", "src/state.ts", "typescript", "isReady", ""),
                code_row("c2", "src/hooks.rs", "rust", "before", ""),
                code_row(
                    "c3",
                    "src/x.rs",
                    "rust",
                    "fooBar",
                    "has fooBar but not the other word",
                ),
            ]
            .join(","),
        );

        assert_eq!(search_names(&conn, "isReady"), vec!["isReady"]);
        assert_eq!(search_names(&conn, "before"), vec!["before"]);
    }

    #[test]
    fn related_tests_use_the_name_as_typed_without_splitting() {
        let conn = search_fixture(
            &[
                code_row("c1", "src/state.ts", "typescript", "isReady", ""),
                "('t1', 'f_t1', 'tests/ready.rs', 'rust', 'test_ready', 'function', 'fn test_ready()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_t1', NULL, 1, 0, 'code')".to_string(),
                "('t2', 'f_t2', 'tests/state.rs', 'rust', 'isReady_reports_true', 'function', 'fn isReady_reports_true()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_t2', NULL, 1, 0, 'code')".to_string(),
            ]
            .join(","),
        );
        let target = get_symbol_by_name(&conn, "isReady", None).unwrap().unwrap();

        let names: Vec<String> = find_related_tests(&conn, &target, 5)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();

        assert_eq!(names, vec!["isReady_reports_true"]);
    }

    #[test]
    fn exact_name_ranks_before_longer_names_with_the_same_tokens() {
        let conn = search_fixture(
            &[
                code_row(
                    "c1",
                    "src/queries.rs",
                    "rust",
                    "fts_search_symbols_scoped",
                    "search symbols scoped with fts",
                ),
                code_row("c2", "src/queries.rs", "rust", "search_symbols_scoped", ""),
            ]
            .join(","),
        );

        assert_eq!(
            search_names(&conn, "search_symbols_scoped")[0],
            "search_symbols_scoped"
        );
    }

    fn sidecar_fixture() -> Connection {
        search_fixture(
            &[
                code_row("c1", "src/sidecar.rs", "rust", "parseSha256Sidecar", ""),
                code_row(
                    "c2",
                    "src/sidecar.rs",
                    "rust",
                    "parse_sidecar_file",
                    "parse the sha256 sidecar file",
                ),
            ]
            .join(","),
        )
    }

    fn candidate<'a>(candidates: &'a [Candidate], name: &str) -> &'a Candidate {
        candidates
            .iter()
            .find(|c| c.result.symbol.name == name)
            .unwrap_or_else(|| panic!("{name} is not a candidate"))
    }

    #[test]
    fn name_substring_admits_a_symbol_the_word_branch_cannot_reach() {
        let conn = sidecar_fixture();

        let candidates = collect_search_candidates(&conn, "sha256", None, None, false, 10).unwrap();

        let target = candidate(&candidates, "parseSha256Sidecar");
        assert!(target.name_match);
        assert!(!target.word_match);
        assert!(!target.exact_name);
        assert!(target.name_terms.contains(&"sha256".to_string()));
    }

    #[test]
    fn a_row_matching_every_word_does_not_hide_a_row_matching_some() {
        let conn = search_fixture(
            &[
                code_row(
                    "c1",
                    "examples/demo.rs",
                    "rust",
                    "demo",
                    "restore offline state",
                ),
                code_row(
                    "c2",
                    "src/replay.rs",
                    "rust",
                    "replay",
                    "restore offline records",
                ),
            ]
            .join(","),
        );

        let candidates =
            collect_search_candidates(&conn, "restore offline state", None, None, false, 10)
                .unwrap();

        assert!(candidate(&candidates, "demo").word_match);
        assert!(candidate(&candidates, "replay").word_match);
        assert_eq!(search_names(&conn, "restore offline state")[0], "replay");
    }

    #[test]
    fn name_branch_admits_the_target_when_word_matches_exceed_the_cap() {
        let mut rows: Vec<String> = (1..=170)
            .map(|i| {
                code_row(
                    &format!("h{i:03}"),
                    "src/sidecar.rs",
                    "rust",
                    &format!("sidecar_helper_{i:03}"),
                    "parse sidecar file",
                )
            })
            .collect();
        rows.push(code_row(
            "c1",
            "src/sidecar.rs",
            "rust",
            "parseSha256Sidecar",
            "",
        ));
        let conn = search_fixture(&rows.join(","));

        let candidates = collect_search_candidates(
            &conn,
            "parse the sha256 sidecar file",
            None,
            None,
            false,
            40,
        )
        .unwrap();

        assert!(candidate(&candidates, "parseSha256Sidecar").name_match);
        assert_eq!(candidates.iter().filter(|c| c.word_match).count(), 160);
    }

    #[test]
    fn the_or_pass_fills_the_word_cap_but_never_exceeds_it() {
        let mut rows: Vec<String> = (1..=20)
            .map(|i| {
                code_row(
                    &format!("a{i:02}"),
                    "src/a.rs",
                    "rust",
                    &format!("both_{i:02}"),
                    "restore offline",
                )
            })
            .collect();
        rows.extend((1..=50).map(|i| {
            code_row(
                &format!("p{i:02}"),
                "src/p.rs",
                "rust",
                &format!("partial_{i:02}"),
                "restore records",
            )
        }));
        let conn = search_fixture(&rows.join(","));

        let candidates =
            collect_search_candidates(&conn, "restore offline", None, None, false, 10).unwrap();

        let word_rows: Vec<&Candidate> = candidates.iter().filter(|c| c.word_match).collect();
        assert_eq!(word_rows.len(), 40);
        assert_eq!(
            word_rows
                .iter()
                .filter(|c| c.result.symbol.name.starts_with("both_"))
                .count(),
            20
        );
    }

    #[test]
    fn exact_name_is_admitted_regardless_of_case() {
        let conn = search_fixture(&code_row("c1", "src/q.rs", "rust", "xyzzy_q", ""));

        let candidates =
            collect_search_candidates(&conn, "XYZZY_Q", None, None, false, 10).unwrap();
        assert!(candidate(&candidates, "xyzzy_q").exact_name);

        conn.execute_batch("DROP TABLE symbol_names_tri").unwrap();
        let candidates =
            collect_search_candidates(&conn, "xyzzy_q", None, None, false, 10).unwrap();
        assert!(candidate(&candidates, "xyzzy_q").exact_name);
    }

    #[test]
    fn exact_name_with_a_quote_is_admitted_through_the_trigram_index() {
        let conn = search_fixture(&code_row(
            "c1",
            "src/say.js",
            "javascript",
            "say \"hi\"",
            "",
        ));

        let candidates =
            collect_search_candidates(&conn, "say \"hi\"", None, None, false, 10).unwrap();

        let target = candidate(&candidates, "say \"hi\"");
        assert!(target.exact_name && target.name_match);
    }

    #[test]
    fn a_row_matched_by_every_branch_is_one_candidate_with_all_flags() {
        let conn = search_fixture(
            &[
                code_row("c1", "src/a.rs", "rust", "sidecar", ""),
                code_row("c2", "src/b.rs", "rust", "sidecar_helper", ""),
            ]
            .join(","),
        );

        let candidates =
            collect_search_candidates(&conn, "sidecar", None, None, false, 10).unwrap();

        assert_eq!(candidates.len(), 2);
        let target = candidate(&candidates, "sidecar");
        assert!(target.exact_name && target.word_match && target.name_match);
        assert!(target.bm25.is_some());
        let helper = candidate(&candidates, "sidecar_helper");
        assert!(!helper.exact_name && helper.word_match && helper.name_match);
    }

    #[test]
    fn an_index_without_the_trigram_table_returns_word_rows_only() {
        let conn = sidecar_fixture();
        conn.execute_batch("DROP TABLE symbol_names_tri").unwrap();

        let candidates = collect_search_candidates(&conn, "sha256", None, None, false, 10).unwrap();

        let names: Vec<&str> = candidates
            .iter()
            .map(|c| c.result.symbol.name.as_str())
            .collect();
        assert_eq!(names, vec!["parse_sidecar_file"]);
        assert!(candidates.iter().all(|c| c.word_match && !c.name_match));
        assert_eq!(search_names(&conn, "sha256"), vec!["parse_sidecar_file"]);
    }

    #[test]
    fn words_under_three_characters_skip_the_name_branch() {
        let conn = search_fixture(
            &[
                code_row("c1", "src/a.rs", "rust", "ab", ""),
                code_row("c2", "src/b.rs", "rust", "cab", ""),
            ]
            .join(","),
        );

        let candidates = collect_search_candidates(&conn, "ab", None, None, false, 10).unwrap();

        assert!(candidates.iter().all(|c| !c.name_match));
        assert!(candidate(&candidates, "ab").exact_name);
    }

    #[test]
    fn trigram_terms_include_the_identifier_parts_of_each_word() {
        assert_eq!(
            trigram_name_terms("collapse_name"),
            vec!["collapse_name", "collapse", "name"]
        );
        assert_eq!(
            trigram_name_terms("parse the sha256 sidecar"),
            vec!["parse", "sha256", "sha", "256", "sidecar"]
        );
        assert_eq!(trigram_name_terms("isReady"), vec!["isready", "ready"]);
        assert_eq!(trigram_name_terms("the before"), vec!["the", "before"]);
        assert!(trigram_name_terms("ab").is_empty());
    }

    #[test]
    fn snake_case_query_admits_a_pascal_case_name_through_the_name_branch() {
        let conn = search_fixture(
            &[
                code_row("c1", "src/collapse.rs", "rust", "CollapseName", ""),
                code_row("c2", "src/other.rs", "rust", "name_collapsed", ""),
            ]
            .join(","),
        );

        let candidates =
            collect_search_candidates(&conn, "collapse_name", None, None, false, 10).unwrap();

        let target = candidate(&candidates, "CollapseName");
        assert!(target.name_match);
        assert_eq!(target.name_terms, vec!["collapse", "name"]);
        assert_eq!(search_names(&conn, "collapse_name")[0], "CollapseName");
    }

    fn plain_candidate(name: &str, kind: &str, path: &str) -> Candidate {
        Candidate {
            result: SymbolSearchResult {
                symbol: Symbol {
                    symbol_id: format!("{path}:{name}"),
                    file_id: "f".into(),
                    path: path.into(),
                    language: "rust".into(),
                    name: name.into(),
                    kind: kind.into(),
                    signature: None,
                    doc_comment: None,
                    visibility: None,
                    parent_symbol_id: None,
                    start_line: 1,
                    start_column: 0,
                    end_line: 1,
                    end_column: 0,
                    start_byte: 0,
                    end_byte: 0,
                    body_start_line: None,
                    body_start_column: None,
                    body_end_line: None,
                    body_end_column: None,
                    body_start_byte: None,
                    body_end_byte: None,
                    body_hash: None,
                    semantic_group: None,
                    is_test: false,
                    test_container: false,
                },
                score: 0.0,
                snippet: None,
                explain: None,
            },
            bm25: None,
            exact_name: false,
            word_match: false,
            name_match: false,
            name_terms: Vec::new(),
            documentation: false,
        }
    }

    fn function(name: &str) -> Candidate {
        plain_candidate(name, "function", "src/lib.rs")
    }

    fn ranked(candidates: Vec<Candidate>, query: &str) -> Vec<(SymbolSearchResult, SearchExplain)> {
        rerank_with(candidates, query, false, None)
    }

    fn ranked_names(candidates: Vec<Candidate>, query: &str) -> Vec<String> {
        ranked(candidates, query)
            .into_iter()
            .map(|(r, _)| r.symbol.name)
            .collect()
    }

    fn documented(name: &str, signature: Option<&str>, doc: &str) -> Candidate {
        let mut candidate = function(name);
        candidate.result.symbol.signature = signature.map(str::to_string);
        candidate.result.symbol.doc_comment = Some(doc.into());
        candidate
    }

    fn strip_ansi_case() -> Vec<Candidate> {
        vec![
            documented("strip_ansi", None, "Remove ANSI escape sequences"),
            documented(
                "_strip_code_fences",
                Some("def _strip_code_fences(text: str)"),
                "The first fenced code block's body, or the stripped text",
            ),
        ]
    }

    #[test]
    fn rerank_words_split_identifiers_and_drop_stop_words_only_beside_content_words() {
        assert_eq!(
            rerank_words("parse the sha256 sidecar file"),
            vec!["parse", "sha", "256", "sidecar", "file"]
        );
        assert_eq!(
            rerank_words("parse_sha256_sidecar"),
            vec!["parse", "sha", "256", "sidecar"]
        );
        assert_eq!(
            rerank_words("ParseHTTPResponse"),
            vec!["parse", "http", "response"]
        );
        assert_eq!(rerank_words("is_ok"), vec!["ok"]);
        assert_eq!(rerank_words("the before"), vec!["the", "before"]);
    }

    #[test]
    fn stop_words_cover_english_function_words_but_not_identifier_directions() {
        for word in ["was", "whether", "another"] {
            assert!(is_stop_word(word), "{word} must be a stop word");
        }
        for word in ["down", "into", "run"] {
            assert!(!is_stop_word(word), "{word} must stay a content word");
        }
        assert_eq!(rerank_words("what was the file"), vec!["file"]);
    }

    #[test]
    fn a_public_name_sorts_before_its_private_twin_at_an_equal_score() {
        let mut private = function("_create_skill");
        private.bm25 = Some(-9.0);
        let mut public = function("create_skill");
        public.bm25 = Some(-1.0);

        let rows = ranked(vec![private, public], "create skill");

        assert_eq!(rows[0].0.score, rows[1].0.score);
        assert_eq!(rows[0].1.name_strength, rows[1].1.name_strength);
        assert_eq!(
            rows.iter()
                .map(|(r, _)| r.symbol.name.as_str())
                .collect::<Vec<_>>(),
            vec!["create_skill", "_create_skill"]
        );
    }

    #[test]
    fn a_whole_name_constant_yields_to_a_function_that_holds_the_word_with_context() {
        let rows = ranked(
            vec![
                plain_candidate("Glob", "constant", "src/glob.rs"),
                function("matches_glob_pattern"),
            ],
            "glob",
        );

        assert_eq!(rows[0].0.symbol.name, "matches_glob_pattern");
        assert_eq!(rows[1].1.name_tier, "whole");
        assert_eq!(rows[1].1.name_bonus, W_NAME_ALL_WORDS);
    }

    #[test]
    fn name_tiers_are_whole_then_all_words_then_partial_then_none() {
        let rows = ranked(
            vec![
                function("validate_everything"),
                function("validate_syntax_now"),
                function("validate_syntax"),
                function("unrelated"),
            ],
            "validate syntax",
        );
        let tiers: Vec<(&str, &str, f64)> = rows
            .iter()
            .map(|(r, e)| (r.symbol.name.as_str(), e.name_tier.as_str(), e.name_bonus))
            .collect();

        assert_eq!(
            tiers,
            vec![
                ("validate_syntax", "whole", W_NAME_WHOLE),
                ("validate_syntax_now", "all", W_NAME_ALL_WORDS),
                ("validate_everything", "partial", 0.0),
                ("unrelated", "none", 0.0),
            ]
        );
        assert_eq!(rows[0].0.score, W_NAME_WHOLE + W_TERMS + W_KIND_DEFINITION);
        assert_eq!(
            rows[1].0.score,
            W_NAME_ALL_WORDS + W_TERMS + W_KIND_DEFINITION
        );
        assert_eq!(rows[2].0.score, W_TERMS / 2.0 + W_KIND_DEFINITION);
        assert_eq!(rows[3].0.score, W_KIND_DEFINITION);
    }

    #[test]
    fn distinct_scoring_prefers_three_terms_covered_once_over_two_terms_repeated() {
        assert_eq!(
            ranked_names(strip_ansi_case(), "strip ansi escape codes"),
            vec!["strip_ansi", "_strip_code_fences"]
        );
    }

    #[test]
    fn distinct_scoring_denies_the_all_words_bonus_to_a_substring_only_name() {
        let candidates = vec![
            function("execute_julie_extract"),
            documented("slice_bytes", None, "cut a byte range"),
        ];

        let rows = ranked(candidates, "cut");
        let bonuses: Vec<(&str, &str, f64)> = rows
            .iter()
            .map(|(r, e)| (r.symbol.name.as_str(), e.name_tier.as_str(), e.name_bonus))
            .collect();

        assert_eq!(
            bonuses,
            vec![
                ("execute_julie_extract", "partial", 0.0),
                ("slice_bytes", "none", 0.0),
            ]
        );
        assert_eq!(rows[0].0.score, rows[1].0.score);
    }

    #[test]
    fn distinct_scoring_keeps_the_whole_name_and_all_words_tiers_in_order() {
        let candidates = vec![
            function("validate_everything"),
            function("validate_syntax_now"),
            function("validate_syntax"),
            function("unrelated"),
        ];

        assert_eq!(
            ranked_names(candidates, "validate syntax"),
            vec![
                "validate_syntax",
                "validate_syntax_now",
                "validate_everything",
                "unrelated"
            ]
        );
    }

    #[test]
    fn idf_weights_rank_a_term_in_one_row_above_a_term_in_most_rows() {
        let mut rows: Vec<String> = (0..10)
            .map(|i| {
                code_row(
                    &format!("s{i}"),
                    &format!("src/f{i}.rs"),
                    "rust",
                    &format!("search_{i}"),
                    "searches the index",
                )
            })
            .collect();
        rows.push(code_row(
            "rare",
            "src/rare.rs",
            "rust",
            "sanitize_input",
            "sanitize the input",
        ));
        let conn = search_fixture(&rows.join(", "));
        let terms = vec!["sanitize".to_string(), "search".to_string()];

        let weights = idf_weights(&conn, &terms);
        assert!(weights[0] > weights[1]);
    }

    #[test]
    fn idf_weights_count_a_term_the_way_the_index_tokenizer_stems_it() {
        let conn = search_fixture(&code_row(
            "n",
            "src/news.rs",
            "rust",
            "fetch_news",
            "fetch the news feed",
        ));
        let terms = vec!["news".to_string(), "unseen".to_string()];

        let weights = idf_weights(&conn, &terms);

        assert!(weights[0] < weights[1]);
    }

    #[test]
    fn a_signature_hit_past_the_head_byte_cap_does_not_credit_its_term() {
        let crediting_field = |padding: usize| {
            let mut row = function("handler");
            row.result.symbol.signature = Some(format!(
                "fn handler({}sidecar: u8)",
                "a: u8, ".repeat(padding)
            ));
            ranked(vec![row], "sidecar")[0].1.terms[0].1.clone()
        };

        assert_eq!(crediting_field(4), "signature");
        assert_eq!(crediting_field(80), "none");
    }

    #[test]
    fn explain_terms_name_the_crediting_field_of_every_query_term() {
        let rows = ranked(strip_ansi_case(), "strip ansi escape codes");
        let terms: Vec<(&str, &str, f64)> = rows[0]
            .1
            .terms
            .iter()
            .map(|(term, field, credit)| (term.as_str(), field.as_str(), *credit))
            .collect();

        assert_eq!(rows[0].0.symbol.name, "strip_ansi");
        assert_eq!(
            terms,
            vec![
                ("strip", "name", 3.0),
                ("ansi", "name", 3.0),
                ("escape", "doc", TEXT_CREDIT),
                ("codes", "none", 0.0),
            ]
        );
    }

    #[test]
    fn name_coverage_accepts_token_runs_substrings_and_stems() {
        let strengths = |name: &str, query: &str| {
            let stemmer = Stemmer::create(Algorithm::English);
            let words: Vec<QueryWord> = rerank_words(query)
                .into_iter()
                .map(|word| QueryWord {
                    stem: stemmer.stem(&word).into_owned(),
                    word,
                })
                .collect();
            name_hits(name, &words, &stemmer)
        };

        assert_eq!(strengths("parseSha256Sidecar", "sha 256"), vec![3, 3]);
        assert_eq!(strengths("parseSha256Sidecar", "sha256"), vec![3, 3]);
        assert_eq!(strengths("parseSha256Sidecar", "esha"), vec![1]);
        assert_eq!(strengths("validate_syntax", "validation"), vec![2]);
        assert_eq!(strengths("is_ok", "ok"), vec![3]);
        assert_eq!(strengths("isReady", "is"), vec![3]);
        assert_eq!(strengths("größe_berechnen", "größe"), vec![3]);
        assert_eq!(
            strengths("parseSha256Sidecar", "sidecar checksum"),
            vec![3, 0]
        );
        assert_eq!(
            strengths("parseSha256Sidecar", "checksum digest"),
            vec![0, 0]
        );
    }

    #[test]
    fn a_rarer_term_moves_the_score_more_than_a_common_one() {
        let weights = [4.0, 1.0];
        let rows = rerank_with(
            vec![function("rare_helper"), function("common_helper")],
            "rare common",
            false,
            Some(&weights),
        );

        assert_eq!(
            rows[0].1.word_weights,
            vec![("rare".to_string(), 4.0), ("common".to_string(), 1.0)]
        );
        assert_eq!(rows[0].0.symbol.name, "rare_helper");
        assert_eq!(rows[0].0.score, W_TERMS * 4.0 / 5.0 + W_KIND_DEFINITION);
        assert_eq!(rows[1].0.score, W_TERMS * 1.0 / 5.0 + W_KIND_DEFINITION);
    }

    #[test]
    fn any_name_hit_outranks_a_zero_coverage_definition_for_long_queries() {
        let rows = ranked(
            vec![
                function("render_mode"),
                plain_candidate("retry_count", "constant", "src/scan.rs"),
            ],
            "how many times a failed download is tried again retry limit",
        );

        assert_eq!(rows[0].0.symbol.name, "retry_count");
        assert_eq!(rows[0].1.name_tier, "partial");
        assert!(rows[0].0.score > rows[1].0.score);
        assert_eq!(rows[1].0.score, W_KIND_DEFINITION);
    }

    #[test]
    fn a_doc_hit_past_the_head_byte_cap_does_not_credit_its_term() {
        let mut row = function("load");
        row.result.symbol.signature = Some("fn load(config: &Config) -> Loaded".into());
        row.result.symbol.doc_comment = Some(format!("{}settings", "é".repeat(200)));
        let (result, explain) = ranked(vec![row], "config settings").remove(0);

        assert_eq!(
            explain.terms,
            vec![
                ("config".into(), "signature".into(), TEXT_CREDIT),
                ("settings".into(), "none".into(), 0.0),
            ]
        );
        assert_eq!(result.score, explain.term_score + W_KIND_DEFINITION);
    }

    #[test]
    fn text_coverage_matches_whole_tokens_by_word_or_stem_prefix() {
        let crediting_field =
            |row: Candidate, query: &str| ranked(vec![row], query).remove(0).1.terms[0].1.clone();
        let doc_field = |doc: &str, query: &str| {
            let mut row = function("row");
            row.result.symbol.doc_comment = Some(doc.into());
            crediting_field(row, query)
        };
        let sig_field = |signature: &str, query: &str| {
            let mut row = function("row");
            row.result.symbol.signature = Some(signature.into());
            crediting_field(row, query)
        };

        assert_eq!(doc_field("The system runs.", "stemming"), "none");
        assert_eq!(doc_field("The stemmer runs.", "stemming"), "doc");
        assert_eq!(doc_field("Compares stems.", "stemming"), "doc");
        assert_eq!(doc_field("An important port.", "porter"), "none");
        assert_eq!(
            sig_field("fn sha256sum(data: &[u8]) -> String", "sha256"),
            "signature"
        );
        assert_eq!(sig_field("fn is_ok()", "ok"), "signature");
        assert_eq!(sig_field("fn okay()", "ok"), "none");
        assert_eq!(
            sig_field("fn parseSha256Sidecar(text)", "sidecar"),
            "signature"
        );
    }

    #[test]
    fn text_tokens_split_like_query_words_then_identifiers() {
        fn two_pass(text: &str) -> Vec<&str> {
            query_words(text)
                .into_iter()
                .flat_map(split_identifier)
                .collect()
        }
        fn one_pass(text: &str) -> Vec<&str> {
            let mut out = Vec::new();
            text_tokens_into(text, &mut out);
            out
        }
        let ascii = "fn parseHTTPResponse2(raw: &str, _id: u8) -> Vec<&str> // sha256_sum";
        let unicode = "Berechnet die Größe: größe_berechnen(pfad) -> ÜberGroß2x";

        assert_eq!(one_pass(ascii), two_pass(ascii));
        assert_eq!(
            one_pass(ascii),
            vec![
                "fn", "parse", "HTTP", "Response", "2", "raw", "str", "id", "u", "8", "Vec", "str",
                "sha", "256", "sum",
            ]
        );
        assert_eq!(one_pass(unicode), two_pass(unicode));
        assert!(one_pass("").is_empty());
        assert!(one_pass("_ __ ...").is_empty());
    }

    #[test]
    fn a_doc_credits_a_term_by_its_stem() {
        let mut row = function("check");
        row.result.symbol.doc_comment = Some("Validates the input.".into());
        let explain = ranked(vec![row], "validation").remove(0).1;

        assert_eq!(
            explain.terms,
            vec![("validation".into(), "doc".into(), TEXT_CREDIT)]
        );
    }

    #[test]
    fn kind_prior_orders_definitions_over_members_over_imports() {
        let rows = ranked(
            vec![
                plain_candidate("Scan", "import", "src/a.rs"),
                plain_candidate("Scan", "enum_member", "src/b.rs"),
                plain_candidate("Scan", "function", "src/c.rs"),
            ],
            "scan",
        );
        let order: Vec<(&str, f64)> = rows
            .iter()
            .map(|(r, e)| (r.symbol.path.as_str(), e.kind_prior))
            .collect();

        assert_eq!(
            order,
            vec![
                ("src/c.rs", W_KIND_DEFINITION),
                ("src/b.rs", W_KIND_MEMBER),
                ("src/a.rs", W_KIND_IMPORT),
            ]
        );
    }

    #[test]
    fn a_partial_name_match_on_a_member_beats_the_kind_prior_of_a_function() {
        let names = ranked_names(
            vec![
                function("RenderMode"),
                plain_candidate("MaxRetryCount", "constant", "pkg/scan.go"),
            ],
            "retry download limit timeout",
        );

        assert_eq!(names[0], "MaxRetryCount");
    }

    #[test]
    fn path_role_demotes_role_directories_unless_the_query_names_them() {
        let rows = |query: &str| {
            ranked(
                vec![
                    plain_candidate("verifyChecksum", "function", "scripts/launcher.ts"),
                    plain_candidate("verify_checksum", "function", "src/archive.rs"),
                ],
                query,
            )
        };

        let plain = rows("verify checksum");
        assert_eq!(plain[0].0.symbol.path, "src/archive.rs");
        assert_eq!(plain[1].1.path_role, W_PATH_ROLE);

        let named = rows("launcher script verify checksum");
        assert!(named.iter().all(|(_, e)| e.path_role == 0.0));

        let only_launcher = rows("launcher verify checksum");
        assert_eq!(only_launcher[0].0.symbol.path, "src/archive.rs");
        assert_eq!(only_launcher[1].1.path_role, W_PATH_ROLE);

        let windows = ranked(
            vec![plain_candidate(
                "verifyChecksum",
                "function",
                "scripts\\launcher.ts",
            )],
            "verify checksum",
        );
        assert_eq!(windows[0].1.path_role, W_PATH_ROLE);
    }

    #[test]
    fn documentation_rows_sort_after_every_code_row() {
        let mut heading = plain_candidate("Verify checksum", "heading", "README.md");
        heading.documentation = true;
        heading.result.symbol.language = "markdown".into();
        heading.result.symbol.signature = Some("Verify checksum".into());
        heading.result.symbol.doc_comment = Some("Verify the checksum of the archive.".into());
        let rows = ranked(
            vec![
                heading,
                plain_candidate("unrelated", "variable", "src/a.rs"),
            ],
            "verify checksum",
        );

        assert_eq!(rows[0].0.symbol.name, "unrelated");
        assert_eq!(rows[1].1.documentation, W_DOCUMENTATION_ROW);
        assert_eq!(rows[1].1.name_tier, "whole");
        assert!(rows[1].0.score < 0.0);
    }

    #[test]
    fn test_intent_boosts_test_rows_only_when_tests_are_included_and_named() {
        let rows = |query: &str, include_tests: bool| {
            let mut test_row = plain_candidate("payment_flow", "function", "tests/payment.rs");
            test_row.result.symbol.is_test = true;
            let plain_row = plain_candidate("payment_flow", "function", "src/payment.rs");
            rerank_with(vec![plain_row, test_row], query, include_tests, None)
        };

        let boosted = rows("payment flow tests", true);
        assert_eq!(boosted[0].0.symbol.path, "tests/payment.rs");
        assert_eq!(boosted[0].1.test_intent, W_TEST_INTENT);
        assert_eq!(boosted[1].1.test_intent, 0.0);

        assert!(
            rows("payment flow tests", false)
                .iter()
                .all(|(_, e)| e.test_intent == 0.0)
        );
        assert!(
            rows("payment flow", true)
                .iter()
                .all(|(_, e)| e.test_intent == 0.0)
        );
    }

    #[test]
    fn ties_break_by_bm25_then_name_length_then_path() {
        let mut word_row = plain_candidate("payment", "function", "src/z.rs");
        word_row.word_match = true;
        word_row.bm25 = Some(-4.0);
        let mut weaker_word_row = plain_candidate("payment", "function", "src/a.rs");
        weaker_word_row.word_match = true;
        weaker_word_row.bm25 = Some(-2.0);
        let mut name_only = plain_candidate("payment", "function", "src/b.rs");
        name_only.name_match = true;
        let rows = ranked(
            vec![
                plain_candidate("payment", "function", "src/y.rs"),
                name_only,
                weaker_word_row,
                word_row,
            ],
            "payment",
        );
        let paths: Vec<&str> = rows.iter().map(|(r, _)| r.symbol.path.as_str()).collect();

        assert_eq!(paths, vec!["src/z.rs", "src/a.rs", "src/b.rs", "src/y.rs"]);

        let by_length = ranked_names(
            vec![
                function("payment_gateway_client"),
                function("payment_gateway"),
            ],
            "gateway",
        );
        assert_eq!(by_length, vec!["payment_gateway", "payment_gateway_client"]);
    }

    #[test]
    fn a_whole_token_name_outranks_a_substring_name_with_better_bm25() {
        let mut token = function("csr");
        token.bm25 = Some(-1.0);
        let mut substring = function("action_csrf_token");
        substring.bm25 = Some(-5.0);

        let rows = ranked(vec![substring, token], "csr adjacency");

        assert!(rows[0].0.score > rows[1].0.score);
        assert_eq!(rows[0].0.symbol.name, "csr");
    }

    #[test]
    fn an_acronym_token_outranks_a_name_that_only_contains_it() {
        let mut token = function("http_client");
        token.bm25 = Some(-1.0);
        let mut substring = function("shttpd_config");
        substring.bm25 = Some(-5.0);

        let rows = ranked(vec![substring, token], "http");

        assert!(rows[0].0.score > rows[1].0.score);
        assert_eq!(rows[0].0.symbol.name, "http_client");
    }

    #[test]
    fn explain_reports_the_sum_of_the_name_strengths() {
        let rows = ranked(vec![function("action_csrf_token")], "csr token");

        assert_eq!(rows[0].1.name_strength, 4);
    }

    #[test]
    fn snippets_follow_the_admitting_branch() {
        let mut word_row = function("parse_sidecar_file");
        word_row.word_match = true;
        word_row.result.snippet = Some("parse the [sha256] sidecar file".into());
        let mut name_row = function("parseSha256Sidecar");
        name_row.name_match = true;
        name_row.name_terms = vec!["sha".into(), "sha256".into(), "256".into()];
        let mut exact_row = function("sha256");
        exact_row.exact_name = true;
        let rows = ranked(vec![word_row, name_row, exact_row], "sha256");
        let snippets: Vec<(&str, &str)> = rows
            .iter()
            .map(|(r, _)| (r.symbol.name.as_str(), r.snippet.as_deref().unwrap()))
            .collect();

        assert_eq!(
            snippets,
            vec![
                ("sha256", "sha256"),
                ("parseSha256Sidecar", "parse[Sha256]Sidecar"),
                ("parse_sidecar_file", "parse the [sha256] sidecar file"),
            ]
        );
        assert_eq!(rows[1].1.branches, vec!["name"]);
        assert_eq!(rows[0].1.branches, vec!["exact"]);
    }

    #[test]
    fn explain_is_attached_only_when_requested() {
        let conn = sidecar_fixture();
        let query = "sha256";

        let silent = fts_search_symbols_scoped(&conn, query, None, None, false, 10).unwrap();
        assert!(silent.iter().all(|r| r.explain.is_none()));
        assert!(silent[0].score > 0.0);
        assert_eq!(
            serde_json::to_value(&silent[0]).unwrap().get("explain"),
            None
        );

        let explained =
            fts_search_symbols_explained(&conn, query, None, None, false, 10, true).unwrap();
        let by_name = |name: &str| {
            explained
                .iter()
                .find(|r| r.symbol.name == name)
                .and_then(|r| r.explain.as_ref())
                .unwrap()
        };
        let name_only = by_name("parseSha256Sidecar");
        assert_eq!(name_only.candidates, 2);
        assert_eq!(name_only.branches, vec!["name"]);
        assert_eq!(name_only.bm25, None);
        let word_row = by_name("parse_sidecar_file");
        assert!(word_row.bm25.unwrap() < 0.0);
        assert_eq!(word_row.candidates, 2);
        assert!(
            serde_json::to_value(&explained[0])
                .unwrap()
                .get("explain")
                .is_some()
        );
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
    fn find_related_tests_returns_each_test_once_under_the_limit() {
        let dir = crate::safe_tempdir();
        let conn = open_read_write(&dir.path().join("related_tests_limit.db")).unwrap();
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
            CREATE TABLE type_facts (
                type_fact_id TEXT, symbol_id TEXT, language TEXT, resolved_type TEXT, generic_params_json TEXT
            );
            INSERT INTO symbols VALUES
                ('s_target', 'f1', 'src/lib.rs', 'rust', 'compute', 'function', 'pub fn compute()', NULL, 'pub', NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
                ('t_a', 'f2', 'tests/a.rs', 'rust', 'first_case', 'function', 'fn first_case()', NULL, NULL, NULL, 1, 0, 20, 1, 0, 300, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0),
                ('t_b', 'f3', 'tests/b.rs', 'rust', 'second_case', 'function', 'fn second_case()', NULL, NULL, NULL, 1, 0, 10, 1, 0, 100, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1, 0);
            INSERT INTO pending_relationships (from_symbol_id, target_terminal_name, kind, path, start_line, start_column, target_receiver, target_namespace_json, target_display_name) VALUES
                ('t_a', 'compute', 'calls', 'tests/a.rs', 3, 4, NULL, NULL, 'compute'),
                ('t_a', 'compute', 'calls', 'tests/a.rs', 5, 4, NULL, NULL, 'compute'),
                ('t_a', 'compute', 'calls', 'tests/a.rs', 7, 4, NULL, NULL, 'compute'),
                ('t_a', 'compute', 'calls', 'tests/a.rs', 9, 4, NULL, NULL, 'compute'),
                ('t_a', 'compute', 'calls', 'tests/a.rs', 11, 4, NULL, NULL, 'compute'),
                ('t_b', 'compute', 'calls', 'tests/b.rs', 3, 4, NULL, NULL, 'compute');",
        )
        .unwrap();
        let target = get_symbol_by_name(&conn, "compute", None).unwrap().unwrap();

        let tests = find_related_tests(&conn, &target, 5).unwrap();

        let mut names: Vec<&str> = tests.iter().map(|t| t.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["first_case", "second_case"]);
    }

    #[test]
    fn documentation_rows_rank_after_code_in_search() {
        let dir = crate::safe_tempdir();
        let conn = open_read_write(&dir.path().join("doc_rank.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT,
                kind TEXT, signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
                body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
                body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
                semantic_group TEXT, is_test INTEGER, test_container INTEGER, content_type TEXT
            );
            INSERT INTO symbols VALUES
                ('s_doc', 'f1', 'docs/plans/018.adoc', 'asciidoc', 'Reconcile offline edits',
                 'heading', 'Reconcile offline edits', NULL, NULL, NULL,
                 3, 0, 3, 1, 10, 40, 3, 0, 3, 1, 10, 40, 'hash_doc', NULL, 0, 0, 'documentation'),
                ('s_code', 'f2', 'src/sync.rs', 'rust', 'reconcile_offline_edits', 'function',
                 'fn reconcile_offline_edits()', 'Reconcile offline edits at startup', 'pub', NULL,
                 10, 0, 20, 1, 100, 250, 12, 4, 19, 1, 120, 240, 'hash_code', NULL, 0, 0, 'code');",
        )
        .unwrap();
        ensure_fts_index(&conn).unwrap();

        let results =
            fts_search_symbols_scoped(&conn, "reconcile offline edits", None, None, false, 10)
                .unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].symbol.name, "reconcile_offline_edits");
        assert_eq!(results[1].symbol.name, "Reconcile offline edits");
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
            CREATE TABLE type_facts (
                type_fact_id TEXT, symbol_id TEXT, language TEXT, resolved_type TEXT, generic_params_json TEXT
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
                start_line INTEGER, end_line INTEGER, confidence REAL, metadata_json TEXT
            );
            CREATE TABLE literals (
                literal_id TEXT PRIMARY KEY, file_id TEXT, path TEXT NOT NULL, language TEXT,
                kind TEXT, literal_text TEXT, carrier TEXT, containing_symbol_id TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
                start_byte INTEGER, end_byte INTEGER
            );
            INSERT INTO structural_facts VALUES
                ('sf_toml', 'f1', 'Cargo.toml', 'toml', 'toml.key_value.v1', 'key_value', 'table', NULL, 1, 2, 1.0, '{\"key\":\"command\",\"key_path\":\"mcp_servers.code-kb.command\"}'),
                ('sf_yaml', 'f6', '.github/workflows/ci.yml', 'yaml', 'yaml.key_value.v1', 'key_value', 'block_mapping_pair', NULL, 3, 3, 1.0, '{\"key\":\"name\",\"key_path\":\"$.on.name\"}'),
                ('sf_route', 'f2', 'src/routes/api.rs', 'rust', 'axum.route.v1', 'get_users', 'function', NULL, 10, 20, 1.0, '{\"verb\":\"GET\",\"normalized_route_template\":\"/api/v1/users/:id\"}'),
                ('sf_sql', 'f3', 'src/db/queries.rs', 'rust', 'sql.select_query.v1', 'select_users', 'function', NULL, 30, 40, 1.0, NULL),
                ('sf_model', 'f4', 'src/models/user.rs', 'rust', 'sql.table_definition.v1', 'User', 'struct', NULL, 50, 60, 1.0, NULL),
                ('sf_css', 'f7', 'web/site.css', 'css', 'css.media_query.v1', 'media', 'media_statement', NULL, 1, 1, 1.0, NULL),
                ('sf_custom', 'f5', 'src/custom.rs', 'rust', 'my_custom_pattern', 'custom_name', 'item', NULL, 70, 80, 1.0, NULL);
            INSERT INTO literals VALUES
                ('lit_toml', 'f1', 'Cargo.toml', 'toml', 'toml_key', '\"version\"', 'key', NULL, 3, 0, 3, 9, 20, 29),
                ('lit_route', 'f2', 'src/routes/api.rs', 'rust', 'http_route', '\"/api/v1/users\"', 'string', NULL, 12, 0, 12, 15, 100, 115),
                ('lit_sql', 'f3', 'src/db/queries.rs', 'rust', 'sql_query', '\"SELECT * FROM users\"', 'string', NULL, 32, 0, 32, 21, 200, 221),
                ('lit_model', 'f4', 'src/models/user.rs', 'rust', 'model_table', '\"users_table\"', 'string', NULL, 52, 0, 52, 13, 300, 313);",
        )
        .unwrap();

        // 1. "config" alias
        let facts_config = find_structural_facts_scoped(&conn, "config", None, 10).unwrap();
        assert_eq!(facts_config.len(), 2);
        assert_eq!(facts_config[0].pattern_id, "yaml.key_value.v1");
        assert_eq!(facts_config[0].key.as_deref(), Some("on.name"));
        assert_eq!(facts_config[1].pattern_id, "toml.key_value.v1");
        assert_eq!(
            facts_config[1].key.as_deref(),
            Some("mcp_servers.code-kb.command")
        );
        let lits_config = find_literals_scoped(&conn, "config", None, 10).unwrap();
        assert_eq!(lits_config.len(), 1);
        assert_eq!(lits_config[0].kind, "toml_key");

        // 2. "route" and "routes" aliases
        let facts_route = find_structural_facts_scoped(&conn, "route", None, 10).unwrap();
        assert_eq!(facts_route.len(), 1);
        assert_eq!(facts_route[0].pattern_id, "axum.route.v1");
        assert_eq!(facts_route[0].key.as_deref(), Some("/api/v1/users/:id"));
        let facts_routes = find_structural_facts_scoped(&conn, "routes", None, 10).unwrap();
        assert_eq!(facts_routes.len(), 1);
        let lits_route = find_literals_scoped(&conn, "route", None, 10).unwrap();
        assert_eq!(lits_route.len(), 1);
        assert_eq!(lits_route[0].kind, "http_route");

        // 3. "query", "queries", "sql" aliases
        for q in &["query", "queries", "sql"] {
            let facts = find_structural_facts_scoped(&conn, q, None, 10).unwrap();
            assert_eq!(facts.len(), 2, "Failed for {}", q);
            assert!(facts.iter().all(|f| f.pattern_id.starts_with("sql.")));
            let lits = find_literals_scoped(&conn, q, None, 10).unwrap();
            assert_eq!(lits.len(), 1, "Failed for {}", q);
            assert_eq!(lits[0].kind, "sql_query");
        }

        // 4. "model" and "models" aliases
        for m in &["model", "models"] {
            let facts = find_structural_facts_scoped(&conn, m, None, 10).unwrap();
            assert_eq!(facts.len(), 1, "Failed for {}", m);
            assert_eq!(facts[0].pattern_id, "sql.table_definition.v1");
            let lits = find_literals_scoped(&conn, m, None, 10).unwrap();
            assert_eq!(lits.len(), 1, "Failed for {}", m);
            assert_eq!(lits[0].kind, "model_table");
        }

        // 5. Custom / unknown category
        let facts_custom = find_structural_facts_scoped(&conn, "custom_pattern", None, 10).unwrap();
        assert_eq!(facts_custom.len(), 1);
        assert_eq!(facts_custom[0].pattern_id, "my_custom_pattern");
        assert_eq!(facts_custom[0].key, None);

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
        assert_eq!(f_del.len(), 2);
        let l_del = find_literals(&conn, "config", 10).unwrap();
        assert_eq!(l_del.len(), 1);
    }

    fn local_variable_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT,
                kind TEXT, signature TEXT, doc_comment TEXT, visibility TEXT,
                parent_symbol_id TEXT, start_line INTEGER, start_column INTEGER,
                end_line INTEGER, end_column INTEGER, start_byte INTEGER, end_byte INTEGER,
                body_start_line INTEGER, body_start_column INTEGER, body_end_line INTEGER,
                body_end_column INTEGER, body_start_byte INTEGER, body_end_byte INTEGER,
                body_hash TEXT, semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            INSERT INTO symbols (symbol_id, file_id, path, language, name, kind, signature,
                                 parent_symbol_id, start_line, start_column, end_line, end_column,
                                 start_byte, end_byte, is_test, test_container)
            VALUES
                ('func', 'f1', 'src/db.rs', 'rust', 'open_conn', 'function',
                 'fn open_conn() -> sqlite Connection', NULL, 1, 0, 9, 1, 0, 100, 0, 0),
                ('local', 'f1', 'src/db.rs', 'rust', 'conn', 'variable',
                 'let conn: sqlite Connection', 'func', 2, 4, 2, 30, 10, 40, 0, 0),
                ('pool', 'f1', 'src/db.rs', 'rust', 'Pool', 'struct',
                 'struct Pool sqlite', NULL, 12, 0, 16, 1, 120, 200, 0, 0),
                ('field', 'f1', 'src/db.rs', 'rust', 'conn', 'variable',
                 'conn: sqlite Connection', 'pool', 13, 4, 13, 28, 130, 160, 0, 0),
                ('global', 'f1', 'src/db.rs', 'rust', 'conn', 'variable',
                 'static conn: sqlite Connection', NULL, 20, 0, 20, 30, 210, 240, 0, 0),
                ('closure', 'f1', 'src/db.rs', 'rust', 'with_conn', 'variable',
                 'let with_conn = |c: sqlite Connection|', 'func', 4, 4, 6, 5, 50, 90, 0, 0),
                ('nested', 'f1', 'src/db.rs', 'rust', 'conn', 'variable',
                 'let conn = c sqlite', 'closure', 5, 8, 5, 24, 60, 80, 0, 0);",
        )
        .unwrap();
        conn
    }

    fn matched_symbol_ids(conn: &Connection, query: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT s.symbol_id FROM symbols_fts f
                 JOIN symbols s ON s.rowid = f.rowid
                 WHERE f.symbols_fts MATCH ?1 ORDER BY s.symbol_id",
            )
            .unwrap();
        let mut ids = stmt
            .query_map(params![query], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        ids.sort();
        ids
    }

    #[test]
    fn fts_index_excludes_locals_and_rebuilds_a_stale_index() {
        let conn = local_variable_fixture();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE symbols_fts USING fts5(
                name, signature, doc_comment,
                content='symbols', content_rowid='rowid', tokenize='porter unicode61'
            );
            INSERT INTO symbols_fts(rowid, name, signature, doc_comment)
            SELECT rowid, name, signature, doc_comment FROM symbols;",
        )
        .unwrap();

        ensure_fts_index(&conn).unwrap();

        assert_eq!(
            matched_symbol_ids(&conn, "sqlite"),
            vec!["field", "func", "global", "pool"]
        );
    }

    #[test]
    fn lookup_excludes_locals_and_parameters() {
        let conn = local_variable_fixture();

        let ids: Vec<String> = search_symbols_scoped(&conn, "conn", None, None, false, 10)
            .unwrap()
            .into_iter()
            .map(|s| s.symbol_id)
            .collect();

        assert!(!ids.contains(&"local".to_string()));
        assert!(!ids.contains(&"nested".to_string()));
        assert!(ids.contains(&"field".to_string()));
        assert!(ids.contains(&"global".to_string()));
    }

    #[test]
    fn search_excludes_locals_and_parameters() {
        let conn = local_variable_fixture();
        ensure_fts_index(&conn).unwrap();

        let ids: Vec<String> = fts_search_symbols_scoped(&conn, "sqlite", None, None, false, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.symbol.symbol_id)
            .collect();

        assert!(!ids.contains(&"local".to_string()));
        assert!(ids.contains(&"func".to_string()));
    }

    #[test]
    fn variable_kind_search_keeps_full_text_matching() {
        let conn = local_variable_fixture();
        ensure_fts_index(&conn).unwrap();

        let ids: Vec<String> = fts_search_symbols_scoped(
            &conn,
            "sqlite connection",
            Some("variable"),
            None,
            false,
            10,
        )
        .unwrap()
        .into_iter()
        .map(|r| r.symbol.symbol_id)
        .collect();

        assert!(ids.contains(&"global".to_string()));
        assert!(ids.contains(&"field".to_string()));
    }

    #[test]
    fn qualified_lookup_returns_the_named_local_variable() {
        let conn = local_variable_fixture();

        let ids: Vec<String> =
            search_symbols_scoped(&conn, "open_conn::conn", None, None, false, 10)
                .unwrap()
                .into_iter()
                .map(|s| s.symbol_id)
                .collect();

        assert_eq!(ids, vec!["local".to_string()]);
    }

    #[test]
    fn exact_local_variable_outranks_a_partial_global_match_within_the_limit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT,
                kind TEXT, signature TEXT, doc_comment TEXT, visibility TEXT,
                parent_symbol_id TEXT, start_line INTEGER, start_column INTEGER,
                end_line INTEGER, end_column INTEGER, start_byte INTEGER, end_byte INTEGER,
                body_start_line INTEGER, body_start_column INTEGER, body_end_line INTEGER,
                body_end_column INTEGER, body_start_byte INTEGER, body_end_byte INTEGER,
                body_hash TEXT, semantic_group TEXT, is_test INTEGER, test_container INTEGER
            );
            INSERT INTO symbols (symbol_id, file_id, path, language, name, kind, signature,
                                 parent_symbol_id, start_line, start_column, end_line, end_column,
                                 start_byte, end_byte, is_test, test_container)
            VALUES
                ('func', 'f1', 'src/sum.rs', 'rust', 'digest', 'function',
                 'fn digest()', NULL, 1, 0, 9, 1, 0, 100, 0, 0),
                ('local', 'f1', 'src/sum.rs', 'rust', 'checksum', 'variable',
                 'let checksum', 'func', 2, 4, 2, 30, 10, 40, 0, 0),
                ('global', 'f1', 'src/sum.rs', 'rust', 'getChecksum', 'variable',
                 'const getChecksum', NULL, 20, 0, 20, 30, 210, 240, 0, 0);",
        )
        .unwrap();
        ensure_fts_index(&conn).unwrap();

        let rows =
            fts_search_symbols_explained(&conn, "checksum", Some("variable"), None, false, 1, true)
                .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol.symbol_id, "local");
        let explain = rows[0].explain.as_ref().unwrap();
        assert_eq!(explain.name_tier, "whole");
        assert_eq!(explain.branches, vec!["exact", "name"]);
        assert_eq!(explain.candidates, 2);
    }

    #[test]
    fn variable_kind_filter_returns_locals_and_parameters() {
        let conn = local_variable_fixture();
        ensure_fts_index(&conn).unwrap();

        let lookup_ids: Vec<String> =
            search_symbols_scoped(&conn, "conn", Some("variable"), None, false, 10)
                .unwrap()
                .into_iter()
                .map(|s| s.symbol_id)
                .collect();
        assert!(lookup_ids.contains(&"local".to_string()));
        assert!(lookup_ids.contains(&"nested".to_string()));

        let search_ids: Vec<String> =
            fts_search_symbols_scoped(&conn, "conn", Some("variable"), None, false, 10)
                .unwrap()
                .into_iter()
                .map(|r| r.symbol.symbol_id)
                .collect();
        assert!(search_ids.contains(&"local".to_string()));
    }
}
