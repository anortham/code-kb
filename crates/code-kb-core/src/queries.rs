use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};
use rust_stemmers::{Algorithm, Stemmer};
use std::collections::{BTreeMap, HashMap, HashSet};
use thiserror::Error;

mod import_scope;

use crate::db::local_variable_predicate;
use crate::models::{
    ANONYMOUS_CLASS_PREFIX, BlastRadiusResult, FileFact, ImpactedSymbol, LiteralFact, OpenEnd,
    ReferenceSite, SearchExplain, StructuralFact, Symbol, SymbolSearchResult, TestTarget, TypeFact,
    has_generated_lambda_name, is_generated_lambda_name,
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
        "Ambiguous symbol '{0}': found {1} matching candidates. Choose a candidate by its kind and source line, then pass its id as `symbol_id` (CLI: `--symbol-id`) to get_symbol_body, get_symbol_context, find_references, or blast_radius:\n{2}"
    )]
    AmbiguousSymbol(String, usize, String),
    #[error("Invalid direction '{0}': must be 'callers' or 'callees'")]
    InvalidDirection(String),
    #[error("Result limit must be between 0 and {MAX_RESULT_LIMIT}, got {0}")]
    InvalidResultLimit(usize),
    #[error("Report page offset {offset} with limit {limit} overflows the platform page range")]
    InvalidResultPage { offset: usize, limit: usize },
}

pub const MAX_RESULT_LIMIT: usize = 200;

pub fn validate_result_limit(limit: usize) -> Result<(), QueryError> {
    if limit > MAX_RESULT_LIMIT {
        return Err(QueryError::InvalidResultLimit(limit));
    }
    Ok(())
}

/// Validate a per-page size and return its checked end offset.
pub fn validate_result_page(offset: usize, limit: usize) -> Result<usize, QueryError> {
    validate_result_limit(limit)?;
    let end = offset
        .checked_add(limit)
        .ok_or(QueryError::InvalidResultPage { offset, limit })?;
    Ok(end)
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

/// The most files one outline lists by name.
pub const OUTLINE_FILE_CAP: usize = 1000;

/// The path filter and depth of one outline, as SQL parameters.
pub struct OutlineScope {
    path: Option<String>,
    path_bs: Option<String>,
    prefix: Option<String>,
    prefix_bs: Option<String>,
    /// The most path separators a file the outline lists by name can have.
    max_slashes: i64,
}

impl OutlineScope {
    pub fn new(path_filter: Option<&str>, depth: usize) -> Self {
        let path = path_filter
            .map(|p| p.replace('\\', "/").trim_matches('/').to_string())
            .filter(|p| !p.is_empty());
        let path_bs = path.as_ref().map(|p| p.replace('/', "\\"));
        let max_slashes = match &path {
            None => depth.saturating_sub(1),
            Some(p) => p.matches('/').count() + depth,
        } as i64;
        Self {
            prefix: path.as_ref().map(|p| format!("{}/%", escape_like(p))),
            prefix_bs: path_bs.as_ref().map(|p| format!("{}\\\\%", escape_like(p))),
            path,
            path_bs,
            max_slashes,
        }
    }

    /// True when the outline lists `path` by name instead of counting it in its folder.
    pub fn lists(&self, path: &str) -> bool {
        path.matches(['/', '\\']).count() as i64 <= self.max_slashes
    }

    fn params(&self) -> [(&'static str, &dyn ToSql); 5] {
        [
            (":path", &self.path),
            (":path_bs", &self.path_bs),
            (":path_prefix", &self.prefix),
            (":path_prefix_bs", &self.prefix_bs),
            (":max_slashes", &self.max_slashes),
        ]
    }
}

/// The `WHERE` terms for the files in an outline scope that have an extractor.
fn outline_file_terms(conn: &Connection) -> String {
    let supported = if has_column(conn, "files", "status") {
        "COALESCE(status, '') != 'unsupported'"
    } else {
        "1 = 1"
    };
    format!(
        "(:path IS NULL
            OR path = :path COLLATE NOCASE
            OR path = :path_bs COLLATE NOCASE
            OR path LIKE :path_prefix ESCAPE '\\'
            OR path LIKE :path_prefix_bs ESCAPE '\\')
         AND {supported}"
    )
}

/// The files an outline lists by name, in path order.
fn outline_files_cte(conn: &Connection) -> String {
    format!(
        "outline_files AS (
            SELECT path FROM files
            WHERE {}
              AND length(path) - length(replace(replace(path, '/', ''), '\\', '')) <= :max_slashes
            ORDER BY path ASC
            LIMIT {OUTLINE_FILE_CAP}
        )",
        outline_file_terms(conn)
    )
}

/// Streams every file path in the outline scope that has an extractor, in path order.
pub fn for_each_outline_path(
    conn: &Connection,
    scope: &OutlineScope,
    mut visit: impl FnMut(String) -> bool,
) -> Result<(), QueryError> {
    let sql = format!(
        "SELECT path FROM files WHERE {} ORDER BY path ASC",
        outline_file_terms(conn)
    );
    let mut stmt = conn.prepare(&sql)?;
    let params = scope.params();
    let mut rows = stmt.query(&params[..4])?;
    while let Some(row) = rows.next()? {
        if !visit(row.get(0)?) {
            break;
        }
    }
    Ok(())
}

/// True when `alias` names a definition at the top of its file: no parent, or only a namespace,
/// module, or package around it.
fn outline_level(alias: &str) -> String {
    format!(
        "({alias}.parent_symbol_id IS NULL OR EXISTS (
            SELECT 1 FROM symbols ns WHERE ns.symbol_id = {alias}.parent_symbol_id
              AND ns.kind IN ('namespace', 'module', 'package')))"
    )
}

const OUTLINE_KINDS: &str =
    "('function', 'method', 'struct', 'enum', 'trait', 'class', 'interface', 'type')";

/// SQL mirror of `is_generated_lambda_name`: a Python lambda that julie names after its line.
fn generated_lambda_predicate(alias: &str) -> String {
    format!(
        "({alias}.language = 'python' AND {alias}.kind = 'function'
          AND {alias}.name GLOB 'lambda_[0-9]*' AND substr({alias}.name, 8) NOT GLOB '*[^0-9]*'
          AND COALESCE({alias}.signature, '') GLOB 'lambda*')"
    )
}

/// Load up to `limit_per_file` top-level definitions for each file the outline lists by name.
/// Files deeper than `depth` are filtered out in SQLite to keep memory strictly bounded. The
/// unary `+` keeps the overload check on the name index: the parent index would walk every
/// top-level symbol of the repository for each row.
pub fn load_scoped_outline_symbols(
    conn: &Connection,
    path_filter: Option<&str>,
    depth: usize,
    limit_per_file: usize,
) -> Result<HashMap<String, Vec<Symbol>>, QueryError> {
    let scope = OutlineScope::new(path_filter, depth);
    let sql = format!(
        "WITH {files},
        ranked AS (
            SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                   s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
                   s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
                   s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
                   s.is_test, s.test_container,
                   ROW_NUMBER() OVER (
                       PARTITION BY s.path ORDER BY (s.is_test = 1 OR s.test_container = 1), s.start_line ASC
                   ) as rn
            FROM symbols s
            JOIN outline_files bf ON s.path = bf.path
            WHERE s.kind IN {OUTLINE_KINDS}
              AND {top}
              AND NOT {lambda}
              AND NOT EXISTS (SELECT 1 FROM symbols overload
                              WHERE +overload.path = s.path AND overload.name = s.name
                                AND overload.kind = s.kind
                                AND +overload.parent_symbol_id IS s.parent_symbol_id
                                AND overload.start_line < s.start_line)
        )
        SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
               visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
               start_byte, end_byte, body_start_line, body_start_column, body_end_line,
               body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
               is_test, test_container
        FROM ranked
        WHERE rn <= :limit
        ORDER BY path ASC, start_line ASC",
        files = outline_files_cte(conn),
        top = outline_level("s"),
        lambda = generated_lambda_predicate("s"),
    );

    let mut stmt = conn.prepare(&sql)?;
    let limit = limit_per_file as i64;
    let mut params = scope.params().to_vec();
    params.push((":limit", &limit));
    let mut rows = stmt.query(&params[..])?;

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

/// How many names one outline row stands for.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OutlineCounts {
    /// Distinct top-level definitions that are not tests; overloads count once.
    pub definitions: usize,
    /// Tests at any level, without setup and fixtures.
    pub tests: usize,
    /// Setup, teardown, and fixture members.
    pub fixtures: usize,
}

/// For each file the outline lists by name: its top-level definitions that are not tests, its
/// tests, and its fixtures, so the outline can say how many names it left out.
pub fn load_outline_counts(
    conn: &Connection,
    path_filter: Option<&str>,
    depth: usize,
) -> Result<HashMap<String, OutlineCounts>, QueryError> {
    let scope = OutlineScope::new(path_filter, depth);
    let lifecycle = if has_column(conn, "symbols", "test_lifecycle") {
        "COALESCE(s.test_lifecycle, 0) != 0"
    } else {
        "0"
    };
    let mut stmt = conn.prepare(&format!(
        "WITH {files}
         SELECT replace(s.path, '\\', '/'),
                COUNT(DISTINCT CASE WHEN {top} AND s.is_test = 0 AND s.test_container = 0
                    AND s.kind IN {OUTLINE_KINDS} AND NOT {lambda}
                    THEN s.kind || ' ' || s.name END),
                SUM(s.is_test = 1 AND NOT {lifecycle}),
                SUM(s.is_test = 1 AND {lifecycle})
         FROM symbols s
         JOIN outline_files bf ON s.path = bf.path
         GROUP BY replace(s.path, '\\', '/')",
        files = outline_files_cte(conn),
        top = outline_level("s"),
        lambda = generated_lambda_predicate("s"),
    ))?;
    let rows = stmt.query_map(&scope.params()[..], |row| {
        Ok((
            row.get::<_, String>(0)?,
            OutlineCounts {
                definitions: row.get::<_, i64>(1)? as usize,
                tests: row.get::<_, i64>(2)? as usize,
                fixtures: row.get::<_, i64>(3)? as usize,
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
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

/// Streams the paths of files in the outline scope that no extractor reads.
pub fn for_each_unsupported_path(
    conn: &Connection,
    scope: &OutlineScope,
    mut visit: impl FnMut(&str),
) -> Result<(), QueryError> {
    if !has_column(conn, "files", "status") {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "SELECT path FROM files
         WHERE status = 'unsupported'
           AND (:path IS NULL
             OR path = :path COLLATE NOCASE
             OR path = :path_bs COLLATE NOCASE
             OR path LIKE :path_prefix ESCAPE '\\'
             OR path LIKE :path_prefix_bs ESCAPE '\\')",
    )?;
    let params = scope.params();
    let mut rows = stmt.query(&params[..4])?;
    while let Some(row) = rows.next()? {
        visit(&row.get::<_, String>(0)?);
    }
    Ok(())
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
/// True when `alias` is an attribute row such as `self.debug = …` whose name a base class already
/// defines with a value or a property, or whose own class defines it that way in another row than
/// a `self.` write (`@property def static_folder`). A base class named by a pending `extends` row counts only
/// when no class of that name is as close to the subclass's file, so an unclear base keeps the row. The row is a write to an inherited attribute, not a
/// definition, so lookup, search, and skeletons leave it out. `CASE` keeps SQLite from walking
/// the base classes of every row a search touches: it evaluates an `AND` with `EXISTS` eagerly.
pub(crate) fn inherited_attribute_write(alias: &str) -> String {
    let rival_rank = path_proximity("rival.path", "pe.path");
    let base_rank = path_proximity("c.path", "pe.path");
    format!(
        "(CASE WHEN {alias}.kind = 'property' AND {alias}.parent_symbol_id IS NOT NULL
          AND ({alias}.signature LIKE 'self.%' OR {alias}.signature LIKE 'this.%' OR {alias}.signature LIKE 'cls.%')
          THEN EXISTS (
            WITH RECURSIVE base(symbol_id, depth) AS (
                SELECT {alias}.parent_symbol_id, 0
                UNION
                SELECT r.to_symbol_id, base.depth + 1
                FROM relationships r JOIN base ON r.from_symbol_id = base.symbol_id
                WHERE +r.kind = 'extends' AND base.depth < 8
                UNION
                SELECT c.symbol_id, base.depth + 1
                FROM pending_relationships pe
                JOIN base ON pe.from_symbol_id = base.symbol_id
                JOIN symbols c ON c.name = pe.target_terminal_name
                WHERE +pe.kind = 'extends' AND base.depth < 8
                  AND c.symbol_id != {alias}.parent_symbol_id
                  AND c.kind IN ('class', 'interface', 'struct', 'trait', 'protocol')
                  AND NOT EXISTS (
                      SELECT 1 FROM symbols rival
                      WHERE rival.name = c.name AND rival.symbol_id != c.symbol_id
                        AND rival.kind IN ('class', 'interface', 'struct', 'trait', 'protocol')
                        AND {rival_rank} >= {base_rank})
            )
            SELECT 1 FROM base JOIN symbols inherited ON inherited.parent_symbol_id = base.symbol_id
            WHERE inherited.name = {alias}.name
              AND (base.depth > 0 OR NOT (COALESCE(inherited.signature, '') LIKE 'self.%'
                   OR inherited.signature LIKE 'this.%' OR inherited.signature LIKE 'cls.%'))
              AND (inherited.kind NOT IN ('variable', 'field') OR inherited.signature LIKE '%=%'))
          ELSE 0 END)"
    )
}

/// ` AND NOT` [`inherited_attribute_write`] for the row `s`, or nothing for an index without
/// relationship tables.
fn inherited_write_exclusion(conn: &Connection) -> String {
    if has_table(conn, "relationships") && has_table(conn, "pending_relationships") {
        format!(" AND NOT {}", inherited_attribute_write("s"))
    } else {
        String::new()
    }
}

/// True when `alias` is the top-level row that julie writes for an R global that a function
/// creates (`counter <<- 0`, `v ->> last`, `assign("glob", v, envir = .GlobalEnv)`), and the
/// global already has a definition: a top-level row of that name without such an `assignment`
/// (`counter <- 0`), or an earlier such row by path and line. All of them name one global, so the
/// later rows are writes to it, not definitions.
fn r_global_write(alias: &str) -> String {
    let created = |row: &str| {
        format!(
            "CASE WHEN json_valid({row}.metadata_json)
                  THEN json_extract({row}.metadata_json, '$.assignment') END IN ('<<-', '->>', 'assign')"
        )
    };
    format!(
        "(CASE WHEN {alias}.language = 'r' AND {alias}.parent_symbol_id IS NULL
               AND {alias}.kind IN ('variable', 'function') AND {alias_created}
          THEN EXISTS (
            SELECT 1 FROM symbols g
            WHERE g.name = {alias}.name AND g.language = 'r' AND g.parent_symbol_id IS NULL
              AND g.kind IN ('variable', 'function') AND g.symbol_id != {alias}.symbol_id
              AND (NOT COALESCE({g_created}, 0) OR g.path < {alias}.path
                   OR g.path = {alias}.path AND g.start_line < {alias}.start_line))
          ELSE 0 END)",
        alias_created = created(alias),
        g_created = created("g"),
    )
}

/// The ids among `ids` that [`inherited_attribute_write`] or [`r_global_write`] leaves out.
/// Callers check only the rows they return, because the check walks base classes and a common
/// word touches thousands of rows.
pub fn inherited_writes_among<'a>(
    conn: &Connection,
    ids: impl Iterator<Item = &'a str>,
) -> Result<HashSet<String>, QueryError> {
    if inherited_write_exclusion(conn).is_empty() {
        return Ok(HashSet::new());
    }
    let r_write = if has_column(conn, "symbols", "metadata_json") {
        format!(" OR {}", r_global_write("s"))
    } else {
        String::new()
    };
    let ids = serde_json::to_string(&ids.collect::<Vec<_>>()).unwrap_or_default();
    let mut stmt = conn.prepare(&format!(
        "SELECT s.symbol_id FROM symbols s
         WHERE s.symbol_id IN (SELECT value FROM json_each(?1)) AND ({}{r_write})",
        inherited_attribute_write("s")
    ))?;
    let found = stmt
        .query_map(params![ids], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(found)
}

/// The names among `names` that [`inherited_attribute_write`] would leave out as `self.` writes
/// in the class `class_id`. `check_edit` asks the current index about the rows of its base scan,
/// which holds only the changed files and so cannot see a base class in another file.
pub(crate) fn inherited_write_names<'a>(
    conn: &Connection,
    class_id: &str,
    names: impl Iterator<Item = &'a str>,
) -> Result<HashSet<String>, QueryError> {
    if inherited_write_exclusion(conn).is_empty() {
        return Ok(HashSet::new());
    }
    let names = serde_json::to_string(&names.collect::<Vec<_>>()).unwrap_or_default();
    let mut stmt = conn.prepare(&format!(
        "WITH s(parent_symbol_id, name, kind, signature) AS (
             SELECT ?1, value, 'property', 'self.' || value FROM json_each(?2))
         SELECT s.name FROM s WHERE {}",
        inherited_attribute_write("s")
    ))?;
    let found = stmt
        .query_map(params![class_id, names], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(found)
}

fn lookup_trigram_phrase(query: &str, kind_filter: Option<&str>) -> Option<String> {
    if kind_filter == Some("variable")
        || query.contains("::")
        || query.contains('.')
        || query.contains('\0')
        || query.chars().count() < 3
    {
        return None;
    }
    Some(format!("\"{}\"", query.replace('"', "\"\"")))
}

fn search_symbols_sql(
    variables_wanted: bool,
    include_tests: bool,
    limit: usize,
    use_trigram: bool,
) -> String {
    let (from, query_filter) = if use_trigram {
        (
            "symbol_names_tri CROSS JOIN symbols s ON s.rowid = symbol_names_tri.rowid",
            "symbol_names_tri MATCH :trigram AND (s.name = :query OR s.name LIKE :pattern ESCAPE '\\')",
        )
    } else {
        (
            "symbols s",
            "(s.name = :query OR s.name LIKE :pattern ESCAPE '\\')",
        )
    };
    let mut sql = format!(
        "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
                s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
                s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
                s.is_test, s.test_container
         FROM {from}
         WHERE {query_filter}
           AND {KIND_FILTER}
           AND (:path IS NULL OR replace(s.path, '\\', '/') = :path COLLATE NOCASE OR replace(s.path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(s.path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
    );
    sql.push_str(&document_link_exclusion());
    sql.push_str(&format!(
        " AND (+s.name = :query OR NOT {})",
        anonymous_class_predicate("s")
    ));

    if !variables_wanted {
        sql.push_str(&format!(" AND NOT {}", local_variable_predicate("s")));
    }

    if !include_tests {
        sql.push_str(&format!(
            " AND (+s.name = :query OR (s.is_test = 0 AND s.test_container = 0 AND NOT {}))",
            test_path_predicate("s")
        ));
    }

    sql.push_str(
        " ORDER BY (s.name = :query) DESC, (s.name LIKE :prefix ESCAPE '\\') DESC, (s.kind IN ('function', 'struct', 'class', 'trait', 'method', 'enum', 'interface', 'type')) DESC, ",
    );
    sql.push_str(&format!(
        "(s.language IN ({})) ASC, ",
        non_code_language_list()
    ));
    sql.push_str(
        "(COALESCE(s.signature, '') LIKE 'self.%') ASC, length(s.name) ASC, s.path ASC LIMIT ",
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
    if let Some(owner) = query
        .strip_suffix("::")
        .or_else(|| query.strip_suffix('.'))
        .and_then(|owner| owner.rsplit(['.', ':']).next())
        .filter(|owner| !owner.is_empty())
    {
        return owner_members(
            conn,
            owner,
            norm_kind.as_deref(),
            path_filter,
            include_tests,
            limit,
        );
    }
    if (query.contains("::") || query.contains('.'))
        && let Some(sym) = get_symbol_by_name(conn, query, path_filter)?
    {
        let kind_matches = norm_kind
            .as_deref()
            .is_none_or(|kind| sym.kind == kind || crate::formatters::display_kind(&sym) == kind);
        return Ok(if kind_matches { vec![sym] } else { Vec::new() });
    }

    let pattern = format!("%{}%", escape_like(query));
    let prefix = format!("{}%", escape_like(query));
    let normalized_path = path_filter.map(|p| {
        p.replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    });
    let escaped_path = normalized_path.as_deref().map(escape_like);

    let path_val = normalized_path.as_deref();
    let path_like = escaped_path.as_deref();
    let kind_val = norm_kind.as_deref();
    let trigram_phrase =
        lookup_trigram_phrase(query, kind_val).filter(|_| crate::db::fts_index_is_ready(conn));
    let mut params: Vec<(&str, &dyn ToSql)> = vec![
        (":query", &query),
        (":pattern", &pattern),
        (":prefix", &prefix),
        (":kind", &kind_val),
        (":path", &path_val),
        (":path_like", &path_like),
    ];
    if let Some(phrase) = trigram_phrase.as_ref() {
        params.push((":trigram", phrase));
    }
    let mut fetch = 4 * limit;
    loop {
        let sql = search_symbols_sql(
            norm_kind.as_deref() == Some("variable"),
            include_tests,
            fetch,
            trigram_phrase.is_some(),
        );
        #[cfg(test)]
        if trigram_phrase.is_some() {
            LOOKUP_TRIGRAM_EXECUTIONS.with(|count| count.set(count.get() + 1));
        }
        let rows = conn
            .prepare(&sql)?
            .query_map(params.as_slice(), map_symbol)?
            .collect::<Result<Vec<Symbol>, _>>()?;
        let fetched = rows.len();
        let inherited = inherited_writes_among(conn, rows.iter().map(|s| s.symbol_id.as_str()))?;
        let mut kept: Vec<Symbol> = rows
            .into_iter()
            .filter(|s| !inherited.contains(&s.symbol_id))
            .collect();
        let has_definition = kept.iter().any(|s| s.kind != "import");
        let mut counted = 0;
        let past_limit = kept.iter().position(|s| {
            if !folds_into_import_line(query, s, has_definition) {
                counted += 1;
            }
            counted > limit
        });
        if let Some(cut) = past_limit {
            kept.truncate(cut);
            return Ok(kept);
        }
        if fetched < fetch || fetch >= 4 * MAX_RESULT_LIMIT {
            return Ok(kept);
        }
        fetch *= 2;
    }
}

const MEMBER_LIST_OWNER_KINDS: &str = "'class', 'struct', 'interface', 'trait', 'enum', 'record', 'object', \
                           'protocol', 'union', 'module', 'namespace'";

/// SQL predicate for the `:kind` filter over `symbols s`: the stored kind or the kind that
/// `formatters::display_kind` shows, so a kind copied from the output always filters.
const KIND_FILTER: &str = "(:kind IS NULL OR s.kind = :kind OR :kind = CASE
      WHEN s.language = 'html' AND s.kind = 'class' THEN 'element'
      WHEN s.language = 'sql' AND s.kind = 'class' THEN 'table'
      WHEN s.language = 'markdown' AND s.kind = 'module' THEN 'section'
      WHEN s.language = 'markdown' AND s.kind = 'import' THEN 'link'
      WHEN s.kind = 'property' AND s.language <> 'fsharp'
           AND (s.signature GLOB 'self.*' OR s.signature GLOB 'this.*' OR s.signature GLOB 'cls.*')
        THEN 'attribute'
    END)";

/// The members of the classes, modules, and namespaces named `owner`, in source order: what
/// `lookup_symbol("App.")` lists.
fn owner_members(
    conn: &Connection,
    owner: &str,
    kind: Option<&str>,
    path_filter: Option<&str>,
    include_tests: bool,
    limit: usize,
) -> Result<Vec<Symbol>, QueryError> {
    let path = path_filter.map(|p| {
        p.replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    });
    let path_like = path.as_deref().map(escape_like);
    let tests = if include_tests {
        String::new()
    } else {
        format!(
            " AND +s.is_test = 0 AND +s.test_container = 0 AND NOT {}",
            test_path_predicate("s")
        )
    };
    let docs = documentation_language_list();
    let sql = format!(
        "SELECT s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
                s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
                s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
                s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
                s.is_test, s.test_container
         FROM symbols o JOIN symbols s ON s.parent_symbol_id = o.symbol_id
         WHERE o.name = :owner
           AND o.kind IN ({MEMBER_LIST_OWNER_KINDS})
           AND (o.language NOT IN ({docs})
                OR NOT EXISTS (SELECT 1 FROM symbols c WHERE c.name = :owner
                                 AND c.kind IN ({MEMBER_LIST_OWNER_KINDS}) AND c.language NOT IN ({docs})))
           AND s.kind NOT IN ('parameter', 'import')
           AND {KIND_FILTER}
           AND (:path IS NULL OR replace(s.path, '\\', '/') = :path COLLATE NOCASE
                OR replace(s.path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\'
                OR replace(s.path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\'){tests}
         ORDER BY s.path, s.start_line"
    );
    let rows = conn
        .prepare(&sql)?
        .query_map(
            rusqlite::named_params! {
                ":owner": owner,
                ":kind": kind,
                ":path": path.as_deref(),
                ":path_like": path_like.as_deref(),
            },
            map_symbol,
        )?
        .collect::<Result<Vec<Symbol>, _>>()?;
    let inherited = inherited_writes_among(conn, rows.iter().map(|s| s.symbol_id.as_str()))?;
    Ok(rows
        .into_iter()
        .filter(|s| !inherited.contains(&s.symbol_id))
        .take(limit)
        .collect())
}

/// True when lookup shows `row` inside the one line that counts the imports of `query`, not as a
/// row of its own: an import of that exact name, when some row is a definition. Such rows do not
/// count toward the lookup limit.
pub fn folds_into_import_line(query: &str, row: &Symbol, has_definition: bool) -> bool {
    has_definition
        && row.kind == "import"
        && (row.name == query
            || row.name.ends_with(&format!(".{query}"))
            || row.name.ends_with(&format!("::{query}")))
}

/// Sanitizes a free-form user query into `(and_query, or_query)` formatted for SQLite FTS5.
/// Each alphanumeric/underscore token is quoted and given a prefix wildcard: `"token"*`.
/// Identifiers are split at case boundaries too (`parseHTTPResponse` -> `parse HTTP Response`),
/// so a camelCase query finds a snake_case symbol. Each split identifier keeps its unsplit form
/// as an alternative inside its own required term, so a camelCase symbol, which FTS5 indexes as
/// one token, still matches. A query of two or three words also tries their concatenation.
/// English stop words are dropped unless the whole query is stop words, and tokens under three
/// characters get no prefix wildcard. A digit part (`1` in `zzq1_thing`) is left out of the OR
/// query when another part remains, because it matches every `= 1` in the index.
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
    let any_word = split.iter().any(|(parts, _)| {
        parts
            .iter()
            .any(|p| (!any_content || !is_stop_word(p)) && !is_digit_word(p))
    });

    let mut and_groups: Vec<String> = Vec::new();
    let mut or_terms: Vec<String> = Vec::new();
    for (parts, whole) in split {
        let parts: Vec<&str> = parts
            .into_iter()
            .filter(|p| !any_content || !is_stop_word(p))
            .collect();
        let or_parts = parts
            .iter()
            .filter(|p| !any_word || !is_digit_word(p))
            .map(|p| fts5_term(p));
        or_terms.extend(or_parts);
        let parts: Vec<String> = parts.into_iter().map(fts5_term).collect();
        let whole = whole.map(fts5_term);
        let group = match (parts.is_empty(), whole.as_deref()) {
            (true, None) => continue,
            (true, Some(w)) => w.to_string(),
            (false, None) => parts.join(" "),
            (false, Some(w)) => format!("(({}) OR {w})", parts.join(" ")),
        };
        and_groups.push(group);
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
/// deduplicated. Stop words are dropped unless every term is a stop word, and digit terms unless
/// every term is digits. Empty when no term qualifies.
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
    let any_word = terms.iter().any(|t| !is_digit_word(t));
    terms.retain(|t| !any_word || !is_digit_word(t));
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
#[derive(Clone)]
pub(crate) struct Candidate {
    pub result: SymbolSearchResult,
    pub bm25: Option<f64>,
    pub exact_name: bool,
    pub word_match: bool,
    pub name_match: bool,
    /// Admitted as a member of a class that a query word names.
    pub owner_match: bool,
    pub name_terms: Vec<String>,
    pub documentation: bool,
    /// A function or method declared inside another function or method.
    pub nested: bool,
    /// The class or other type the row is a member of.
    pub owner: Option<String>,
    /// Weak context from a highly ranked code type; never substitutes for a member's own hit.
    owner_context: Option<String>,
    /// The module that the file declares or that a manifest gives it: a Go package clause, a
    /// qmldir `module` line, the `name` line of the Cargo package (or its `[lib]`) for a Rust
    /// crate root, a Dart `library` name, the SwiftPM target that holds a Swift file, or the
    /// `build.zig` module or artifact of a Zig root source file.
    pub file_module: Option<String>,
    /// The module or namespace the row is a direct member of, when code names the row through
    /// it: an Elixir, Ruby, F#, or Erlang module, a Rust `mod`, a TypeScript namespace, or a
    /// Scala package object.
    pub module_owner: Option<String>,
}

/// The kinds of symbol whose members search and lookup name as `Owner.member`.
const OWNER_KINDS: &str =
    "('class', 'struct', 'interface', 'trait', 'enum', 'record', 'object', 'protocol', 'union')";

fn candidate_columns(conn: &Connection) -> String {
    format!(
        "s.rowid AS row_id, s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind,
                s.signature, s.doc_comment, s.visibility, s.parent_symbol_id, s.start_line,
                s.start_column, s.end_line, s.end_column, s.start_byte, s.end_byte,
                s.body_start_line, s.body_start_column, s.body_end_line, s.body_end_column,
                s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group, s.is_test,
                s.test_container,
                (s.language IN ({doc_langs}) OR NOT ({not_doc})) AS documentation,
                (s.kind IN ('function', 'method')
                 AND EXISTS (SELECT 1 FROM symbols nest WHERE nest.symbol_id = s.parent_symbol_id
                             AND nest.kind IN ('function', 'method', 'constructor')
                             AND s.start_byte > nest.start_byte AND s.end_byte <= nest.end_byte)) AS nested,
                (SELECT o.name FROM symbols o WHERE o.symbol_id = s.parent_symbol_id
                   AND o.kind IN {OWNER_KINDS}) AS owner_name",
        doc_langs = documentation_language_list(),
        not_doc = not_documentation(conn, "s")
    )
}

/// The module a file declares. A Go file names its package and a qmldir file its module on
/// one top-level `module` row. A Rust crate root (`src/lib.rs`, `src/main.rs`) takes the
/// `name` of the `[lib]` or `[package]` table of the `Cargo.toml` beside its `src` folder; the
/// signature of that row is `name = "my-crate"`. A Dart file names its library on a top-level
/// `namespace` row of type `library` (`library blog.posts;`), and a part names it on its
/// `part of blog.posts;` import row; a part that names its library by URI names no module.
const FILE_MODULE_COLUMN: &str = "CASE
      WHEN s.language = 'dart' THEN
        (SELECT f.name FROM symbols f WHERE f.file_id = s.file_id AND f.parent_symbol_id IS NULL
           AND CASE WHEN json_valid(f.metadata_json) THEN
                 CASE json_extract(f.metadata_json, '$.type')
                   WHEN 'library' THEN f.kind = 'namespace'
                   WHEN 'part_of' THEN f.kind = 'import' AND f.name NOT GLOB '*[/:]*'
                     AND f.name NOT GLOB '*.dart' END END
         ORDER BY f.start_line LIMIT 1)
      WHEN s.language IN ('go', 'qmldir') THEN
        (SELECT f.name FROM symbols f WHERE f.file_id = s.file_id AND f.kind = 'module'
           AND f.parent_symbol_id IS NULL ORDER BY f.start_line LIMIT 1)
      WHEN s.language = 'rust' AND (replace(s.path, '\\', '/') = 'src/lib.rs'
           OR replace(s.path, '\\', '/') GLOB '*/src/lib.rs') THEN
        (SELECT c.signature FROM symbols c JOIN symbols t ON t.symbol_id = c.parent_symbol_id
           WHERE c.path = substr(s.path, 1, length(s.path) - 10) || 'Cargo.toml'
             AND c.name = 'name' AND t.name IN ('lib', 'package') AND t.kind = 'module'
           ORDER BY t.name = 'lib' DESC LIMIT 1)
      WHEN s.language = 'rust' AND (replace(s.path, '\\', '/') = 'src/main.rs'
           OR replace(s.path, '\\', '/') GLOB '*/src/main.rs') THEN
        (SELECT c.signature FROM symbols c JOIN symbols t ON t.symbol_id = c.parent_symbol_id
           WHERE c.path = substr(s.path, 1, length(s.path) - 11) || 'Cargo.toml'
             AND c.name = 'name' AND t.name = 'package' AND t.kind = 'module' LIMIT 1)
    END";

/// The module or namespace a row is a direct member of, when code writes that module to name
/// the row (`MyApp.Blog.render`, `inner::render`). A C#, C++, PHP, Java, or Kotlin namespace or
/// package wraps whole files and code names the type in it, so it credits nothing; a Rust
/// `mod tests` is test code.
const MODULE_OWNER_COLUMN: &str = "(SELECT m.name FROM symbols m
      WHERE m.symbol_id = s.parent_symbol_id AND m.test_container = 0
        AND ((m.kind = 'module' AND m.language IN ('elixir', 'ruby', 'fsharp', 'erlang'))
          OR (m.kind = 'namespace' AND (m.language IN ('rust', 'typescript', 'tsx')
            OR (m.language = 'scala' AND m.signature GLOB 'package object *')))))";

/// The languages whose modules `MODULE_OWNER_COLUMN` reads.
const MODULE_OWNER_LANGUAGES: &[&str] = &[
    "elixir",
    "ruby",
    "fsharp",
    "erlang",
    "rust",
    "typescript",
    "tsx",
    "scala",
];

/// Whether `FILE_MODULE_COLUMN` can name a module for a row of this file.
fn may_declare_file_module(language: &str, path: &str) -> bool {
    match language {
        "go" | "qmldir" | "dart" => true,
        "rust" => {
            let path = path.replace('\\', "/");
            ["src/lib.rs", "src/main.rs"]
                .iter()
                .any(|root| path == *root || path.ends_with(&format!("/{root}")))
        }
        _ => false,
    }
}

/// The most package manifest rows that one search reads.
const MANIFEST_MODULE_ROWS: i64 = 4096;

/// The module names that package manifests give to source files, read once per search and only
/// for a language that has a candidate: a SwiftPM target names every file in its folder, and a
/// `build.zig` module or artifact names its root source file.
#[derive(Default)]
struct ManifestModules {
    swift_targets: Option<Vec<(String, String)>>,
    zig_roots: Option<HashMap<String, String>>,
}

impl ManifestModules {
    /// The target whose folder holds `path`; the deepest folder wins.
    fn swift_target(
        &mut self,
        conn: &Connection,
        path: &str,
    ) -> Result<Option<String>, QueryError> {
        if self.swift_targets.is_none() {
            self.swift_targets = Some(swift_target_folders(conn)?);
        }
        let path = path.replace('\\', "/");
        Ok(self
            .swift_targets
            .iter()
            .flatten()
            .filter(|(folder, _)| path.starts_with(&format!("{folder}/")))
            .max_by_key(|(folder, _)| folder.len())
            .map(|(_, name)| name.clone()))
    }

    fn zig_module(&mut self, conn: &Connection, path: &str) -> Result<Option<String>, QueryError> {
        if self.zig_roots.is_none() {
            self.zig_roots = Some(zig_root_modules(conn)?);
        }
        let path = path.replace('\\', "/");
        Ok(self
            .zig_roots
            .as_ref()
            .and_then(|roots| roots.get(&path).cloned()))
    }
}

/// `path` without `.` segments and with each `name/..` removed, or `None` when it leaves the root.
fn normalized_relative_path(path: &str) -> Option<String> {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            segment => segments.push(segment),
        }
    }
    (!segments.is_empty()).then(|| segments.join("/"))
}

fn joined_path(folder: &str, relative: &str) -> Option<String> {
    normalized_relative_path(&format!("{folder}/{relative}"))
}

/// The folder and name of each SwiftPM target (`swiftpm.target.v1`). A target without `path:`
/// lives in `Sources/<name>` (`Tests/<name>` for a test target), and SwiftPM takes the first of
/// `Sources`, `Source`, `src`, and `srcs` (`Tests` first for a test target) that the package
/// holds; julie does not read the disk, so an indexed file under the folder shows that it exists.
fn swift_target_folders(conn: &Connection) -> Result<Vec<(String, String)>, QueryError> {
    if !has_table(conn, "structural_facts") {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT path, metadata_json FROM structural_facts
         WHERE pattern_id = 'swiftpm.target.v1' AND language = 'swift'
           AND json_valid(metadata_json)
         ORDER BY path, start_line LIMIT ?1",
    )?;
    let facts = stmt
        .query_map([MANIFEST_MODULE_ROWS], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut holds_files =
        conn.prepare("SELECT 1 FROM files WHERE path > ?1 AND path < ?2 LIMIT 1")?;
    let mut existing: HashMap<String, bool> = HashMap::new();
    let mut folders = Vec::new();
    for (manifest, metadata) in facts {
        let Ok(fact) = serde_json::from_str::<serde_json::Value>(&metadata) else {
            continue;
        };
        let (Some(name), Some(effective)) =
            (fact["name"].as_str(), fact["effective_path"].as_str())
        else {
            continue;
        };
        let package = crate::db::parent_folder(&manifest).replace('\\', "/");
        let folder = match effective.split_once('/') {
            Some((root, rest)) if fact["path_origin"].as_str() == Some("default") => {
                let roots: &[&str] = match root {
                    "Sources" => &["Sources", "Source", "src", "srcs"],
                    "Tests" => &["Tests", "Sources", "Source", "src", "srcs"],
                    _ => &[],
                };
                let mut chosen = root;
                for candidate in roots {
                    let Some(prefix) = joined_path(&package, candidate) else {
                        continue;
                    };
                    let exists = match existing.get(&prefix) {
                        Some(exists) => *exists,
                        None => {
                            let exists = holds_files
                                .exists(params![format!("{prefix}/"), format!("{prefix}0")])?;
                            existing.insert(prefix, exists);
                            exists
                        }
                    };
                    if exists {
                        chosen = candidate;
                        break;
                    }
                }
                joined_path(&package, &format!("{chosen}/{rest}"))
            }
            _ => joined_path(&package, effective),
        };
        if let Some(folder) = folder {
            folders.push((folder, name.to_string()));
        }
    }
    Ok(folders)
}

/// The root source file of each `build.zig` module (`b.addModule("blogz", …)`) and, when no
/// module names it, of each artifact other than a test (`b.addExecutable(.{ .name = "tool", … })`),
/// with that name. The path in `build.zig` is relative to its folder.
fn zig_root_modules(conn: &Connection) -> Result<HashMap<String, String>, QueryError> {
    let mut roots = HashMap::new();
    if !has_table(conn, "structural_facts") {
        return Ok(roots);
    }
    let mut stmt = conn.prepare(
        "SELECT path, metadata_json FROM structural_facts
         WHERE pattern_id IN ('zig.build_module.v1', 'zig.build_artifact.v1') AND language = 'zig'
           AND json_valid(metadata_json)
         ORDER BY pattern_id DESC, path, start_line LIMIT ?1",
    )?;
    let facts = stmt
        .query_map([MANIFEST_MODULE_ROWS], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (build, metadata) in facts {
        let Ok(fact) = serde_json::from_str::<serde_json::Value>(&metadata) else {
            continue;
        };
        let name = fact["module_name"].as_str().or_else(|| {
            (fact["artifact_kind"].as_str() != Some("test"))
                .then(|| fact["artifact_name"].as_str())
                .flatten()
        });
        let (Some(name), Some(root)) = (name, fact["root_source_file"].as_str()) else {
            continue;
        };
        if let Some(root) = joined_path(crate::db::parent_folder(&build), root) {
            roots.entry(root).or_insert_with(|| name.to_string());
        }
    }
    Ok(roots)
}

/// Fills the module facts of each candidate with one primary-key query per row that can have
/// one, so the cost follows the admitted candidates, not the rows a branch scans. A Swift or Zig
/// row takes its module from the package manifests instead.
fn load_module_facts(conn: &Connection, candidates: &mut [Candidate]) -> Result<(), QueryError> {
    let file_module_column = if has_column(conn, "symbols", "metadata_json") {
        FILE_MODULE_COLUMN
    } else {
        "NULL"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {file_module_column}, {MODULE_OWNER_COLUMN} FROM symbols s WHERE s.symbol_id = ?1"
    ))?;
    let mut manifests = ManifestModules::default();
    for candidate in candidates {
        let symbol = &candidate.result.symbol;
        match symbol.language.as_str() {
            "swift" => {
                candidate.file_module = manifests.swift_target(conn, &symbol.path)?;
                continue;
            }
            "zig" => {
                candidate.file_module = manifests.zig_module(conn, &symbol.path)?;
                continue;
            }
            _ => {}
        }
        let may_have_module_owner = symbol.parent_symbol_id.is_some()
            && MODULE_OWNER_LANGUAGES.contains(&symbol.language.as_str());
        if !may_have_module_owner && !may_declare_file_module(&symbol.language, &symbol.path) {
            continue;
        }
        let facts = stmt
            .query_row([&candidate.result.symbol.symbol_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?;
        (candidate.file_module, candidate.module_owner) = facts.unwrap_or((None, None));
    }
    Ok(())
}

/// julie records a Markdown or other documentation link as an `import` row; no lookup or search
/// for code wants it.
/// SQL mirror of [`is_anonymous_class`]: a class that code-kb names `<anonymous Task in make>`.
/// No query names it, so ranked search leaves it out, and lookup lists it only by its whole
/// name. Its members stay.
pub(crate) fn anonymous_class_predicate(alias: &str) -> String {
    format!("({alias}.kind = 'class' AND {alias}.name GLOB '{ANONYMOUS_CLASS_PREFIX}*')")
}

fn document_link_exclusion() -> String {
    format!(
        " AND NOT (s.kind = 'import' AND s.language IN ({}))",
        documentation_language_list()
    )
}

fn candidate_filters(searching_variables: bool, include_tests: bool) -> String {
    let mut sql = format!(
        " AND {KIND_FILTER}
          AND (:path IS NULL OR replace(s.path, '\\', '/') = :path COLLATE NOCASE OR replace(s.path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(s.path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
    );
    sql.push_str(&document_link_exclusion());
    sql.push_str(&format!(" AND NOT {}", anonymous_class_predicate("s")));
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

fn map_search_candidate(row: &Row, terms: &[String]) -> rusqlite::Result<(i64, Candidate)> {
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
        owner_match: false,
        name_terms,
        documentation: row.get::<_, Option<i64>>("documentation")? == Some(1),
        nested: row.get::<_, Option<i64>>("nested")? == Some(1),
        owner: row.get("owner_name")?,
        owner_context: None,
        file_module: None,
        module_owner: None,
    };
    Ok((row.get("row_id")?, candidate))
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

    let new_candidate = |row: &Row| map_search_candidate(row, &terms);

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut by_rowid: HashMap<i64, usize> = HashMap::new();
    let mut admit = |rowid: i64, incoming: Candidate| match by_rowid.get(&rowid).copied() {
        Some(i) => {
            let existing = &mut candidates[i];
            existing.exact_name |= incoming.exact_name;
            existing.word_match |= incoming.word_match;
            existing.name_match |= incoming.name_match;
            existing.owner_match |= incoming.owner_match;
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

    let owner_sql = if has_trigram {
        format!(
            "SELECT {columns} FROM symbol_names_tri
             CROSS JOIN symbols o ON o.rowid = symbol_names_tri.rowid
             CROSS JOIN symbols s ON s.parent_symbol_id = o.symbol_id
             WHERE symbol_names_tri MATCH :phrase AND length(o.name) = length(:word)
               AND o.kind IN {OWNER_KINDS} {filters}
             ORDER BY s.path ASC, s.start_line ASC LIMIT {name_cap}"
        )
    } else {
        format!(
            "SELECT {columns} FROM symbols o
             CROSS JOIN symbols s ON s.parent_symbol_id = o.symbol_id
             WHERE o.name = :word COLLATE NOCASE AND :phrase IS NOT NULL
               AND o.kind IN {OWNER_KINDS} {filters}
             ORDER BY s.path ASC, s.start_line ASC LIMIT {name_cap}"
        )
    };
    let mut owner_stmt = conn.prepare(&owner_sql)?;
    let owner_words = rerank_words(query);
    let any_word = owner_words.iter().any(|w| !is_digit_word(w));
    for word in owner_words
        .iter()
        .filter(|w| w.len() >= 3 && (!any_word || !is_digit_word(w)))
    {
        let phrase = format!("\"{word}\"");
        let owner_rows = owner_stmt
            .query_map(
                rusqlite::named_params! {
                    ":word": word,
                    ":phrase": phrase,
                    ":kind": kind_val,
                    ":path": path_val,
                    ":path_like": path_like,
                },
                new_candidate,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        for (rowid, mut candidate) in owner_rows {
            candidate.owner_match = true;
            admit(rowid, candidate);
        }
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
        #[cfg(test)]
        WORD_QUERY_EXECUTIONS.with(|count| count.set(count.get() + 1));
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
        if and_q != or_q && word_admitted.len() < word_cap {
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

    let inherited = inherited_writes_among(
        conn,
        candidates
            .iter()
            .map(|c| c.result.symbol.symbol_id.as_str()),
    )?;
    candidates.retain(|c| !inherited.contains(&c.result.symbol.symbol_id));
    load_module_facts(conn, &mut candidates)?;
    Ok(candidates)
}

#[cfg(test)]
thread_local! {
    static WORD_QUERY_EXECUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LOOKUP_TRIGRAM_EXECUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Follow only a few highly ranked code types, one level down. A member must still match
/// the query itself; the owner's context can supply only weaker, otherwise missing terms.
fn expand_ranked_owner_members(
    conn: &Connection,
    candidates: &mut Vec<Candidate>,
    ranked: &[(SymbolSearchResult, SearchExplain)],
    query: &str,
    kind_filter: Option<&str>,
    path_filter: Option<&str>,
    include_tests: bool,
) -> Result<bool, QueryError> {
    // ponytail: First 3 code types in the top 20, at most 40 direct members each.
    // Raise these ceilings only when a larger held-out evaluation validates the recall need.
    let owners: Vec<_> = ranked
        .iter()
        .take(20)
        .filter(|(result, explain)| {
            explain.documentation == 0.0
                && OWNER_KINDS.contains(&format!("'{}'", result.symbol.kind))
        })
        .take(3)
        .map(|(result, _)| &result.symbol)
        .collect();
    if owners.is_empty() {
        return Ok(false);
    }
    let normalized_path = path_filter.map(|p| {
        p.replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string()
    });
    let escaped_path = normalized_path.as_deref().map(escape_like);
    let norm_kind = kind_filter.map(normalize_kind);
    let columns = candidate_columns(conn);
    let filters = candidate_filters(norm_kind.as_deref() == Some("variable"), include_tests);
    let mut stmt = conn.prepare(&format!(
        "SELECT {columns} FROM symbols s WHERE s.parent_symbol_id = :owner {filters}
         ORDER BY s.start_line, s.start_column, s.symbol_id LIMIT 40"
    ))?;
    let terms = trigram_name_terms(query);
    let stemmer = Stemmer::create(Algorithm::English);
    let words = stemmed_query_words(query, &stemmer);
    let mut members = Vec::new();
    for owner in owners {
        let context = format!(
            "{} {}",
            owner.name,
            head_bytes(owner.doc_comment.as_deref().unwrap_or(""), TEXT_HEAD_BYTES)
        );
        let rows = stmt.query_map(
            rusqlite::named_params! {
                ":owner": owner.symbol_id,
                ":kind": norm_kind.as_deref(),
                ":path": normalized_path.as_deref(),
                ":path_like": escaped_path.as_deref(),
            },
            |row| map_search_candidate(row, &terms),
        )?;
        for row in rows {
            let (_, mut member) = row?;
            let symbol = &member.result.symbol;
            let mut tokens = Vec::new();
            let named = name_hits(&symbol.name, &words, &stemmer)
                .iter()
                .any(|hit| *hit > 0);
            let text_match = [symbol.signature.as_deref(), symbol.doc_comment.as_deref()]
                .into_iter()
                .any(|text| {
                    text_hits(
                        text.map(|s| head_bytes(s, TEXT_HEAD_BYTES)),
                        &words,
                        &mut tokens,
                    )
                    .into_iter()
                    .any(|hit| hit)
                });
            if named || text_match {
                member.owner_context = Some(context.clone());
                members.push(member);
            }
        }
    }
    let inherited = inherited_writes_among(
        conn,
        members.iter().map(|c| c.result.symbol.symbol_id.as_str()),
    )?;
    members.retain(|member| !inherited.contains(&member.result.symbol.symbol_id));
    load_module_facts(conn, &mut members)?;
    let mut changed = false;
    for member in members {
        let id = &member.result.symbol.symbol_id;
        if let Some(existing) = candidates
            .iter_mut()
            .find(|c| c.result.symbol.symbol_id == *id)
        {
            existing.owner_context = member.owner_context;
        } else {
            candidates.push(member);
        }
        changed = true;
    }
    Ok(changed)
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
        let mut sql = format!(
            "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                    visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                    start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                    body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                    is_test, test_container
              FROM symbols s
              WHERE (name = :query OR name LIKE :pattern ESCAPE '\\')
                AND {KIND_FILTER}
                AND (:path IS NULL OR replace(path, '\\', '/') = :path COLLATE NOCASE OR replace(path, '\\', '/') LIKE :path_like || '/%' ESCAPE '\\' OR replace(path, '\\', '/') LIKE '%/' || :path_like ESCAPE '\\')",
        );
        sql.push_str(&document_link_exclusion());
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

    let mut candidates =
        collect_search_candidates(conn, query, kind_filter, path_filter, include_tests, limit)?;
    let started = std::time::Instant::now();
    let idf = idf_weights(conn, &rerank_words(query));
    let mut ranked = rerank_with(candidates.clone(), query, include_tests, Some(&idf));
    let mut rerank_us = started.elapsed().as_micros();
    let mut candidate_count = candidates.len();
    if expand_ranked_owner_members(
        conn,
        &mut candidates,
        &ranked,
        query,
        kind_filter,
        path_filter,
        include_tests,
    )? {
        candidate_count = candidates.len();
        let started = std::time::Instant::now();
        ranked = rerank_with(candidates, query, include_tests, Some(&idf));
        rerank_us += started.elapsed().as_micros();
    }
    let query_words = rerank_words(query);
    if query_words.iter().any(|word| !is_digit_word(word)) {
        ranked.retain(|(_, breakdown)| holds_a_query_word(breakdown));
    }
    let unique_query_terms: HashSet<String> = query_words.into_iter().collect();
    let min_multi_term_results = ((limit * 3).div_ceil(4)).max(15).min(limit);
    let multi_term_results = ranked
        .iter()
        .filter(|(_, breakdown)| covered_query_term_count(&breakdown.terms) >= 2)
        .count();
    if unique_query_terms.len() > 1 && multi_term_results >= min_multi_term_results {
        let mut strong_results_seen = 0;
        ranked.retain(|(_, breakdown)| {
            if covered_query_term_count(&breakdown.terms) >= 2 {
                strong_results_seen += 1;
                true
            } else {
                strong_results_seen < min_multi_term_results || has_distinctive_name_term(breakdown)
            }
        });
    }
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
/// A helper declared inside a function ranks below a module or class member of equal score.
const W_NESTED: f64 = -2.0;
const W_PATH_ROLE: f64 = -10.0;
const W_DOCUMENTATION_ROW: f64 = -200.0;
const W_TEST_INTENT: f64 = 5.0;
const W_TERMS: f64 = 52.0;
const MAX_TERM_CREDIT: f64 = 3.0;
const TEXT_CREDIT: f64 = 1.0;
const FIELD_DIVERSITY_BONUS: f64 = 6.0;
const DISTINCTIVE_NAME_IDF_FACTOR: f64 = 1.1;
/// A word that names the class a member belongs to: `Flask init` means `Flask.__init__`.
const OWNER_CREDIT: f64 = 3.0;
const OWNER_CONTEXT_CREDIT: f64 = 0.5;
/// A word that names the row's module, as `blog` names `flaskr/blog.py` and `blog/__init__.py`.
const PATH_CREDIT: f64 = 0.5;
const TEXT_HEAD_BYTES: usize = 400;
const DOC_HEAD_BYTES: usize = 640;

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
/// Folders whose rows never earn a path word credit, so a query cannot lift copied code.
const UNCREDITED_PATH_SEGMENTS: &[&str] = &["fixtures", "vendor", "vendored", "third_party"];
const TEST_INTENT_WORDS: &[&str] = &["test", "tests", "spec", "specs"];

struct QueryWord {
    word: String,
    stem: String,
}

fn stemmed_query_words(query: &str, stemmer: &Stemmer) -> Vec<QueryWord> {
    rerank_words(query)
        .into_iter()
        .map(|word| QueryWord {
            stem: stemmer.stem(&word).into_owned(),
            word,
        })
        .collect()
}

/// Per-word hits of one candidate: which query words its name, signature, and capped doc
/// cover. The name carries a match strength per word, the others a plain hit.
struct Hits {
    name: Vec<u8>,
    /// The query word is the whole name of the class the row belongs to, or its stem. Counted
    /// only when another word matches the row's own name, as in `Flask init`.
    owner: Vec<bool>,
    signature: Vec<bool>,
    doc: Vec<bool>,
    owner_context: Vec<bool>,
    path: Vec<bool>,
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
/// a name stem 2, the owning class `OWNER_CREDIT`, a signature or doc hit `TEXT_CREDIT`, a name
/// substring 1, then for an otherwise unmatched term weak owner context 0.5 or a path word
/// `PATH_CREDIT`, nothing 0.
fn term_credits(hits: &Hits, words: &[QueryWord]) -> Vec<(String, String, f64)> {
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let (field, credit) = match hits.name[i] {
                3 => ("name", 3.0),
                2 => ("name", 2.0),
                _ if hits.owner[i] => ("owner", OWNER_CREDIT),
                _ if hits.signature[i] => ("signature", TEXT_CREDIT),
                _ if hits.doc[i] => ("doc", TEXT_CREDIT),
                1 => ("name", 1.0),
                _ if hits.owner_context[i] => ("owner_context", OWNER_CONTEXT_CREDIT),
                _ if hits.path[i] => ("path", PATH_CREDIT),
                _ => ("none", 0.0),
            };
            (w.word.clone(), field.to_string(), credit)
        })
        .collect()
}

/// Adds a prior when one whole name token, the signature, and at least two distinct doc terms
/// independently support the query, which the row covers with at least half its distinct terms.
/// The caller limits it to functions and methods, so request-shaped types cannot earn it.
fn credited_field_bonus(terms: &[(String, String, f64)]) -> f64 {
    let query_terms: HashSet<&str> = terms.iter().map(|(word, _, _)| word.as_str()).collect();
    let covered_terms: HashSet<&str> = terms
        .iter()
        .filter(|(_, _, credit)| *credit >= TEXT_CREDIT)
        .map(|(word, _, _)| word.as_str())
        .collect();
    let adequate_query_coverage =
        !query_terms.is_empty() && covered_terms.len() * 2 >= query_terms.len();
    let mut document_terms = HashSet::new();
    let has_exact_name_term = terms
        .iter()
        .any(|(_, field, credit)| field == "name" && *credit == 3.0);
    let fields = terms.iter().filter(|(_, _, credit)| *credit > 0.0).fold(
        0u8,
        |fields, (word, field, _)| {
            if field == "doc" {
                // The query can repeat a word; count distinct words as independent evidence.
                document_terms.insert(word.as_str());
            }
            fields
                | match field.as_str() {
                    "name" => 1,
                    "owner" => 2,
                    "signature" => 4,
                    "doc" => 8,
                    "owner_context" => 16,
                    _ => 0,
                }
        },
    );
    if has_exact_name_term
        && fields & 4 != 0
        && fields & 8 != 0
        && document_terms.len() >= 2
        && adequate_query_coverage
    {
        FIELD_DIVERSITY_BONUS
    } else {
        0.0
    }
}

fn is_digit_word(word: &str) -> bool {
    word.chars().all(|c| c.is_ascii_digit())
}

/// The branches whose admission proves a query word other than a digit: the whole query as the
/// name, an FTS5 word match, a name substring, or the class a word names. Retrieval leaves the
/// digit words out of the branches that match any one word.
const WORD_PROOF_BRANCHES: &[&str] = &["exact", "word", "name", "owner"];

/// True when the row credits a query word other than a digit, or when a recall branch proved one,
/// as FTS5 does for a word past the doc head that the rerank reads. Only owner-context members,
/// which a digit (`= 1`) can admit, are left to fail it.
fn holds_a_query_word(explain: &SearchExplain) -> bool {
    explain
        .terms
        .iter()
        .any(|(word, field, credit)| *credit > 0.0 && field != "path" && !is_digit_word(word))
        || explain
            .branches
            .iter()
            .any(|branch| WORD_PROOF_BRANCHES.contains(&branch.as_str()))
}

/// Query words the row itself matches; a path word credits the file, not the row, so it does not count.
fn covered_query_term_count(terms: &[(String, String, f64)]) -> usize {
    terms
        .iter()
        .filter(|(_, field, credit)| *credit > 0.0 && field != "path")
        .map(|(word, _, _)| word.as_str())
        .collect::<HashSet<_>>()
        .len()
}

fn has_distinctive_name_term(explain: &SearchExplain) -> bool {
    let weights = &explain.word_weights;
    let Some(mean_weight) = (!weights.is_empty())
        .then(|| weights.iter().map(|(_, weight)| weight).sum::<f64>() / weights.len() as f64)
    else {
        return false;
    };
    explain
        .terms
        .iter()
        .zip(weights)
        .any(|((term, field, credit), (weighted_term, weight))| {
            term == weighted_term
                && field == "name"
                && *credit >= 2.0
                && *weight >= mean_weight * DISTINCTIVE_NAME_IDF_FACTOR
        })
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

fn is_path_segment_in(segment: &str, list: &[&str]) -> bool {
    list.iter().any(|entry| entry.eq_ignore_ascii_case(segment))
}

/// File stems that take the name of their folder in every language: `blog/mod.rs`, a Deno
/// `blog/mod.ts`, `blog/__init__.py`, `blog/__main__.py`, `blog/index.ts`.
const MODULE_INDEX_STEMS: &[&str] = &["mod", "__init__", "__main__", "index"];

/// Entry files that take the name of their folder in one language: `require("blog")` loads
/// `blog/init.lua`, a Swift target's `main.swift` belongs to the target folder, and a Scala
/// `package.scala` holds the package object of its folder.
const LANGUAGE_INDEX_FILES: &[(&str, &str)] = &[
    ("lua", "init.lua"),
    ("swift", "main.swift"),
    ("scala", "package.scala"),
];

/// Package roots: in a `src` folder they take the package name, elsewhere their folder
/// (`src/bin/tool/main.rs` is the `tool` binary).
const PACKAGE_ROOT_FILES: &[(&str, &str)] = &[
    ("rust", "lib.rs"),
    ("rust", "main.rs"),
    ("zig", "root.zig"),
    ("zig", "main.zig"),
];

/// The value of a TOML `name = "my-crate"` line.
fn toml_string_value(line: &str) -> Option<&str> {
    let (_, value) = line.split_once('=')?;
    let value = value.trim().trim_matches(['"', '\'']).trim();
    (!value.is_empty()).then_some(value)
}

/// The last segment of a dotted or `::` module name: `Post` for `MyApp.Blog.Post`.
fn last_module_segment(name: &str) -> &str {
    name.rsplit(['.', ':', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(name)
}

/// The names of the row's module, as code names it: the file name without its extensions, or
/// the folder of a module index file, or the package of a package root or a Go file (with the
/// Go file's own name), and the module the row is a direct member of. A test path and a path
/// through a vendored or fixture folder have none.
fn module_names(
    path: &str,
    language: &str,
    file_module: Option<&str>,
    module_owner: Option<&str>,
) -> Vec<String> {
    let segments: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    let Some((file, folders)) = segments.split_last() else {
        return Vec::new();
    };
    if is_test_path(path)
        || folders
            .iter()
            .any(|folder| is_path_segment_in(folder, UNCREDITED_PATH_SEGMENTS))
    {
        return Vec::new();
    }
    let stem = file.trim_start_matches('.').split('.').next().unwrap_or("");
    let folder = folders.last().copied();
    let entry_of = |files: &[(&str, &str)]| {
        files
            .iter()
            .any(|(lang, name)| *lang == language && name.eq_ignore_ascii_case(file))
    };
    let mut names: Vec<&str> = Vec::new();
    if language == "zig"
        && let Some(module) = file_module
    {
        names.push(module);
        if !entry_of(PACKAGE_ROOT_FILES) {
            names.push(stem);
        }
    } else if language == "swift"
        && let Some(target) = file_module
    {
        names.push(target);
        if !entry_of(LANGUAGE_INDEX_FILES) {
            names.push(stem);
        }
    } else if entry_of(PACKAGE_ROOT_FILES) {
        if folder.is_some_and(|f| f.eq_ignore_ascii_case("src")) {
            let package_folder = folders.len().checked_sub(2).map(|i| folders[i]);
            names.extend(file_module.and_then(toml_string_value).or(package_folder));
        } else {
            names.extend(folder);
        }
    } else if MODULE_INDEX_STEMS.contains(&stem) || entry_of(LANGUAGE_INDEX_FILES) {
        names.extend(folder);
    } else if language == "go" {
        match file_module {
            Some("main") => names.extend(folder),
            Some(package) => names.push(package),
            None => {}
        }
        names.push(stem);
    } else if language == "qmldir" {
        names.extend(file_module.map(last_module_segment));
    } else if language == "dart" {
        names.push(stem);
        names.extend(file_module.map(last_module_segment));
    } else {
        names.push(stem);
    }
    names.extend(module_owner.map(last_module_segment));
    let mut unique: Vec<String> = Vec::new();
    for name in names {
        if !name.is_empty() && !unique.iter().any(|seen| seen == name) {
            unique.push(name.to_string());
        }
    }
    unique
}

/// Per query word, whether it names one of the row's modules: a run of words whose letters
/// spell the whole name (`release corpus check` for `release-corpus-check.py`), or one word
/// with the name's stem (`view` for `views.py`).
fn path_hits(modules: &[String], words: &[QueryWord], stemmer: &Stemmer) -> Vec<bool> {
    let mut marks = vec![false; words.len()];
    for module in modules
        .iter()
        .map(|m| collapse(m))
        .filter(|m| !m.is_empty())
    {
        let module_stem = stemmer.stem(&module);
        for start in 0..words.len() {
            if words[start].stem == module_stem {
                marks[start] = true;
                continue;
            }
            let mut joined = String::new();
            for end in start..words.len() {
                joined.push_str(&words[end].word);
                if joined.len() >= module.len() {
                    if joined == module {
                        marks[start..=end].fill(true);
                    }
                    break;
                }
            }
        }
    }
    marks
}

/// Demotes a row under a role folder such as `scripts` or `examples` unless the query names that
/// folder or the row's module, so `blog` keeps `examples/tutorial/flaskr/blog.py`.
fn path_role(path: &str, words: &[QueryWord], stemmer: &Stemmer, names_module: bool) -> f64 {
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
    if named || names_module {
        0.0
    } else {
        W_PATH_ROLE
    }
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
    let words = stemmed_query_words(query, &stemmer);
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
            let name = name_hits(&symbol.name, &words, &stemmer);
            let named = name.iter().any(|strength| *strength > 0);
            Hits {
                owner: match candidate.owner.as_deref().map(collapse) {
                    Some(owner) if named => {
                        let owner_stem = stemmer.stem(&owner);
                        words
                            .iter()
                            .map(|w| owner == w.word || owner_stem == w.stem)
                            .collect()
                    }
                    _ => vec![false; words.len()],
                },
                name,
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
                        .map(|doc| head_bytes(doc, DOC_HEAD_BYTES)),
                    &words,
                    &mut tokens,
                ),
                owner_context: text_hits(candidate.owner_context.as_deref(), &words, &mut tokens),
                path: path_hits(
                    &module_names(
                        &symbol.path,
                        &symbol.language,
                        candidate.file_module.as_deref(),
                        candidate.module_owner.as_deref(),
                    ),
                    &words,
                    &stemmer,
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
            let names_module = hits.path.contains(&true);
            let terms = term_credits(&hits, &words);
            let field_diversity_bonus = if matches!(symbol.kind.as_str(), "function" | "method") {
                credited_field_bonus(&terms)
            } else {
                0.0
            };
            let explain = SearchExplain {
                bm25: candidate.bm25,
                branches: [
                    (candidate.exact_name, "exact"),
                    (candidate.word_match, "word"),
                    (candidate.name_match, "name"),
                    (candidate.owner_match, "owner"),
                    (candidate.owner_context.is_some(), "owner_context"),
                ]
                .into_iter()
                .filter(|(hit, _)| *hit)
                .map(|(_, branch)| branch.to_string())
                .collect(),
                name_tier: tier.to_string(),
                name_strength,
                term_score: term_score(&terms, &term_weights),
                field_diversity_bonus,
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
                path_role: path_role(&symbol.path, &words, &stemmer, names_module),
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
                nested: if candidate.nested { W_NESTED } else { 0.0 },
                terms,
                word_weights: word_weights.clone(),
                candidates: 0,
                rerank_us: 0,
            };
            let score = explain.term_score
                + explain.field_diversity_bonus
                + explain.name_bonus
                + explain.kind_prior
                + explain.path_role
                + explain.documentation
                + explain.test_intent
                + explain.nested;
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

const LOW_SIGNAL_KINDS_SQL: &str =
    "'import','variable','parameter','field','property','module','namespace'";

/// Find tests related to a target symbol by caller relationships, naming pattern, or FTS matching.
pub fn find_related_tests(
    conn: &Connection,
    target_symbol: &Symbol,
    limit: usize,
) -> Result<Vec<Symbol>, QueryError> {
    if limit == 0 || DOCUMENTATION_LANGUAGES.contains(&target_symbol.language.as_str()) {
        return Ok(Vec::new());
    }

    const COLUMNS: &str = "s.symbol_id, s.file_id, s.path, s.language, s.name, s.kind, s.signature, s.doc_comment,
            s.visibility, s.parent_symbol_id, s.start_line, s.start_column, s.end_line, s.end_column,
            s.start_byte, s.end_byte, s.body_start_line, s.body_start_column, s.body_end_line,
            s.body_end_column, s.body_start_byte, s.body_end_byte, s.body_hash, s.semantic_group,
            s.is_test, s.test_container";
    // Rows a scan writes can land in any order, so every query below needs a full ORDER BY.
    const TEST_ORDER: &str = "s.is_test DESC, s.path, s.start_line, s.symbol_id";
    let not_setup = |alias: &str| {
        if has_column(conn, "symbols", "test_lifecycle") {
            format!("COALESCE({alias}.test_lifecycle, 0) = 0")
        } else {
            "1 = 1".to_string()
        }
    };
    let is_test = format!(
        "((s.is_test = 1 OR s.test_container = 1
           OR ({} AND s.kind IN ('function', 'method') AND ({}
               OR EXISTS (SELECT 1 FROM symbols tc
                          WHERE tc.symbol_id = s.parent_symbol_id
                            AND (tc.test_container = 1
                                 OR (tc.kind IN ('function', 'method') AND {}
                                     AND (tc.is_test = 1 OR {})))))))
          AND {})",
        test_path_predicate("s"),
        names_a_test_sql("s"),
        not_setup("tc"),
        names_a_test_sql("tc"),
        not_setup("s")
    );
    let not_documentation = not_documentation(conn, "s");

    let mut tests = Vec::new();
    let mut seen_ids = std::collections::HashSet::from([target_symbol.symbol_id.clone()]);
    let direct_call_language = call_languages_compatible_sql("s.language", "?3");

    let callers_sql = format!(
        "SELECT {COLUMNS}
     FROM symbols s
     JOIN relationships r ON r.from_symbol_id = s.symbol_id
     WHERE r.to_symbol_id = ?1
       AND (r.kind != 'calls' OR {direct_call_language})
       AND {is_test} AND {not_documentation}
     ORDER BY {TEST_ORDER}
     LIMIT ?2"
    );

    if let Ok(mut stmt) = conn.prepare(&callers_sql)
        && let Ok(rows) = stmt.query_map(
            params![
                target_symbol.symbol_id,
                limit as i64,
                target_symbol.language.as_str()
            ],
            map_symbol,
        )
    {
        for row in rows.flatten() {
            let row = enclosing_test(conn, row)?;
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
        let pending_language = pending_relationship_language_filter("p.kind", "s_from", "s_target");
        let pending_receiver_chain =
            if pending_receiver_chain_requires_full_for_target_name(conn, &target_symbol.name) {
                pending_target_predicate(conn, "s_target", "s_target_parent")
            } else {
                pending_target_predicate_without_typed_receiver_chain(
                    conn,
                    "s_target",
                    "s_target_parent",
                )
            };
        let pending_sql = format!(
            "SELECT DISTINCT {COLUMNS}
     FROM pending_relationships p
     JOIN symbols s ON p.from_symbol_id = s.symbol_id
     JOIN symbols s_from ON s_from.symbol_id = s.symbol_id
     JOIN symbols s_target ON s_target.symbol_id = ?1
     LEFT JOIN symbols s_target_parent ON s_target.parent_symbol_id = s_target_parent.symbol_id
     WHERE p.target_terminal_name = s_target.name
       AND {pending_language}
       AND {is_test}
       AND {not_documentation}
       AND {pred}
     ORDER BY {TEST_ORDER}
     LIMIT ?2",
            pred = pending_receiver_chain
        );

        if let Ok(mut stmt) = conn.prepare(&pending_sql)
            && let Ok(rows) = stmt.query_map(
                params![target_symbol.symbol_id, remaining as i64],
                map_symbol,
            )
        {
            for row in rows.flatten() {
                let row = enclosing_test(conn, row)?;
                if seen_ids.insert(row.symbol_id.clone()) {
                    tests.push(row);
                    if tests.len() >= limit {
                        return Ok(tests);
                    }
                }
            }
        }
    }

    // A test that reads or assigns the symbol (`app.wsgi_app = ...`) has no call edge.
    let reported_call_language = call_languages_compatible_sql("s.language", "s_target.language");
    let reported_name_language = identifier_language_matches("s", "s_target");
    let mut test_by_id = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM symbols s
         JOIN symbols s_target ON s_target.symbol_id = ?4
         WHERE s.symbol_id = ?1
           AND {is_test}
           AND {not_documentation}
           AND ((?2 = 0 AND ?3 = 0)
                OR (?2 = 1 AND {reported_call_language})
                OR (?3 = 1 AND {reported_name_language}))"
    ))?;
    for site in find_references_for_symbol(
        conn,
        &target_symbol.name,
        "callers",
        MAX_RESULT_LIMIT,
        &target_symbol.symbol_id,
    )
    .unwrap_or_default()
    {
        if site.kind == "member_access (candidate)" || seen_ids.contains(&site.from_symbol_id) {
            continue;
        }
        let explicit_noncall = site.kind != "calls"
            && conn.query_row(
                "SELECT EXISTS (
                         SELECT 1 FROM relationships
                         WHERE from_symbol_id = ?1 AND to_symbol_id = ?2 AND kind = ?3
                     )",
                params![site.from_symbol_id, target_symbol.symbol_id, site.kind],
                |row| row.get::<_, bool>(0),
            )?;
        let is_name_only = site.kind != "calls" && !explicit_noncall;
        if let Some(test) = test_by_id
            .query_row(
                params![
                    site.from_symbol_id,
                    site.kind == "calls",
                    is_name_only,
                    target_symbol.symbol_id
                ],
                map_symbol,
            )
            .optional()?
        {
            let test = enclosing_test(conn, test)?;
            if seen_ids.insert(test.symbol_id.clone()) {
                tests.push(test);
                if tests.len() >= limit {
                    return Ok(tests);
                }
            }
        }
    }

    // `Flask(...)` runs `Flask.__init__`, so the tests that build the class test its constructor.
    if (target_symbol.kind == "constructor" || target_symbol.name == "__init__")
        && tests.len() < limit
        && let Some(class) = target_symbol
            .parent_symbol_id
            .as_deref()
            .map(|id| get_symbol_by_id(conn, id))
            .transpose()?
            .flatten()
            .filter(|class| class.kind == "class" || class.kind == "struct")
    {
        for test in find_related_tests(conn, &class, limit - tests.len())? {
            if seen_ids.insert(test.symbol_id.clone()) {
                tests.push(test);
            }
        }
        return Ok(tests);
    }

    let name = &target_symbol.name;
    let dunder = name.len() > 4 && name.starts_with("__") && name.ends_with("__");
    let definitions: i64 = conn.query_row(
        "SELECT COUNT(*) FROM symbols WHERE name = ?1 AND kind NOT IN ('import', 'export')",
        params![name],
        |row| row.get(0),
    )?;
    if dunder || definitions > 1 {
        return Ok(tests);
    }

    let remaining = limit - tests.len();
    // SQLite tests these terms in the order written, and the test-path rule costs about 16 times
    // the name match on every row, so the name match comes first.
    let name_sql = format!(
        "SELECT {COLUMNS}
     FROM symbols s
     WHERE (s.name LIKE ?1 ESCAPE '\\' OR s.signature LIKE ?1 ESCAPE '\\')
       AND {is_test}
       AND {not_documentation}
     ORDER BY (s.name LIKE ?1 ESCAPE '\\') DESC, {TEST_ORDER}
     LIMIT ?2"
    );

    if let Ok(mut stmt) = conn.prepare(&name_sql)
        && let Ok(rows) = stmt.query_map(
            params![format!("%{}%", escape_like(name)), (remaining * 2) as i64],
            map_symbol,
        )
    {
        for row in rows.flatten() {
            let row = enclosing_test(conn, row)?;
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
         WHERE symbols_fts MATCH ?1 AND {is_test} AND {not_documentation}
         ORDER BY {TEST_ORDER}
         LIMIT ?2"
        );

        let and_q = name_prefix_query(&target_symbol.name);
        if !and_q.is_empty()
            && let Ok(mut stmt) = conn.prepare(&fts_sql)
            && let Ok(rows) = stmt.query_map(params![and_q, (remaining * 2) as i64], map_symbol)
        {
            for row in rows.flatten() {
                let row = enclosing_test(conn, row)?;
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

/// Renames each row whose parent is a class or another code type to `Owner.name`, so a list of
/// `__init__` rows says which class each one belongs to. For text output only: other tools take
/// the qualified name, and JSON keeps the raw name.
pub fn qualify_members<'a>(
    conn: &Connection,
    symbols: impl IntoIterator<Item = &'a mut Symbol>,
) -> Result<(), QueryError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT name FROM symbols
         WHERE symbol_id = ?1
           AND kind IN ('class', 'struct', 'interface', 'trait', 'enum', 'record', 'object', 'protocol', 'union')
           AND language NOT IN ({})",
        documentation_language_list()
    ))?;
    for symbol in symbols {
        let Some(parent) = symbol.parent_symbol_id.as_deref() else {
            continue;
        };
        if let Some(owner) = stmt
            .query_row(params![parent], |row| row.get::<_, String>(0))
            .optional()?
            && !symbol.name.starts_with(&format!("{owner}."))
        {
            symbol.name = format!("{owner}.{}", symbol.name);
        }
    }
    Ok(())
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

/// Retrieve one indexed symbol by its exact current-index identifier.
pub fn get_symbol_by_id(conn: &Connection, symbol_id: &str) -> Result<Option<Symbol>, QueryError> {
    conn.query_row(
        "SELECT symbol_id, file_id, path, language, name, kind, signature, doc_comment,
                visibility, parent_symbol_id, start_line, start_column, end_line, end_column,
                start_byte, end_byte, body_start_line, body_start_column, body_end_line,
                body_end_column, body_start_byte, body_end_byte, body_hash, semantic_group,
                is_test, test_container
         FROM symbols WHERE symbol_id = ?1",
        [symbol_id],
        map_symbol,
    )
    .optional()
    .map_err(QueryError::from)
}

/// The signature of a callee row. A CSS rule's signature is its whole text, so a CSS callee shows
/// the head before its `{` from a bounded prefix.
const CALLEE_SIGNATURE_SQL: &str = "CASE WHEN s_to.language = 'css'
    THEN rtrim(substr(s_to.signature, 1, instr(substr(s_to.signature, 1, 1024) || '{', '{') - 1))
    ELSE s_to.signature END";

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

fn name_candidates(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
    exact_path: bool,
) -> Result<Vec<Symbol>, QueryError> {
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
        return Ok(Vec::new());
    }

    if matches.len() == 1 {
        return Ok(vec![matches.remove(0)]);
    }

    // Exclude imports if non-import candidates exist
    let candidates: Vec<Symbol> = if matches.iter().any(|s| s.kind != "import") {
        matches.into_iter().filter(|s| s.kind != "import").collect()
    } else {
        matches
    };
    let writes = inherited_writes_among(conn, candidates.iter().map(|s| s.symbol_id.as_str()))?;
    let candidates: Vec<Symbol> = if candidates.iter().any(|s| !writes.contains(&s.symbol_id)) {
        candidates
            .into_iter()
            .filter(|s| !writes.contains(&s.symbol_id))
            .collect()
    } else {
        candidates
    };

    if candidates.len() == 1 {
        return Ok(vec![candidates.into_iter().next().unwrap()]);
    }

    // Check if there's an exact match on full name among candidates
    let exact_name_matches: Vec<_> = candidates
        .iter()
        .filter(|s| s.name == name)
        .cloned()
        .collect();
    if exact_name_matches.len() == 1 {
        return Ok(vec![exact_name_matches.into_iter().next().unwrap()]);
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
        return Ok(vec![def_matches.into_iter().next().unwrap()]);
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
            return Ok(vec![exact_path_matches.into_iter().next().unwrap()]);
        }
    }

    if active_pool.len() == 1 {
        return Ok(vec![active_pool.into_iter().next().unwrap()]);
    }

    let mut outside_tests = active_pool.iter().filter(|s| !is_test_path(&s.path));
    if let (Some(only), None) = (outside_tests.next(), outside_tests.next()) {
        return Ok(vec![only.clone()]);
    }

    let mut outer = outer_definitions(conn, &active_pool)?;
    if outer.len() == 1 {
        return Ok(vec![outer.remove(0)]);
    }

    Ok(active_pool)
}

fn get_symbol_by_name_internal(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
    exact_path: bool,
) -> Result<Option<Symbol>, QueryError> {
    let mut candidates = name_candidates(conn, name, path_filter, exact_path)?;
    match candidates.len() {
        0 | 1 => Ok(candidates.pop()),
        _ => Err(ambiguous_symbol(name, &candidates)),
    }
}

fn ambiguous_symbol(name: &str, candidates: &[Symbol]) -> QueryError {
    QueryError::AmbiguousSymbol(
        name.to_string(),
        candidates.len(),
        candidate_id_lines(candidates),
    )
}

/// One `- kind `name` in path:line (id=…)` line for each candidate.
pub(crate) fn candidate_id_lines(candidates: &[Symbol]) -> String {
    let mut lines = String::new();
    for s in candidates {
        lines.push_str(&format!(
            "- {} `{}` in {}:{} (id={})\n",
            crate::formatters::display_kind(s),
            s.name,
            s.path,
            s.start_line,
            s.symbol_id
        ));
    }
    lines
}

/// The overloads a name selects: one symbol, or every symbol with that name, owner, and file.
/// Any other set of candidates is an `AmbiguousSymbol` error.
pub fn get_symbol_overloads(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> Result<Vec<Symbol>, QueryError> {
    let candidates = name_candidates(conn, name, path_filter, false)?;
    let first = candidates.first();
    let overloads = candidates.iter().all(|s| {
        first.is_some_and(|first| {
            s.name == first.name
                && s.path == first.path
                && s.parent_symbol_id == first.parent_symbol_id
        })
    });
    if overloads {
        Ok(candidates)
    } else {
        Err(ambiguous_symbol(name, &candidates))
    }
}

/// The candidates left after dropping an `export` row that repeats a definition on its line and,
/// when anything else remains, the locals of a function or method.
fn outer_definitions(conn: &Connection, pool: &[Symbol]) -> Result<Vec<Symbol>, QueryError> {
    let definitions: Vec<&Symbol> = pool
        .iter()
        .filter(|s| {
            s.kind != "export"
                || !pool.iter().any(|other| {
                    other.kind != "export"
                        && other.path == s.path
                        && other.start_line == s.start_line
                })
        })
        .collect();
    let mut parent_kind = conn.prepare("SELECT kind FROM symbols WHERE symbol_id = ?1")?;
    let mut outer = Vec::new();
    for symbol in &definitions {
        let kind = match &symbol.parent_symbol_id {
            Some(parent) => parent_kind
                .query_row([parent], |row| row.get::<_, String>(0))
                .optional()?,
            None => None,
        };
        if !matches!(kind.as_deref(), Some("function" | "method" | "constructor")) {
            outer.push((*symbol).clone());
        }
    }
    Ok(if outer.is_empty() {
        definitions.into_iter().cloned().collect()
    } else {
        outer
    })
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
    Ok(scored.into_iter().map(|(_, symbol)| symbol).collect())
}

/// Up to three indexed symbols whose names are close to `name`, definitions before imports.
/// Never fails.
pub fn suggest_symbol_names(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> Vec<Symbol> {
    let mut found = symbol_name_candidates(conn, name, path_filter);
    found.truncate(SUGGESTED_NAMES);
    found
}

const SUGGESTED_NAMES: usize = 3;
const SUGGESTION_POOL: usize = 20;

/// The close names of `name`, best first: definitions before imports, and no import of a name
/// that one of them defines, so the imports of a widely used name never hide its definition.
fn symbol_name_candidates(conn: &Connection, name: &str, path_filter: Option<&str>) -> Vec<Symbol> {
    let (ancestors, terminal) = split_qualified_name(name);
    let mut found =
        search_symbols_scoped(conn, terminal, None, path_filter, false, SUGGESTION_POOL)
            .unwrap_or_default();
    if found.is_empty() && terminal.chars().count() >= 4 {
        found = near_names_by_trigram(conn, terminal, path_filter).unwrap_or_default();
    }
    let defined: HashSet<String> = found
        .iter()
        .filter(|symbol| symbol.kind != "import")
        .map(|symbol| symbol.name.clone())
        .collect();
    found.retain(|symbol| symbol.kind != "import" || !defined.contains(&symbol.name));
    found.sort_by_key(|symbol| symbol.kind == "import");
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

    let max_edits = (basename.chars().count() / 4).clamp(1, 2);
    let mut near: Vec<(usize, String)> = Vec::new();
    let _ = conn
        .prepare(&format!("SELECT {path_expr} FROM files"))
        .and_then(|mut stmt| {
            for path in stmt.query_map([], |row| row.get::<_, String>(0))?.flatten() {
                let name = path.rsplit('/').next().unwrap_or(&path).to_lowercase();
                let edits = edit_distance(&name, &basename);
                if edits <= max_edits {
                    near.push((edits, path));
                }
            }
            Ok(())
        });
    near.sort();
    near.into_iter().take(3).map(|(_, path)| path).collect()
}

/// The workspace name and the recovery hint for a symbol that is not indexed.
pub fn symbol_not_found_parts(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> (String, String) {
    let hint = name_suggestions(conn, name, path_filter, &|_| true)
        .hint()
        .unwrap_or_else(|| NO_SIMILAR_NAME.to_string());
    (workspace_name(conn), hint)
}

/// The hint for a name with no definition and no close name.
pub const NO_SIMILAR_NAME: &str = "No similar name is indexed; check the workspace and spelling.";

/// The names to offer for a name that no row has.
#[derive(Debug, Clone, Default)]
pub struct NameSuggestions {
    /// The path filter that excludes every row of `symbols`, which then all have the name.
    pub excluded_by: Option<String>,
    /// Up to three close names, or the definitions of the name outside `excluded_by`.
    pub symbols: Vec<Symbol>,
}

impl NameSuggestions {
    /// The `Did you mean one of:` text, or the definitions elsewhere; `None` with no symbol.
    pub fn hint(&self) -> Option<String> {
        if self.symbols.is_empty() {
            return None;
        }
        let list = self
            .symbols
            .iter()
            .map(|s| format!("  - {} `{}` ({}:{})", s.kind, s.name, s.path, s.start_line))
            .collect::<Vec<_>>()
            .join("\n");
        Some(match &self.excluded_by {
            Some(path) => format!("`{path}` does not define it; it is defined in:\n{list}"),
            None => format!("Did you mean one of:\n{list}"),
        })
    }
}

/// The definitions of `name` that `path_filter` excludes, or else the close names that
/// [`suggest_symbol_names`] finds; only symbols that `keep` accepts, at most three.
pub fn name_suggestions(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
    keep: &dyn Fn(&Symbol) -> bool,
) -> NameSuggestions {
    let terminal = split_qualified_name(name).1;
    let pick = |path_filter: Option<&str>, accept: &dyn Fn(&Symbol) -> bool| {
        let mut found: Vec<Symbol> = symbol_name_candidates(conn, name, path_filter)
            .into_iter()
            .filter(|symbol| keep(symbol) && accept(symbol))
            .collect();
        found.truncate(SUGGESTED_NAMES);
        found
    };
    if let Some(path) = path_filter {
        let elsewhere = pick(None, &|symbol| symbol.name == terminal);
        if !elsewhere.is_empty() {
            return NameSuggestions {
                excluded_by: Some(path.to_string()),
                symbols: elsewhere,
            };
        }
    }
    NameSuggestions {
        excluded_by: None,
        symbols: pick(path_filter, &|_| true),
    }
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
    find_references_for_symbol_ext(conn, symbol_name, direction, limit, symbol_id, false)
}

pub fn find_references_for_symbol_ext(
    conn: &Connection,
    symbol_name: &str,
    direction: &str,
    limit: usize,
    symbol_id: &str,
    include_external: bool,
) -> Result<Vec<ReferenceSite>, QueryError> {
    find_references_internal(
        conn,
        symbol_name,
        direction,
        limit,
        Some(symbol_id),
        include_external,
    )
}

/// Find one bounded, source-ordered page of a symbol's callees and the complete matching count.
pub fn find_callees_page_scoped(
    conn: &Connection,
    symbol_name: &str,
    limit: usize,
    offset: usize,
    include_external: bool,
    path_filter: Option<&str>,
) -> Result<(Vec<ReferenceSite>, usize), QueryError> {
    validate_result_page(offset, limit)?;
    match get_symbol_by_name(conn, symbol_name, path_filter)? {
        Some(target) => find_callees_page_internal(
            conn,
            &target.name,
            Some(&target.symbol_id),
            limit,
            offset,
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

/// Find one bounded, source-ordered page of a selected symbol's callees and the complete count.
pub fn find_callees_page_for_symbol(
    conn: &Connection,
    symbol_name: &str,
    symbol_id: &str,
    limit: usize,
    offset: usize,
    include_external: bool,
) -> Result<(Vec<ReferenceSite>, usize), QueryError> {
    validate_result_page(offset, limit)?;
    find_callees_page_internal(
        conn,
        symbol_name,
        Some(symbol_id),
        limit,
        offset,
        include_external,
    )
}

fn find_callees_page_internal(
    conn: &Connection,
    symbol_name: &str,
    symbol_id: Option<&str>,
    limit: usize,
    offset: usize,
    include_external: bool,
) -> Result<(Vec<ReferenceSite>, usize), QueryError> {
    let (cte, reads_owners) = callee_page_cte(conn, include_external, symbol_id);
    let owners = if reads_owners {
        receiver_chain_owner_map(
            conn,
            "",
            "",
            "s_from.name = ?1 AND (?2 IS NULL OR p.from_symbol_id = ?2)",
            params![symbol_name, symbol_id],
        )?
    } else {
        "{}".to_string()
    };
    let count_sql = format!("{cte} SELECT COUNT(*) FROM merged_sites");
    let total: i64 = if reads_owners {
        conn.query_row(&count_sql, params![symbol_name, symbol_id, owners], |row| {
            row.get(0)
        })?
    } else {
        conn.query_row(&count_sql, params![symbol_name, symbol_id], |row| {
            row.get(0)
        })?
    };
    let total = total as usize;
    if limit == 0 || offset > i64::MAX as usize {
        return Ok((Vec::new(), total));
    }

    let mut stmt = conn.prepare(&format!(
        "{cte}
         SELECT from_symbol_name, from_symbol_id, to_symbol_name, kind, path,
                start_line, start_column
         FROM merged_sites
         ORDER BY path, start_line, start_column, to_symbol_name, kind, from_symbol_id
         LIMIT ?4 OFFSET ?5"
    ))?;
    let rows = stmt.query_map(
        params![symbol_name, symbol_id, owners, limit as i64, offset as i64],
        |row| {
            Ok(ReferenceSite {
                from_symbol_name: row.get(0)?,
                from_symbol_id: row.get(1)?,
                to_symbol_name: row.get(2)?,
                kind: row.get(3)?,
                path: row.get::<_, String>(4)?.replace('\\', "/"),
                start_line: row.get::<_, Option<i64>>(5)?.map(|line| line as usize),
                start_column: row.get::<_, Option<i64>>(6)?.map(|column| column as usize),
                occurrences: None,
                target: None,
                enclosing_symbol_name: None,
            })
        },
    )?;
    let mut sites = rows.collect::<Result<Vec<_>, _>>()?;
    describe_callee_targets(conn, &mut sites)?;
    Ok((sites, total))
}

/// The callee sites CTE, and whether it reads the receiver-chain owner map bound as `?3`.
fn callee_page_cte(
    conn: &Connection,
    include_external: bool,
    symbol_id: Option<&str>,
) -> (String, bool) {
    let pending_language = pending_relationship_language_filter("p.kind", "s_from", "s_to");
    let mut reads_owners = false;
    let pending_predicate = if include_external || !has_pending_namespace_column(conn) {
        String::new()
    } else {
        let pending_receiver_chain = symbol_id.is_none_or(|source_id| {
            pending_receiver_chain_requires_full_for_source(conn, source_id)
        });
        if pending_receiver_chain {
            reads_owners = has_typed_receiver_chain_support(conn);
            pending_target_predicate_with_receiver_chain_owner(
                conn,
                "s_to",
                "s_to_parent",
                &bound_receiver_chain_owner_sql("?3"),
            )
        } else {
            pending_target_predicate_without_typed_receiver_chain(conn, "s_to", "s_to_parent")
        }
    };
    let pending = if has_pending_namespace_column(conn) {
        if include_external {
            "SELECT s_from.name AS from_symbol_name,
                    p.from_symbol_id AS from_symbol_id,
                    COALESCE(NULLIF(p.target_display_name, ''), p.target_terminal_name) AS to_symbol_name,
                    p.kind AS kind,
                    p.path AS path,
                    p.start_line AS start_line,
                    p.start_column AS start_column
             FROM pending_relationships p
             JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
             WHERE s_from.name = ?1 AND (?2 IS NULL OR p.from_symbol_id = ?2)"
                .to_string()
        } else {
            format!(
                "SELECT s_from.name AS from_symbol_name,
                        p.from_symbol_id AS from_symbol_id,
                        p.target_terminal_name AS to_symbol_name,
                        p.kind AS kind,
                        p.path AS path,
                        p.start_line AS start_line,
                        p.start_column AS start_column
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 WHERE s_from.name = ?1 AND (?2 IS NULL OR p.from_symbol_id = ?2)
                   AND EXISTS (
                       SELECT 1 FROM symbols s_to
                       LEFT JOIN symbols s_to_parent ON s_to.parent_symbol_id = s_to_parent.symbol_id
                       WHERE s_to.name = p.target_terminal_name
                         AND {}
                         AND {pending_language}
                         AND {}
                   )",
                callee_target_kind("s_to_parent"),
                pending_predicate,
            )
        }
    } else if include_external {
        "SELECT s_from.name AS from_symbol_name,
                p.from_symbol_id AS from_symbol_id,
                p.target_terminal_name AS to_symbol_name,
                p.kind AS kind,
                p.path AS path,
                p.start_line AS start_line,
                p.start_column AS start_column
         FROM pending_relationships p
         JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
         WHERE s_from.name = ?1 AND (?2 IS NULL OR p.from_symbol_id = ?2)"
            .to_string()
    } else {
        format!(
            "SELECT s_from.name AS from_symbol_name,
                p.from_symbol_id AS from_symbol_id,
                p.target_terminal_name AS to_symbol_name,
                p.kind AS kind,
                p.path AS path,
                p.start_line AS start_line,
                p.start_column AS start_column
         FROM pending_relationships p
         JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
         WHERE s_from.name = ?1 AND (?2 IS NULL OR p.from_symbol_id = ?2)
           AND EXISTS (SELECT 1 FROM symbols s_to WHERE s_to.name = p.target_terminal_name
                       AND {pending_language})"
        )
    };
    let relationship_call_language =
        call_languages_compatible_sql("s_from.language", "s_to.language");
    let cte = format!(
        "WITH raw_sites AS (
             SELECT s_from.name AS from_symbol_name,
                    r.from_symbol_id AS from_symbol_id,
                    s_to.name AS to_symbol_name,
                    r.kind AS kind,
                    r.path AS path,
                    r.start_line AS start_line,
                    r.start_column AS start_column
             FROM relationships r
             JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
             JOIN symbols s_to ON r.to_symbol_id = s_to.symbol_id
             WHERE s_from.name = ?1 AND (?2 IS NULL OR r.from_symbol_id = ?2)
               AND (r.kind != 'calls' OR {relationship_call_language})
             UNION ALL
             {pending}
         ), merged_sites AS (
             SELECT from_symbol_name, from_symbol_id, to_symbol_name, kind, path,
                    start_line, MIN(start_column) AS start_column
             FROM raw_sites
             GROUP BY from_symbol_name, from_symbol_id, to_symbol_name, kind, path, start_line
         )"
    );
    (cte, reads_owners)
}

/// SQL expression ranking a candidate path against the call site `p.path`:
/// 2 for the same file, 1 for the same directory, 0 otherwise.
fn call_site_proximity(candidate_path: &str) -> String {
    path_proximity(candidate_path, "p.path")
}

/// SQL expression ranking `candidate_path` against `site_path`: 2 for the same file, 1 for the
/// same directory, 0 otherwise.
pub(crate) fn path_proximity(candidate_path: &str, site_path: &str) -> String {
    let normalized = format!("replace({candidate_path}, '\\', '/')");
    let call_site = format!("replace({site_path}, '\\', '/')");
    format!(
        "CASE WHEN {normalized} = {call_site} THEN 2
              WHEN rtrim({normalized}, replace({normalized}, '/', '')) = rtrim({call_site}, replace({call_site}, '/', '')) THEN 1
              ELSE 0 END"
    )
}

/// True when the import source `value` is a relative file path (`../core/util.js`).
fn is_relative_import_path(value: &str) -> String {
    format!("({value} LIKE '.%' AND instr({value}, '/') > 0)")
}

/// True when `{target}` is the file the relative import source `value` names, resolved from the
/// folder of the call site `p.path`: `../core/util.js` from `src/v4/locales/ru.ts` names
/// `src/v4/core/util.ts`, `.js`, `.tsx`, and the other script files, or that folder's `index`.
fn relative_import_matches(value: &str, target: &str) -> String {
    let stem = format!(
        "CASE WHEN {value} GLOB '*.[jt]s' THEN substr({value}, 1, length({value}) - 3)
              WHEN {value} GLOB '*.[mc][jt]s' OR {value} GLOB '*.[jt]sx'
                   THEN substr({value}, 1, length({value}) - 4)
              ELSE {value} END"
    );
    let caller = "replace(p.path, '\\', '/')";
    let parent_dir = "rtrim(substr(hop.dir, 1, length(hop.dir) - 1), replace(substr(hop.dir, 1, length(hop.dir) - 1), '/', ''))";
    let files = ["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "d.ts"]
        .iter()
        .flat_map(|ext| {
            [
                format!("hop.dir || hop.rest || '.{ext}'"),
                format!("hop.dir || hop.rest || '/index.{ext}'"),
            ]
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "EXISTS (
            WITH RECURSIVE hop(dir, rest) AS (
                SELECT rtrim({caller}, replace({caller}, '/', '')),
                       CASE WHEN ({stem}) LIKE './%' THEN substr(({stem}), 3) ELSE ({stem}) END
                UNION ALL
                SELECT {parent_dir}, substr(hop.rest, 4) FROM hop WHERE hop.rest LIKE '../%'
            )
            SELECT 1 FROM hop
            WHERE hop.rest NOT LIKE '../%' AND hop.rest != ''
              AND replace({target}.path, '\\', '/') IN ({files})
        )"
    )
}

/// Languages whose files import each other's names, so a name-only reference may cross them.
pub(crate) const JS_FAMILY_LANGUAGES: [&str; 5] = ["javascript", "typescript", "tsx", "jsx", "vue"];
pub(crate) const JS_FAMILY_LANGUAGES_SQL: &str = js_family_sql!();

pub(crate) fn is_js_family(language: &str) -> bool {
    JS_FAMILY_LANGUAGES.contains(&language)
}

const JVM_CALL_LANGUAGES: &[&str] = &["java", "kotlin", "scala", "groovy"];
const C_CALL_LANGUAGES: &[&str] = &["c", "cpp", "objc", "objcpp", "qml"];
const DOTNET_CALL_LANGUAGES: &[&str] = &["csharp", "razor", "vbnet", "fsharp"];
const CALL_LANGUAGE_FAMILIES: &[(&str, &[&str])] = &[
    ("js", &JS_FAMILY_LANGUAGES),
    ("jvm", JVM_CALL_LANGUAGES),
    ("c", C_CALL_LANGUAGES),
    ("dotnet", DOTNET_CALL_LANGUAGES),
];

pub(crate) fn call_language_family(language: &str) -> &str {
    CALL_LANGUAGE_FAMILIES
        .iter()
        .find(|(_, languages)| languages.contains(&language))
        .map(|(family, _)| *family)
        .unwrap_or(language)
}

pub(crate) fn call_languages_compatible(row: &str, target: &str) -> bool {
    row.is_empty() || call_language_family(row) == call_language_family(target)
}

fn call_language_family_sql(language: &str) -> String {
    let cases = CALL_LANGUAGE_FAMILIES
        .iter()
        .map(|(family, languages)| {
            let values = languages
                .iter()
                .map(|language| format!("'{language}'"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("WHEN {language} IN ({values}) THEN '{family}'")
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("CASE {cases} ELSE {language} END")
}

fn call_languages_compatible_sql(row: &str, target: &str) -> String {
    let row = format!("COALESCE({row}, '')");
    let target = format!("COALESCE({target}, '')");
    format!(
        "({row} = '' OR {} = {})",
        call_language_family_sql(&row),
        call_language_family_sql(&target)
    )
}

/// SQL that holds when a name-only identifier in `identifier` may refer to `target` across
/// languages: the same language, the JavaScript family, Razor and C#, or QML to C++.
fn identifier_language_matches(identifier: &str, target: &str) -> String {
    format!(
        "({target}.language = {identifier}.language
          OR ({identifier}.language IN ({JS_FAMILY_LANGUAGES_SQL})
              AND {target}.language IN ({JS_FAMILY_LANGUAGES_SQL}))
          OR ({identifier}.language IN ('razor', 'csharp')
              AND {target}.language IN ('razor', 'csharp'))
          OR ({identifier}.language = 'qml' AND {target}.language = 'cpp'))"
    )
}

fn pending_relationship_language_filter(kind: &str, source: &str, target: &str) -> String {
    format!(
        "(({kind} = 'calls' AND {call_language})
          OR ({kind} != 'calls' AND {name_language}))",
        call_language = call_languages_compatible_sql(
            &format!("{source}.language"),
            &format!("{target}.language")
        ),
        name_language = identifier_language_matches(source, target)
    )
}

const NESTED_TYPE_KINDS: &str =
    "'class', 'struct', 'interface', 'enum', 'record', 'trait', 'protocol'";

/// Languages where a bare call inside a class reaches the members of that class and its bases,
/// as a call through `this` or `self` does.
const IMPLICIT_RECEIVER_LANGUAGES: &[&str] = &[
    "java", "csharp", "kotlin", "swift", "cpp", "scala", "dart", "ruby", "vbnet",
];

fn receiver_builder_calls(conn: &Connection) -> String {
    // Same-file calls resolve into `relationships`; cross-file calls remain pending. Keep the
    // resolved symbol ID so same-named builders in different lexical scopes do not collapse.
    let resolved_calls = if has_column(conn, "relationships", "from_symbol_id")
        && has_column(conn, "relationships", "to_symbol_id")
        && has_column(conn, "relationships", "kind")
        && has_column(conn, "relationships", "path")
        && has_column(conn, "relationships", "start_line")
    {
        "UNION
         SELECT resolved_builder.name, assign_call.path, resolved_builder.symbol_id
         FROM symbols assigned
         JOIN relationships assign_call
           ON assign_call.from_symbol_id = p.from_symbol_id
          AND assign_call.start_line = assigned.start_line
          AND +assign_call.kind = 'calls'
         JOIN symbols resolved_builder ON resolved_builder.symbol_id = assign_call.to_symbol_id
         WHERE assigned.parent_symbol_id = p.from_symbol_id
           AND +assigned.name = p.target_receiver
           AND +assigned.kind = 'variable'
           AND assigned.start_line <= p.start_line"
    } else {
        ""
    };
    let fixture_calls = if has_column(conn, "symbols", "test_lifecycle") {
        "UNION
         SELECT fixture_call.target_terminal_name, fixture.path, NULL
         FROM symbols fixture
         JOIN pending_relationships fixture_call ON fixture_call.from_symbol_id = fixture.symbol_id
         WHERE fixture.name = p.target_receiver
           AND fixture.kind IN ('function', 'method')
           AND +fixture.test_lifecycle = 1
           AND EXISTS (
               SELECT 1 FROM symbols parameter
               WHERE parameter.parent_symbol_id = p.from_symbol_id
                 AND +parameter.name = p.target_receiver
                 AND (parameter.kind = 'parameter'
                      OR json_valid(parameter.metadata_json)
                         AND json_extract(parameter.metadata_json, '$.role') = 'parameter')
           )
           AND (fixture.path = p.path
                OR fixture.path LIKE '%conftest.py'
                   AND substr(p.path, 1, length(fixture.path) - 11)
                       = substr(fixture.path, 1, length(fixture.path) - 11))"
    } else {
        ""
    };
    // Unary `+` keeps SQLite on the parent index for common receiver names.
    format!(
        "SELECT assign_call.target_terminal_name, assign_call.path, NULL
         FROM symbols assigned
         JOIN pending_relationships assign_call
           ON assign_call.from_symbol_id = p.from_symbol_id
          AND assign_call.start_line = assigned.start_line
          AND +assign_call.kind = 'calls'
         WHERE assigned.parent_symbol_id = p.from_symbol_id
           AND +assigned.name = p.target_receiver
           AND +assigned.kind = 'variable'
           AND assigned.start_line <= p.start_line
         {resolved_calls}
         {fixture_calls}"
    )
}

fn builder_return_type_sql(builder: &str) -> String {
    format!(
        "CASE WHEN json_valid({builder}.metadata_json)
              THEN COALESCE(NULLIF(json_extract({builder}.metadata_json, '$.returnType'), ''),
                            json_extract({builder}.metadata_json, '$.inferredReturnType')) END"
    )
}

fn receiver_definition_scope(target: &str, origin: &str) -> String {
    let source = "json_extract(binding.metadata_json, '$.source')";
    let relative = is_relative_import_path(source);
    let relative_match = relative_import_matches(source, target).replace("p.path", origin);
    let caller = format!("replace({origin}, '\\', '/')");
    let parent_dir = "rtrim(substr(hop.dir, 1, length(hop.dir) - 1), replace(substr(hop.dir, 1, length(hop.dir) - 1), '/', ''))";
    format!(
        "({target}.path = {origin} OR NOT EXISTS (
            SELECT 1 FROM symbols local
            WHERE local.name = {target}.name AND local.path = {origin}
              AND local.kind = {target}.kind
         )) AND NOT EXISTS (
            SELECT 1 FROM symbols binding
            WHERE binding.name = {target}.name AND binding.path = {origin}
              AND binding.kind = 'import' AND json_valid(binding.metadata_json)
              AND NOT ({target}.path = {origin} AND binding.start_line < {target}.start_line)
              AND COALESCE({source}, '') != ''
              AND NOT CASE WHEN {relative} THEN {relative_match}
                  WHEN {source} LIKE '.%' THEN EXISTS (
                      WITH RECURSIVE hop(dir, rest) AS (
                          SELECT rtrim({caller}, replace({caller}, '/', '')), substr({source}, 2)
                          UNION ALL
                          SELECT {parent_dir}, substr(hop.rest, 2) FROM hop WHERE hop.rest LIKE '.%'
                      )
                      SELECT 1 FROM hop WHERE hop.rest NOT LIKE '.%'
                        AND (replace({target}.path, '\\', '/') = hop.dir || replace(hop.rest, '.', '/') || '.py'
                             OR replace({target}.path, '\\', '/') LIKE
                                rtrim(hop.dir || replace(hop.rest, '.', '/'), '/') || '/%')
                  )
                  ELSE instr('/' || replace({target}.path, '\\', '/'), '/' || replace({source}, '.', '/') || '.') > 0
                    OR instr('/' || replace({target}.path, '\\', '/'), '/' || replace({source}, '.', '/') || '/') > 0
                  END
         )"
    )
}

fn member_receiver_match(conn: &Connection) -> String {
    let callable_member = "p.kind = 'member_access'
                    AND CASE WHEN json_valid(p.metadata_json)
                             THEN json_extract(p.metadata_json, '$.role') END IS NOT 'signal_handler'
                    AND EXISTS (
                        SELECT 1 FROM symbols selected
                        JOIN symbols owner ON owner.symbol_id = selected.parent_symbol_id
                        WHERE selected.symbol_id = ?3
                          AND selected.kind IN ('method', 'function', 'constructor')
                          AND owner.kind IN ('class', 'struct', 'interface', 'trait', 'enum', 'protocol')
                    )";
    // A member access without metadata has no receiver evidence: julie leaves `super().x` and
    // HTML and CSS names without it. It stays a candidate for a class method and never
    // reaches another target, where the name alone matches unrelated sites.
    let unproven_member = "p.kind = 'member_access' AND NOT p.has_metadata";
    let documentation_member = format!(
        "{unproven_member} AND p.language IN ({})",
        documentation_language_list()
    );
    if !has_pending_namespace_column(conn) || !has_column(conn, "symbols", "metadata_json") {
        return format!(
            "CASE WHEN {documentation_member} THEN -1 WHEN {callable_member} THEN 0
                  WHEN {unproven_member} THEN -1 ELSE 1 END"
        );
    }
    let matched = pending_target_predicate_with_receiver_chain_status(
        conn,
        "candidate",
        "candidate_parent",
        Some("0"),
    );
    let builders = receiver_builder_calls(conn);
    let builder_return_type = builder_return_type_sql("builder");
    let class_scope = receiver_definition_scope("receiver_class", "builder_call.path");
    let builder_scope = receiver_definition_scope("builder", "builder_call.path");
    let returned_scope = receiver_definition_scope("returned_class", "builder.path");
    format!(
        "CASE WHEN {documentation_member} THEN -1
              WHEN NOT ({callable_member}) THEN CASE WHEN {unproven_member} THEN -1 ELSE 1 END
              WHEN {unproven_member} THEN 0
              WHEN (SELECT COUNT(*) FROM symbols binding
                    WHERE binding.parent_symbol_id = p.from_symbol_id
                      AND +binding.name = p.target_receiver
                      AND +binding.kind IN ('variable', 'parameter')
                      AND binding.start_line <= p.start_line) > 1 THEN 0
              WHEN EXISTS (
                  WITH builder_call(name, path, resolved_symbol_id) AS ({builders})
                  SELECT 1 FROM builder_call
                  WHERE (SELECT COUNT(*) FROM builder_call) > 1
                    AND builder_call.resolved_symbol_id IS NULL
                    AND NOT EXISTS (
                        SELECT 1 FROM symbols receiver_class
                        WHERE receiver_class.name = builder_call.name AND receiver_class.kind = 'class'
                          AND (receiver_class.symbol_id = builder_call.resolved_symbol_id
                               OR builder_call.resolved_symbol_id IS NULL AND {class_scope})
                    )
                    AND NOT EXISTS (
                        SELECT 1 FROM symbols builder
                        JOIN symbols returned_class ON returned_class.name =
                            {builder_return_type}
                        WHERE builder.name = builder_call.name AND builder.kind IN ('function', 'method')
                          AND (builder.symbol_id = builder_call.resolved_symbol_id
                               OR CASE WHEN builder_call.resolved_symbol_id IS NULL
                                            AND EXISTS (
                                                SELECT 1 FROM symbols returned_class_probe
                                                WHERE returned_class_probe.name = {builder_return_type}
                                                  AND returned_class_probe.kind = 'class'
                                            )
                                       THEN {builder_scope} ELSE 0 END)
                          AND returned_class.kind = 'class'
                          AND {returned_scope}
                    )
              ) THEN 0
              ELSE (
                  SELECT CASE WHEN COUNT(*) > 1 THEN 0
                              WHEN MAX(candidate.symbol_id = ?3) = 1 THEN 1
                              WHEN COUNT(*) = 1 THEN -1
                              WHEN EXISTS (
                                  WITH builder_call(name, path, resolved_symbol_id) AS ({builders})
                                  SELECT 1 FROM builder_call
                                  JOIN symbols imported ON imported.name = builder_call.name
                                    AND imported.path = builder_call.path AND imported.kind = 'import'
                                  JOIN symbols selected ON selected.symbol_id = ?3
                                  JOIN pending_relationships base ON base.from_symbol_id = selected.parent_symbol_id
                                    AND base.kind = 'extends' AND base.target_terminal_name = builder_call.name
                                  JOIN symbols base_import ON base_import.name = base.target_terminal_name
                                    AND base_import.path = base.path AND base_import.kind = 'import'
                                  WHERE json_valid(imported.metadata_json) AND json_valid(base_import.metadata_json)
                                    AND json_extract(imported.metadata_json, '$.source')
                                        = json_extract(base_import.metadata_json, '$.source')
                              ) THEN -1
                              ELSE 0 END
                  FROM symbols candidate
                  JOIN symbols candidate_parent ON candidate_parent.symbol_id = candidate.parent_symbol_id
                  WHERE candidate.name = p.name
                    AND candidate.kind IN ('method', 'function', 'constructor')
                    AND candidate_parent.kind IN ('class', 'struct', 'interface', 'trait', 'enum', 'protocol')
                    AND {matched}
              ) END"
    )
}

/// SQL that holds when the import row `alias` binds its name or alias in code (`const math =
/// @import(…)`, `import x as y`), and not when it only loads a file or opens its names: a C or
/// C++ `#include`, a CSS `@import`, a Bash `source`, a PowerShell dot-source or `Import-Module`,
/// an R `source()`, a Ruby `require`, a PHP `require` or `include`, an F# `open` or `#load`, or a
/// Java star import. `binds_name` in `reference_report/loaded_files.rs` is the same rule.
pub(crate) fn import_binds_name_sql(alias: &str) -> String {
    let metadata =
        format!("CASE WHEN json_valid({alias}.metadata_json) THEN {alias}.metadata_json END");
    let form = format!("COALESCE(json_extract({metadata}, '$.form'), '')");
    format!(
        "(json_extract({metadata}, '$.wildcard') IS NOT 1
          AND CASE {alias}.language
              WHEN 'c' THEN 0 WHEN 'css' THEN 0 WHEN 'bash' THEN 0
              WHEN 'powershell' THEN 0 WHEN 'fsharp' THEN 0
              WHEN 'cpp' THEN {form} IN ('namespace', 'using', 'using namespace')
              WHEN 'ruby' THEN {form} = 'autoload'
              WHEN 'r' THEN {form} NOT IN ('source()', '')
              WHEN 'php' THEN {form} NOT IN ('require', 'require_once', 'include', 'include_once')
              ELSE 1 END)"
    )
}

/// SQL that holds when the caller's file `p.path` opens the module that holds `target` (F#
/// `open Make` for `Make.make`), so a bare call there names the members of that module. julie
/// writes such an import with the form `open` and the module path as its source. The open binds
/// no name of its own ([`import_binds_name_sql`]); it brings the members of the module into the
/// file's scope. `0` without import metadata.
pub(crate) fn opened_module_sql(conn: &Connection, target: &str) -> String {
    if !has_column(conn, "symbols", "metadata_json") {
        return "0".to_string();
    }
    format!(
        "EXISTS (
            SELECT 1 FROM symbols opened
            JOIN symbols opened_module ON opened_module.symbol_id = {target}.parent_symbol_id
            WHERE opened.kind = 'import' AND opened.path = p.path
              AND opened_module.kind IN ('module', 'namespace')
              AND json_valid(opened.metadata_json)
              AND json_extract(opened.metadata_json, '$.form') = 'open'
              AND (json_extract(opened.metadata_json, '$.source') = opened_module.name
                   OR json_extract(opened.metadata_json, '$.source')
                      LIKE '%.' || opened_module.name))"
    )
}

/// SQL that holds when the caller's file `p.path` binds `binding_name` through an import whose
/// source module, or imported name, names a folder or file on the path of `target`. A Java import
/// and a GDScript `preload` have rules of their own.
pub(crate) fn module_import_binding_sql(target: &str, binding_name: &str) -> String {
    let target_path = format!("('/' || replace({target}.path, '\\', '/'))");
    let dotted = "replace(module_path.value, '/', '.')";
    let segment = format!("replace({dotted}, rtrim({dotted}, replace({dotted}, '.', '')), '')");
    let relative = is_relative_import_path("import_source.value");
    let relative_match = relative_import_matches("import_source.value", target);
    let binds = import_binds_name_sql("module_import");
    format!(
        "EXISTS (
                        SELECT 1 FROM symbols module_import
                        CROSS JOIN json_each(json_array(
                            json_extract(module_import.metadata_json, '$.source')
                        )) import_source
                        CROSS JOIN json_each(json_array(
                            json_extract(module_import.metadata_json, '$.importedName'),
                            import_source.value
                        )) module_path
                        WHERE module_import.kind = 'import'
                          AND module_import.path = p.path
                          AND module_import.name = {binding_name}
                          AND json_valid(module_import.metadata_json)
                          AND module_import.language NOT IN ('java', 'gdscript')
                          AND {binds}
                          AND CASE WHEN {relative} THEN {relative_match}
                              ELSE {segment} != ''
                                   AND ({target_path} LIKE '%/' || {segment} || '/%'
                                        OR {target_path} LIKE '%/' || {segment} || '.%')
                              END
                    )"
    )
}

/// SQL that holds when the receiver `p.target_receiver` is a parameter or variable of the caller
/// that a call builds as the class owning `target`, directly or through a builder's return type
/// or a fixture, and no nearer class in the chain defines the name. `None` without metadata.
pub(crate) fn built_receiver_sql(conn: &Connection, target: &str) -> Option<String> {
    if !has_column(conn, "symbols", "metadata_json") {
        return None;
    }
    let builder_calls = receiver_builder_calls(conn);
    let builder_return_type = builder_return_type_sql("builder");
    let builder_scope = receiver_definition_scope("builder", "builder_call.path");
    let returned_class_scope = receiver_definition_scope("returned_class", "builder.path");
    let built_scope = receiver_definition_scope("built_class", "built_name.path");
    // CASE prevents scope checks for builders that cannot return an indexed class.
    Some(format!(
        "CASE WHEN EXISTS (
                        SELECT 1 FROM symbols taken
                        WHERE taken.parent_symbol_id = p.from_symbol_id
                          AND taken.name = p.target_receiver
                          AND taken.kind IN ('parameter', 'variable')
                    ) THEN EXISTS (
                        WITH RECURSIVE builder_call(name, path, resolved_symbol_id) AS ({builder_calls}),
                        built_name(name, path, resolved_symbol_id) AS (
                            SELECT name, path, resolved_symbol_id FROM builder_call
                            UNION
                            SELECT returned_class.name, builder.path, returned_class.symbol_id
                            FROM builder_call
                            JOIN symbols builder ON builder.name = builder_call.name
                              AND builder.kind IN ('function', 'method')
                              AND (builder.symbol_id = builder_call.resolved_symbol_id
                                   OR CASE WHEN builder_call.resolved_symbol_id IS NULL
                                                AND EXISTS (
                                                    SELECT 1 FROM symbols returned_class_probe
                                                    WHERE returned_class_probe.name = {builder_return_type}
                                                      AND returned_class_probe.kind = 'class'
                                                )
                                           THEN {builder_scope} ELSE 0 END)
                            JOIN symbols returned_class ON returned_class.name = {builder_return_type}
                              AND returned_class.kind = 'class' AND {returned_class_scope}
                        ),
                        built(symbol_id, depth) AS (
                            SELECT built_class.symbol_id, 0
                            FROM built_name
                            CROSS JOIN symbols built_class ON built_class.name = built_name.name
                            WHERE built_class.kind = 'class' AND {built_scope}
                              AND (built_name.resolved_symbol_id IS NULL
                                   OR built_class.symbol_id = built_name.resolved_symbol_id)
                            UNION
                            SELECT r.to_symbol_id, built.depth + 1
                            FROM relationships r JOIN built ON r.from_symbol_id = built.symbol_id
                            WHERE +r.kind IN ('extends', 'implements') AND built.depth < 8
                            UNION
                            SELECT c.symbol_id, built.depth + 1
                            FROM pending_relationships pe
                            JOIN built ON pe.from_symbol_id = built.symbol_id
                            JOIN symbols c ON c.name = pe.target_terminal_name
                            WHERE +pe.kind IN ('extends', 'implements') AND built.depth < 8
                              AND c.kind IN ('class', 'interface', 'struct', 'trait', 'protocol')
                        )
                        SELECT 1 FROM built WHERE built.symbol_id = {target}.parent_symbol_id
                          AND NOT EXISTS (
                              SELECT 1 FROM built closer
                              JOIN symbols nearer ON nearer.parent_symbol_id = closer.symbol_id
                              WHERE nearer.name = {target}.name
                                AND nearer.kind IN ('method', 'function', 'constructor', 'property')
                                AND closer.depth < built.depth
                          )
                    ) ELSE 0 END"
    ))
}

/// SQL that holds when a variable or parameter named `p.target_receiver` in the caller's file has
/// a type fact naming the parent of `target`, and no other variable of the caller shadows it.
pub(crate) fn receiver_type_sql(conn: &Connection, parent: &str) -> String {
    // Type facts retain only the terminal type name; constrain them to the imported definition.
    let receiver_type_scope = if has_column(conn, "symbols", "metadata_json") {
        receiver_definition_scope(parent, "p.path")
    } else {
        "1 = 1".to_string()
    };
    format!(
        "EXISTS (
                        SELECT 1 FROM symbols receiver
                        JOIN type_facts receiver_type ON receiver_type.symbol_id = receiver.symbol_id
                        WHERE receiver.name = p.target_receiver
                          AND receiver.path = p.path
                          AND receiver_type.resolved_type = {parent}.name
                          AND NOT EXISTS (
                              SELECT 1 FROM symbols shadow
                              WHERE shadow.name = receiver.name
                                AND shadow.path = receiver.path
                                AND shadow.parent_symbol_id = p.from_symbol_id
                                AND shadow.kind IN ('variable', 'parameter')
                                AND shadow.symbol_id != receiver.symbol_id
                          )
                    ) AND {receiver_type_scope}"
    )
}

/// SQL that holds when `target` is a member of the class chain of the caller `s_from`: its own
/// class when `own_class_rule` holds, or a base class or interface named by an `extends` or
/// `implements` row, where the nearest type that defines the name wins. julie resolves most `this`
/// calls to the caller's own class, but not one inside an ordinary JavaScript function, so a
/// `this`, `self`, or `cls` call also matches a sibling member. A base that a pending `extends` or
/// `implements` row names counts unless a type of that name is closer.
pub(crate) fn class_chain_sql(target: &str, own_class_rule: &str) -> String {
    let rival_rank = path_proximity("rival.path", "pe.path");
    let base_rank = path_proximity("c.path", "pe.path");
    let inherited_write = inherited_attribute_write("nearer");
    format!(
        "EXISTS (
                            WITH RECURSIVE base(symbol_id, depth) AS (
                                SELECT s_from.parent_symbol_id, 0
                                UNION
                                SELECT r.to_symbol_id, base.depth + 1
                                FROM relationships r JOIN base ON r.from_symbol_id = base.symbol_id
                                WHERE +r.kind IN ('extends', 'implements') AND base.depth < 8
                                UNION
                                SELECT c.symbol_id, base.depth + 1
                                FROM pending_relationships pe
                                JOIN base ON pe.from_symbol_id = base.symbol_id
                                JOIN symbols c ON c.name = pe.target_terminal_name
                                WHERE +pe.kind IN ('extends', 'implements') AND base.depth < 8
                                  AND c.kind IN ('class', 'interface', 'struct', 'trait', 'protocol')
                                  AND NOT EXISTS (
                                      SELECT 1 FROM symbols rival
                                      WHERE rival.name = c.name
                                        AND rival.kind IN ('class', 'interface', 'struct', 'trait', 'protocol')
                                        AND {rival_rank} > {base_rank})
                            )
                            SELECT 1 FROM base
                            WHERE base.symbol_id = {target}.parent_symbol_id
                              AND (base.depth > 0 OR {own_class_rule})
                              AND NOT EXISTS (
                                  SELECT 1 FROM base closer
                                  JOIN symbols nearer ON nearer.parent_symbol_id = closer.symbol_id
                                  WHERE nearer.name = {target}.name
                                    AND nearer.kind IN ('method', 'function', 'constructor', 'property')
                                    AND closer.depth < base.depth
                                    AND (closer.depth > 0 OR p.target_receiver IS NOT 'super')
                                    AND NOT {inherited_write}
                              )
                        )"
    )
}

/// The quoted, comma-separated [`IMPLICIT_RECEIVER_LANGUAGES`] for an SQL `IN` list.
pub(crate) fn implicit_receiver_language_list() -> String {
    IMPLICIT_RECEIVER_LANGUAGES
        .iter()
        .map(|language| format!("'{language}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// SQL that holds when `target` is defined in a scope that encloses the caller's parent, where the
/// nearest scope that defines or imports the name wins. An enclosing class body contributes only
/// its nested types, and only where a bare call reaches members; its methods are left to the
/// class-chain rule, because julie parses macro calls in C++ test files as methods of made-up
/// classes.
pub(crate) fn enclosing_scope_sql(target: &str) -> String {
    let implicit_receiver_languages = implicit_receiver_language_list();
    format!(
        "EXISTS (
            WITH RECURSIVE lexical(symbol_id, depth) AS (
                SELECT s_from.parent_symbol_id, 1
                UNION ALL
                SELECT outer_scope.parent_symbol_id, lexical.depth + 1
                FROM symbols outer_scope JOIN lexical ON outer_scope.symbol_id = lexical.symbol_id
                WHERE outer_scope.parent_symbol_id IS NOT NULL AND lexical.depth < 32
            )
            SELECT 1 FROM lexical JOIN symbols scope ON scope.symbol_id = lexical.symbol_id
            WHERE lexical.symbol_id = {target}.parent_symbol_id
              AND lexical.depth > 1
              AND (scope.kind NOT IN ({MEMBER_LIST_OWNER_KINDS})
                   OR scope.kind IN ('module', 'namespace')
                   OR {target}.kind IN ({NESTED_TYPE_KINDS})
                      AND s_from.language IN ({implicit_receiver_languages}))
              AND NOT EXISTS (
                  SELECT 1 FROM lexical closer
                  JOIN symbols nearer ON nearer.parent_symbol_id = closer.symbol_id
                  WHERE closer.depth < lexical.depth
                    AND nearer.name = {target}.name
                    AND nearer.kind IN ({target}.kind, 'import')
              )
        )"
    )
}

/// SQL that holds when `target` is a function defined in the initializer of a property, field,
/// variable, or constant (Scala `val v = { def local() = 1; local() }`) and row `p` lies in that
/// initializer. julie names the class around the value as the caller of such a row.
pub(crate) fn initializer_scope_sql(target: &str) -> String {
    format!(
        "({target}.kind = 'function' AND EXISTS (
            SELECT 1 FROM symbols holder
            WHERE holder.symbol_id = {target}.parent_symbol_id
              AND holder.kind IN ('property', 'field', 'variable', 'constant')
              AND holder.path = p.path
              AND p.start_line BETWEEN holder.start_line AND holder.end_line))"
    )
}

const BINDING_VALUE_KINDS: &str = "'variable', 'constant', 'property', 'field'";

/// SQL predicate: row `p` lies where the owner `owner` is named. A function, method, or
/// constructor that holds the owner as a variable or constant, or as a property or field of the
/// object literal of one (`var ctx = { res: { send() {…} } }`), is the only scope that names
/// it; any other owner is named everywhere.
pub(crate) fn owner_binding_in_scope_sql(owner: &str) -> String {
    format!(
        "NOT EXISTS (
            WITH RECURSIVE binding(symbol_id, kind, parent_symbol_id, held_kind, depth) AS (
                SELECT {owner}.symbol_id, {owner}.kind, {owner}.parent_symbol_id, NULL, 0
                UNION ALL
                SELECT holder.symbol_id, holder.kind, holder.parent_symbol_id, binding.kind,
                       binding.depth + 1
                FROM symbols holder JOIN binding ON holder.symbol_id = binding.parent_symbol_id
                WHERE binding.kind IN ({BINDING_VALUE_KINDS}) AND binding.depth < 8
            )
            SELECT 1 FROM binding
            JOIN symbols binding_scope ON binding_scope.symbol_id = binding.symbol_id
            WHERE binding.held_kind IN ('variable', 'constant')
              AND binding_scope.kind IN ('function', 'method', 'constructor')
              AND NOT (binding_scope.path = p.path
                       AND p.start_line BETWEEN binding_scope.start_line AND binding_scope.end_line)
        )"
    )
}

/// The receiver chain of the pending row `p`: its namespace segments, then its receiver.
const PENDING_RECEIVER_CHAIN_JSON: &str = "json_insert(
        CASE WHEN json_valid(p.target_namespace_json)
                   AND json_type(p.target_namespace_json) = 'array'
             THEN p.target_namespace_json ELSE '[]' END,
        '$[#]', p.target_receiver
    )";

/// The owner of the receiver chain of the pending row `p`, whose caller is joined as `s_from`.
fn pending_receiver_chain_owner_sql() -> String {
    typed_receiver_chain_owner_id_sql(
        "p.from_symbol_id",
        "p.path",
        "p.start_line",
        "COALESCE(s_from.language, '')",
        PENDING_RECEIVER_CHAIN_JSON,
    )
}

/// SQL predicate that decides whether a pending call edge `p` (with caller `s_from`) points at
/// the candidate definition `target` (whose parent symbol is joined as `parent`).
pub(crate) fn pending_target_predicate(conn: &Connection, target: &str, parent: &str) -> String {
    pending_target_predicate_with_receiver_chain_status(conn, target, parent, None)
}

fn pending_target_predicate_without_typed_receiver_chain(
    conn: &Connection,
    target: &str,
    parent: &str,
) -> String {
    pending_target_predicate_builder(conn, target, parent, None, None, false)
}

/// [`pending_target_predicate`] with the receiver-chain owner of the pending row `p` read from
/// `receiver_chain_owner`, an expression over values that [`pending_receiver_chain_owners`]
/// computed with the same owner query. Inline, SQLite computes the owner once for each read of
/// the owner walk, which made one walk level over `json.hpp` take a minute.
fn pending_target_predicate_with_receiver_chain_owner(
    conn: &Connection,
    target: &str,
    parent: &str,
    receiver_chain_owner: &str,
) -> String {
    pending_target_predicate_builder(conn, target, parent, None, Some(receiver_chain_owner), true)
}

pub(crate) fn pending_target_predicate_with_receiver_chain_status(
    conn: &Connection,
    target: &str,
    parent: &str,
    receiver_chain_status: Option<&str>,
) -> String {
    pending_target_predicate_builder(conn, target, parent, receiver_chain_status, None, true)
}

fn pending_target_predicate_builder(
    conn: &Connection,
    target: &str,
    parent: &str,
    receiver_chain_status: Option<&str>,
    receiver_chain_owner: Option<&str>,
    include_typed_receiver_chain: bool,
) -> String {
    let ns = "json_each(CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END)";
    let target_path = format!("('/' || replace({target}.path, '\\', '/'))");
    let like_value = "replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_')";
    let target_rank = call_site_proximity(&format!("{target}.path"));
    let closer_path = "replace(closer.path, '\\', '/')";
    let site_path = "replace(p.path, '\\', '/')";
    let closer_folder = format!("rtrim({closer_path}, replace({closer_path}, '/', ''))");
    let site_folder = format!("rtrim({site_path}, replace({site_path}, '/', ''))");
    let closer = format!(
        "SELECT 1 FROM symbols closer
                    WHERE closer.name = {target}.name
                      AND closer.symbol_id != {target}.symbol_id
                      AND closer.parent_symbol_id IS NULL
                      AND closer.kind = {target}.kind
                      AND NOT (p.kind IS 'extends' AND closer.symbol_id = p.from_symbol_id)"
    );
    let relative = is_relative_import_path("import_source.value");
    let relative_match = relative_import_matches("import_source.value", target);
    // `import { util } from "../helpers/util.js"`: `util.f()` reaches only that file's `util`.
    let receiver_import_elsewhere = if has_column(conn, "symbols", "metadata_json") {
        format!(
            "AND NOT EXISTS (
                            SELECT 1 FROM symbols receiver_import
                            CROSS JOIN json_each(json_array(
                                json_extract(receiver_import.metadata_json, '$.source')
                            )) import_source
                            WHERE receiver_import.kind = 'import'
                              AND receiver_import.path = p.path
                              AND receiver_import.name = p.target_receiver
                              AND json_valid(receiver_import.metadata_json)
                              AND {relative}
                              AND NOT {relative_match}
                        )"
        )
    } else {
        String::new()
    };
    let module_import = module_import_binding_sql(target, "p.target_receiver");
    let import_alias_receiver = if has_column(conn, "symbols", "metadata_json") {
        format!("OR EXISTS (
                        SELECT 1 FROM symbols alias_import
                        WHERE alias_import.kind = 'import'
                          AND alias_import.path = p.path
                          AND json_valid(alias_import.metadata_json)
                          AND (json_extract(alias_import.metadata_json, '$.alias') = p.target_receiver
                               OR json_extract(alias_import.metadata_json, '$.local_name') = p.target_receiver)
                          AND COALESCE(json_extract(alias_import.metadata_json, '$.source'), '') NOT LIKE 'Qt%'
                    )
                    OR {module_import}"
        )
    } else {
        String::new()
    };
    let receiver_chain_json = PENDING_RECEIVER_CHAIN_JSON;
    let typed_receiver_chain_supported = has_typed_receiver_chain_support(conn);
    let typed_receiver_chain = if typed_receiver_chain_supported && include_typed_receiver_chain {
        let matches_target = match receiver_chain_status {
            Some(status) => format!(
                "({status}) IN ({RECEIVER_CHAIN_STATUS_RECEIVER_TYPE}, {RECEIVER_CHAIN_STATUS_QUALIFIED_PATH})"
            ),
            // A NULL owner gives the status NULL or 0, so a bound owner skips the status query.
            None => match receiver_chain_owner {
                Some(owner) => format!(
                    "({owner} IS NOT NULL AND {})",
                    typed_receiver_chain_matches_target_sql(owner, target)
                ),
                None => typed_receiver_chain_matches_target_sql(
                    &pending_receiver_chain_owner_sql(),
                    target,
                ),
            },
        };
        format!(
            "(json_array_length({receiver_chain_json}) > 1
              AND p.target_receiver IS NOT NULL AND p.target_receiver != ''
              AND {matches_target})"
        )
    } else {
        "0".to_string()
    };
    let receiver_type = if has_type_fact_support(conn) {
        receiver_type_sql(conn, parent)
    } else {
        "0".to_string()
    };
    let receiver_type = if typed_receiver_chain_supported && include_typed_receiver_chain {
        format!(
            "CASE WHEN json_array_length({receiver_chain_json}) > 1 THEN 0 ELSE {receiver_type} END"
        )
    } else {
        receiver_type
    };
    let fixture_receiver = match built_receiver_sql(conn, target) {
        Some(built) => format!("OR {built}"),
        None => String::new(),
    };
    let inherited = class_chain_sql(target, &format!("{target}.kind IN ('variable', 'field')"));
    let receiver_chain = class_chain_sql(target, "p.target_receiver IS NOT 'super'");
    let implicit_receiver_languages = implicit_receiver_language_list();
    let enclosing = enclosing_scope_sql(target);
    let in_initializer = initializer_scope_sql(target);
    let opened_module = opened_module_sql(conn, target);
    let rust_path = rust_path_rule(target, ns);
    let owner_in_scope = owner_binding_in_scope_sql(parent);
    let predicate = format!(
        "(
            NOT (p.kind IS 'extends' AND p.from_symbol_id = {target}.symbol_id)
            AND (
            (
                {target}.parent_symbol_id IS NOT NULL
                AND {parent}.name IS NOT NULL
                AND (
                    (
                        (
                            EXISTS (SELECT 1 FROM {ns} WHERE value = {parent}.name) AND {owner_in_scope}
                            OR (EXISTS (SELECT 1 FROM {ns} WHERE value = 'Self')
                                AND s_from.parent_symbol_id = {target}.parent_symbol_id)
                            OR (p.target_receiver IS NOT NULL AND p.target_receiver != '' AND {parent}.name = p.target_receiver
                                AND {owner_in_scope}
                                {receiver_import_elsewhere})
                            OR ((p.target_receiver IN ('self', 'this', 'cls', 'Self', 'super')
                                 OR EXISTS (SELECT 1 FROM {ns} WHERE value = 'Self'))
                                AND {receiver_chain})
                            OR {receiver_type}
                            {fixture_receiver}
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
                    OR {typed_receiver_chain}
                )
            )
            OR (
                (p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')
                AND (
                    p.target_receiver IS NULL
                    OR p.target_receiver = ''
                    {import_alias_receiver}
                )
                AND ({target}.parent_symbol_id IS NULL
                     OR s_from.parent_symbol_id = {target}.parent_symbol_id
                     OR {target}.parent_symbol_id = p.from_symbol_id
                     OR {in_initializer}
                     OR CASE WHEN p.target_receiver IS NULL OR p.target_receiver = ''
                        THEN {enclosing} OR {opened_module}
                        ELSE 0 END
                     OR CASE WHEN (p.target_receiver IS NULL OR p.target_receiver = '')
                                  AND s_from.language IN ({implicit_receiver_languages})
                        THEN {parent}.path = s_from.path
                             AND {parent}.name = (
                                 SELECT caller_owner.name FROM symbols caller_owner
                                 WHERE caller_owner.symbol_id = s_from.parent_symbol_id
                                   AND caller_owner.kind = {parent}.kind
                             )
                             OR {inherited}
                        ELSE 0 END)
                AND (p.target_receiver IS NOT NULL AND p.target_receiver != '' OR {target}.path = p.path OR NOT EXISTS (
                    SELECT 1 FROM symbols shadow
                    WHERE shadow.name = {target}.name
                      AND shadow.path = p.path
                      AND shadow.symbol_id != p.from_symbol_id
                      AND shadow.kind NOT IN ('import', 'export', 'module', 'namespace')
                      AND (shadow.parent_symbol_id IS NULL OR shadow.parent_symbol_id = p.from_symbol_id)
                ))
                AND ({target}.parent_symbol_id IS NOT NULL OR CASE {target_rank}
                    WHEN 2 THEN 1
                    WHEN 1 THEN NOT EXISTS ({closer} AND {closer_path} = {site_path})
                    ELSE NOT EXISTS ({closer} AND {closer_folder} = {site_folder}) END)
            )
            OR (
                {target}.parent_symbol_id IS NULL
                AND (
                    EXISTS (
                        SELECT 1 FROM {ns}
                        WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super')
                          AND {target_path} LIKE '%/' || {like_value} || '.%' ESCAPE '\\'
                    )
                    OR ({rust_path})
                )
            )
            )
        )"
    );
    if typed_receiver_chain_supported
        && include_typed_receiver_chain
        && let Some(status) = receiver_chain_status
    {
        format!(
            "CASE WHEN json_array_length({receiver_chain_json}) > 1
                       AND p.target_receiver IS NOT NULL AND p.target_receiver != ''
                       AND COALESCE(({status}), 0) != {RECEIVER_CHAIN_STATUS_LEGACY}
                  THEN COALESCE(({status}) IN ({RECEIVER_CHAIN_STATUS_RECEIVER_TYPE}, {RECEIVER_CHAIN_STATUS_QUALIFIED_PATH}), 0)
                  ELSE {predicate} END"
        )
    } else {
        predicate
    }
}

fn pending_receiver_chain_scope_requires_full(
    conn: &Connection,
    scope: &str,
    values: &[String],
) -> bool {
    if values.is_empty()
        || !has_typed_receiver_chain_support(conn)
        || !has_pending_namespace_column(conn)
    {
        return false;
    }
    let Ok(values_json) = serde_json::to_string(values) else {
        return true;
    };
    let sql = format!(
        "SELECT 1 FROM pending_relationships p
         WHERE {scope}
           AND (
                (p.target_namespace_json IS NOT NULL
                 AND (NOT json_valid(p.target_namespace_json)
                      OR json_type(CASE WHEN json_valid(p.target_namespace_json)
                                        THEN p.target_namespace_json ELSE '[]' END) != 'array'))
                OR (p.target_receiver IS NOT NULL AND p.target_receiver != ''
                    AND json_valid(p.target_namespace_json)
                    AND json_type(CASE WHEN json_valid(p.target_namespace_json)
                                       THEN p.target_namespace_json ELSE '[]' END) = 'array'
                    AND json_array_length(CASE WHEN json_valid(p.target_namespace_json)
                                               THEN p.target_namespace_json ELSE '[]' END) > 0)
           )
         LIMIT 1"
    );
    conn.query_row(&sql, [&values_json], |_| Ok(()))
        .optional()
        .map(|row| row.is_some())
        .unwrap_or(true)
}

fn pending_receiver_chain_requires_full_for_source(conn: &Connection, source_id: &str) -> bool {
    pending_receiver_chain_scope_requires_full(
        conn,
        "p.from_symbol_id IN (SELECT value FROM json_each(?1))",
        &[source_id.to_string()],
    )
}

fn pending_receiver_chain_requires_full_for_target_name(
    conn: &Connection,
    target_name: &str,
) -> bool {
    pending_receiver_chain_scope_requires_full(
        conn,
        "p.target_terminal_name IN (SELECT value FROM json_each(?1))",
        &[target_name.to_string()],
    )
}

fn pending_receiver_chain_requires_full_for_frontier(
    conn: &Connection,
    frontier: &[(String, usize, i64)],
) -> bool {
    if frontier.is_empty()
        || !has_typed_receiver_chain_support(conn)
        || !has_pending_namespace_column(conn)
    {
        return false;
    }
    let frontier_ids: Vec<&str> = frontier
        .iter()
        .map(|(symbol_id, _, _)| symbol_id.as_str())
        .collect();
    let Ok(frontier_json) = serde_json::to_string(&frontier_ids) else {
        return true;
    };
    let sql = "WITH frontier_ids(symbol_id) AS (
                   SELECT value FROM json_each(?1)
               ), target_names(name) AS (
                   SELECT s.name
                   FROM frontier_ids f JOIN symbols s ON s.symbol_id = f.symbol_id
                   UNION
                   SELECT parent.name
                   FROM frontier_ids f
                   JOIN symbols ctor ON ctor.symbol_id = f.symbol_id AND ctor.kind = 'constructor'
                   JOIN symbols parent ON parent.symbol_id = ctor.parent_symbol_id
                   WHERE parent.kind IN ('class', 'struct', 'record')
               )
               SELECT 1
               FROM pending_relationships p
               JOIN target_names target ON target.name = p.target_terminal_name
               WHERE (p.target_namespace_json IS NOT NULL
                      AND (NOT json_valid(p.target_namespace_json)
                           OR json_type(CASE WHEN json_valid(p.target_namespace_json)
                                             THEN p.target_namespace_json ELSE '[]' END) != 'array'))
                  OR (p.target_receiver IS NOT NULL AND p.target_receiver != ''
                      AND json_valid(p.target_namespace_json)
                      AND json_type(CASE WHEN json_valid(p.target_namespace_json)
                                         THEN p.target_namespace_json ELSE '[]' END) = 'array'
                      AND json_array_length(CASE WHEN json_valid(p.target_namespace_json)
                                                 THEN p.target_namespace_json ELSE '[]' END) > 0)
               LIMIT 1";
    conn.query_row(sql, [&frontier_json], |_| Ok(()))
        .optional()
        .map(|row| row.is_some())
        .unwrap_or(true)
}

/// SQL predicate for a Rust call written with a path (`crate::f()`, `super::f()`, `my_crate::f()`,
/// `crate::module::f()`) that reaches the top-level function `target`. A package folder is the
/// path before the last `/src/`; a leading `crate`, `self`, or `super` names the caller's own
/// package, a leading crate name names the package folder with `-` read as `_`, and every other
/// segment must be a file stem or folder below that `src/`. A call rooted at `std`, `core`, or
/// `alloc` never matches.
fn rust_path_rule(target: &str, ns: &str) -> String {
    let target_path = format!("('/' || replace({target}.path, '\\', '/'))");
    let caller_path = "('/' || replace(p.path, '\\', '/'))";
    let skipped = "'std', 'core', 'alloc', 'crate', 'self', 'super', 'Self'";
    let like_segment = "replace(replace(replace(seg.value, '\\', '\\\\'), '%', '\\%'), '_', '\\_')";
    let in_module_path = format!(
        "(module_path.value LIKE '%/' || {like_segment} || '.%' ESCAPE '\\'
          OR module_path.value LIKE '%/' || {like_segment} || '/%' ESCAPE '\\')"
    );
    format!(
        "s_from.language = 'rust'
        AND {target}.language = 'rust'
        AND EXISTS (
            SELECT 1
            FROM json_each(json_array(replace({target_path}, '/src/', char(1)))) marked
            CROSS JOIN json_each(json_array(
                CASE WHEN instr(marked.value, char(1)) > 0
                     THEN length({target_path})
                          - length(replace(marked.value, rtrim(marked.value, replace(marked.value, char(1), '')), ''))
                     ELSE 0 END)) src
            CROSS JOIN json_each(json_array(
                CASE WHEN src.value > 0 THEN replace(substr({target_path}, src.value), '-', '_') ELSE '' END)) module_path
            WHERE (
                EXISTS (
                    SELECT 1 FROM {ns} head
                    WHERE head.key = 0
                      AND (head.value IN ('crate', 'self', 'super')
                           AND ({target}.path = p.path
                                OR src.value > 0
                                   AND substr({caller_path}, 1, src.value) = substr({target_path}, 1, src.value))
                           OR head.value NOT IN ({skipped})
                              AND src.value >= length(head.value) + 6
                              AND replace(substr({target_path}, src.value - length(head.value) - 5, length(head.value) + 6), '-', '_')
                                  = '/' || head.value || '/src/')
                )
                AND NOT EXISTS (
                    SELECT 1 FROM {ns} seg
                    WHERE seg.key > 0 AND seg.value NOT IN ({skipped}) AND NOT {in_module_path}
                )
                OR src.value > 0
                   AND EXISTS (SELECT 1 FROM {ns} head WHERE head.key = 0 AND head.value NOT IN ({skipped}))
                   AND NOT EXISTS (
                       SELECT 1 FROM {ns} seg
                       WHERE seg.value NOT IN ({skipped}) AND NOT {in_module_path}
                   )
            )
        )"
    )
}

/// SQL predicate excluding rows julie marked as documentation, or the always-true `1 = 1` when
/// the column is absent, because a bare `1` in ORDER BY means the first result column in SQLite.
pub(crate) const DOCUMENTATION_LANGUAGES: &[&str] = &[
    "markdown", "yaml", "toml", "json", "html", "css", "xml", "ini", "text",
];

pub(crate) fn documentation_language_list() -> String {
    DOCUMENTATION_LANGUAGES
        .iter()
        .map(|l| format!("'{l}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The languages of data, configuration, and prose files, whose lookup rows follow the code rows
/// of the same rank: the documentation languages and Go's module files.
fn non_code_language_list() -> String {
    DOCUMENTATION_LANGUAGES
        .iter()
        .chain(&["gomod", "gosum"])
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

pub(crate) fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2",
        [table, column],
        |_| Ok(true),
    )
    .unwrap_or(false)
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
    if direction == "callees" {
        return find_callees_page_internal(
            conn,
            symbol_name,
            symbol_id,
            limit,
            0,
            include_external,
        )
        .map(|(sites, _)| sites);
    }
    let mut sites = find_direct_references(conn, symbol_name, limit, symbol_id)?;
    let class = match symbol_id.filter(|_| direction == "callers") {
        Some(id) => constructed_class(conn, id)?,
        None => None,
    };
    if let Some((class_id, class_name)) = class.filter(|_| sites.len() < limit) {
        let mut builds =
            find_direct_references(conn, &class_name, limit - sites.len(), Some(&class_id))?;
        builds.retain(|site| site.kind == "calls");
        sites.extend(builds);
        sites.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then(a.start_line.cmp(&b.start_line))
                .then(a.start_column.cmp(&b.start_column))
        });
    }
    if direction == "callers" {
        populate_lambda_enclosing_names(conn, &mut sites)?;
    }
    Ok(sites)
}

pub(crate) fn populate_lambda_enclosing_names(
    conn: &Connection,
    sites: &mut [ReferenceSite],
) -> Result<(), QueryError> {
    if !sites
        .iter()
        .any(|site| has_generated_lambda_name(&site.from_symbol_name))
    {
        return Ok(());
    }
    let mut stmt = conn.prepare(
        "SELECT caller.language, caller.kind, caller.signature, parent.name
         FROM symbols caller
         LEFT JOIN symbols parent ON parent.symbol_id = caller.parent_symbol_id
         WHERE caller.symbol_id = ?1",
    )?;
    for site in sites
        .iter_mut()
        .filter(|site| has_generated_lambda_name(&site.from_symbol_name))
    {
        let details = stmt
            .query_row([&site.from_symbol_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .optional()?;
        if let Some((language, kind, signature, enclosing_symbol_name)) = details
            && is_generated_lambda_name(
                &site.from_symbol_name,
                &language,
                &kind,
                signature.as_deref(),
            )
        {
            site.enclosing_symbol_name = enclosing_symbol_name;
        }
    }
    Ok(())
}

/// The class a constructor builds, so the calls that build the class count as its callers.
pub(crate) fn constructed_class(
    conn: &Connection,
    symbol_id: &str,
) -> Result<Option<(String, String)>, QueryError> {
    Ok(conn
        .query_row(
            "SELECT class.symbol_id, class.name
             FROM symbols ctor JOIN symbols class ON class.symbol_id = ctor.parent_symbol_id
             WHERE ctor.symbol_id = ?1 AND ctor.kind = 'constructor'
               AND class.kind IN ('class', 'struct', 'record')",
            params![symbol_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}

/// A type-usage or member-access identifier that resolves to the selected target.
pub(crate) struct DirectIdentifier {
    pub identifier_id: String,
    pub site: ReferenceSite,
}

/// Type-usage and member-access identifiers named `symbol_name` that reach the selected target,
/// at most `remaining`: supported rows first, then candidates, each in source order.
pub(crate) fn direct_identifier_rows(
    conn: &Connection,
    symbol_name: &str,
    symbol_id: Option<&str>,
    remaining: usize,
) -> Result<Vec<DirectIdentifier>, QueryError> {
    let receiver_match = member_receiver_match(conn);
    // A name-only type usage reaches only the closest same-named type visible from
    // the caller's lexical scopes, then the file's top-level types.
    let type_usage_shadow = format!(
        "i.kind = 'type_usage'
                 AND ?2 IS NOT NULL
                 AND selected_target.kind IN ({NESTED_TYPE_KINDS})
                 AND EXISTS (
                     WITH RECURSIVE lexical(symbol_id, depth) AS (
                         SELECT s_from.symbol_id, 0
                         UNION ALL
                         SELECT scope.parent_symbol_id, lexical.depth + 1
                         FROM symbols scope JOIN lexical ON scope.symbol_id = lexical.symbol_id
                         WHERE scope.parent_symbol_id IS NOT NULL AND lexical.depth < 32
                     ), visible_types(symbol_id, depth) AS (
                         SELECT shadow.symbol_id, lexical.depth
                         FROM lexical
                         JOIN symbols shadow ON shadow.parent_symbol_id = lexical.symbol_id
                         WHERE shadow.name = selected_target.name
                           AND shadow.kind IN ({NESTED_TYPE_KINDS})
                           AND shadow.language = i.language
                         UNION ALL
                         SELECT shadow.symbol_id, 100
                         FROM symbols shadow
                         WHERE shadow.name = selected_target.name
                           AND shadow.kind IN ({NESTED_TYPE_KINDS})
                           AND shadow.language = i.language
                           AND shadow.parent_symbol_id IS NULL
                           AND replace(shadow.path, '\\', '/') = replace(i.path, '\\', '/')
                     )
                     SELECT 1 FROM visible_types nearest
                     WHERE nearest.symbol_id != selected_target.symbol_id
                       AND nearest.depth = (SELECT MIN(depth) FROM visible_types)
                 )"
    );
    let receiver_match = receiver_match.replace("?3", "?1");
    let language_match = identifier_language_matches("i", "selected_target");
    let receiver_sql = format!(
                "SELECT {receiver_match}
                 FROM (SELECT name, kind, path, start_line, start_column,
                              containing_symbol_id AS from_symbol_id,
                              CASE WHEN json_valid(metadata_json) THEN metadata_json ELSE '{{}}' END AS metadata_json,
                              COALESCE(json_valid(metadata_json), 0) AS has_metadata,
                              language,
                              CASE WHEN json_valid(metadata_json) THEN json_extract(metadata_json, '$.receiver') END AS target_receiver,
                              '[]' AS target_namespace_json
                       FROM identifiers WHERE identifier_id = ?2) p
                 LEFT JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id"
            );
    let mut receiver_stmt = conn.prepare(&receiver_sql)?;
    let ident_sql = format!(
                "WITH raw AS MATERIALIZED (
                     SELECT identifier_id, name, kind, path, start_line, start_column,
                            containing_symbol_id, language,
                            CASE WHEN json_valid(metadata_json) THEN metadata_json ELSE '{{}}' END AS metadata_json
                     FROM identifiers WHERE name = ?1 AND kind IN ('type_usage', 'member_access')
                 )
                 SELECT i.identifier_id,
                        COALESCE(s_from.name, ''),
                        COALESCE(i.containing_symbol_id, ''),
                        i.name,
                        CASE WHEN json_valid(i.metadata_json)
                                  AND json_extract(i.metadata_json, '$.role') = 'signal_handler'
                             THEN CASE WHEN json_extract(i.metadata_json, '$.receiver') = (
                                      SELECT CASE WHEN t.kind IN ('class', 'struct', 'enum', 'interface', 'trait', 'module', 'namespace')
                                                  THEN t.name ELSE tp.name END
                                      FROM symbols t
                                      LEFT JOIN symbols tp ON tp.symbol_id = t.parent_symbol_id
                                      WHERE t.symbol_id = ?2)
                                  THEN 'handler' ELSE 'handler (candidate)' END
                             ELSE i.kind END,
                        i.path,
                        i.start_line,
                        i.start_column
                 FROM raw i
                 LEFT JOIN symbols s_from ON i.containing_symbol_id = s_from.symbol_id
                 LEFT JOIN symbols selected_target ON selected_target.symbol_id = ?2
                 WHERE i.name = ?1 AND i.kind IN ('type_usage', 'member_access')
                   AND COALESCE(s_from.kind, '') != 'import'
                   AND (?2 IS NULL OR {language_match})
                   AND NOT ({type_usage_shadow})
                   AND NOT EXISTS (
                       SELECT 1 FROM relationships covered
                       JOIN symbols covered_to ON covered.to_symbol_id = covered_to.symbol_id
                       WHERE covered_to.name = i.name
                         AND covered.path = i.path
                         AND covered.start_line = i.start_line
                         AND (?2 IS NULL OR covered.to_symbol_id = ?2)
                   )
                   AND NOT EXISTS (
                       SELECT 1 FROM pending_relationships covered
                       WHERE covered.target_terminal_name = i.name
                         AND covered.path = i.path
                         AND covered.start_line = i.start_line
                   )
                   AND (?2 IS NULL OR NOT EXISTS (
                       SELECT 1 FROM symbols owner
                       JOIN symbols member ON member.parent_symbol_id = owner.symbol_id
                       WHERE owner.name = CASE WHEN json_valid(i.metadata_json) THEN json_extract(i.metadata_json, '$.receiver') END
                         AND member.name = i.name
                         AND owner.name IS NOT (SELECT parent.name FROM symbols target
                                                JOIN symbols parent ON parent.symbol_id = target.parent_symbol_id
                                                WHERE target.symbol_id = ?2)
                   ))
                   AND NOT (i.kind = 'member_access' AND EXISTS (
                       SELECT 1 FROM symbols target
                       LEFT JOIN symbols scope ON scope.symbol_id = target.parent_symbol_id
                       WHERE target.symbol_id = ?2
                         AND (scope.kind IN ('function', 'method', 'constructor')
                              OR (scope.symbol_id IS NULL
                                  AND target.path != i.path
                                  AND target.language IN ({JS_FAMILY_LANGUAGES_SQL})
                                  AND COALESCE(target.visibility, 'private') = 'private'))
                   ))
                   AND NOT COALESCE((i.kind = 'member_access'
                        AND json_valid(i.metadata_json)
                        AND json_extract(i.metadata_json, '$.role') IS NOT 'signal_handler'
                        AND json_extract(i.metadata_json, '$.receiver') GLOB '[A-Z]*'
                        AND NOT EXISTS (SELECT 1 FROM symbols known
                                        WHERE known.name = json_extract(i.metadata_json, '$.receiver')
                                          AND known.kind NOT IN ('variable', 'parameter', 'method'))), 0)
                 ORDER BY i.path, i.start_line, i.start_column, i.identifier_id"
            );
    let mut ident_stmt = conn.prepare(&ident_sql)?;
    let mut supported = Vec::new();
    let mut candidates = Vec::new();
    let mut supported_keys = HashSet::new();
    let mut candidate_keys = HashSet::new();
    let site_key = |site: &ReferenceSite| {
        (
            site.from_symbol_id.clone(),
            site.to_symbol_name.clone(),
            site.kind.clone(),
            site.path.clone(),
            site.start_line,
        )
    };
    let mut ident_rows = ident_stmt.query(params![symbol_name, symbol_id])?;
    while let Some(row) = ident_rows.next()? {
        let identifier_id: String = row.get(0)?;
        let mut site = ReferenceSite {
            from_symbol_name: row.get(1)?,
            from_symbol_id: row.get(2)?,
            to_symbol_name: row.get(3)?,
            kind: row.get(4)?,
            path: row.get::<_, String>(5)?.replace('\\', "/"),
            start_line: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
            start_column: row.get::<_, Option<i64>>(7)?.map(|v| v as usize),
            occurrences: None,
            target: None,
            enclosing_symbol_name: None,
        };
        let receiver_match: i64 =
            receiver_stmt.query_row(params![symbol_id, identifier_id], |row| row.get(0))?;
        if receiver_match < 0 {
            continue;
        }
        if receiver_match == 0 && site.kind == "member_access" {
            site.kind = "member_access (candidate)".to_string();
        }
        if site.kind == "member_access (candidate)" {
            if candidates.len() < remaining && candidate_keys.insert(site_key(&site)) {
                candidates.push(DirectIdentifier {
                    identifier_id,
                    site,
                });
            }
        } else if supported_keys.insert(site_key(&site)) {
            supported.push(DirectIdentifier {
                identifier_id,
                site,
            });
            if supported.len() >= remaining {
                break;
            }
        }
    }

    let site_order = |a: &DirectIdentifier, b: &DirectIdentifier| {
        a.site
            .path
            .cmp(&b.site.path)
            .then(a.site.start_line.cmp(&b.site.start_line))
            .then(a.site.start_column.cmp(&b.site.start_column))
    };
    supported.sort_by(site_order);
    supported.truncate(remaining);
    candidates.sort_by(site_order);
    candidates.truncate(remaining - supported.len());
    let mut rows = supported;
    rows.extend(candidates);
    Ok(rows)
}

/// Member accesses whose receiver names the type `symbol_id`, one row per file with an
/// `occurrences` count, at most `remaining`.
pub(crate) fn grouped_member_access_rows(
    conn: &Connection,
    symbol_id: &str,
    remaining: usize,
) -> Result<Vec<ReferenceSite>, QueryError> {
    let language_match = identifier_language_matches("i", "target");
    let mut receiver_stmt = conn.prepare(&format!(
                    "SELECT COALESCE(s.name, ''),
                            COALESCE(i.containing_symbol_id, ''),
                            i.name,
                            i.kind,
                            i.path,
                            MIN(i.start_line),
                            i.start_column,
                            COUNT(*)
                     FROM identifiers i
                     LEFT JOIN symbols s ON i.containing_symbol_id = s.symbol_id
                     JOIN symbols target ON target.symbol_id = ?2
                     LEFT JOIN symbols target_parent ON target_parent.symbol_id = target.parent_symbol_id
                     WHERE i.kind = 'member_access'
                       AND i.name != target.name
                       AND COALESCE(s.kind, '') != 'import'
                       AND target.kind IN ('class', 'struct', 'enum', 'interface', 'trait', 'module', 'namespace')
                       AND json_valid(i.metadata_json)
                       AND json_extract(i.metadata_json, '$.receiver') = target.name
                       AND (json_extract(i.metadata_json, '$.receiver_qualifier') IS NULL
                            OR json_extract(i.metadata_json, '$.receiver_qualifier') = target_parent.name)
                       AND {language_match}
                       AND NOT EXISTS (
                           SELECT 1 FROM symbols nearer
                           WHERE nearer.name = target.name
                             AND nearer.path = i.path
                             AND nearer.symbol_id != target.symbol_id
                             AND nearer.kind IN ({NESTED_TYPE_KINDS}, 'module', 'namespace')
                       )
                     GROUP BY i.path
                     ORDER BY i.path, i.start_line
                     LIMIT ?1"
                ))?;
    let rows = receiver_stmt.query_map(params![remaining as i64, symbol_id], |row| {
        Ok(ReferenceSite {
            from_symbol_name: row.get(0)?,
            from_symbol_id: row.get(1)?,
            to_symbol_name: row.get(2)?,
            kind: row.get(3)?,
            path: row.get::<_, String>(4)?.replace('\\', "/"),
            start_line: row.get::<_, Option<i64>>(5)?.map(|v| v as usize),
            start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
            occurrences: Some(row.get::<_, i64>(7)? as usize),
            target: None,
            enclosing_symbol_name: None,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn find_direct_references(
    conn: &Connection,
    symbol_name: &str,
    limit: usize,
    symbol_id: Option<&str>,
) -> Result<Vec<ReferenceSite>, QueryError> {
    let mut results = Vec::new();

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
            occurrences: None,
            target: None,
            enclosing_symbol_name: None,
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
                let pred =
                    if pending_receiver_chain_requires_full_for_target_name(conn, symbol_name) {
                        pending_target_predicate(conn, "s_target", "s_target_parent")
                    } else {
                        pending_target_predicate_without_typed_receiver_chain(
                            conn,
                            "s_target",
                            "s_target_parent",
                        )
                    };
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
                      LIMIT ?2"),
                )?;

                let p_rows =
                    pending_stmt.query_map(params![symbol_name, remaining as i64, sid], |row| {
                        Ok(ReferenceSite {
                            from_symbol_name: row.get(0)?,
                            from_symbol_id: row.get(1)?,
                            to_symbol_name: row.get(2)?,
                            kind: row.get(3)?,
                            path: row.get::<_, String>(4)?.replace('\\', "/"),
                            start_line: Some(row.get::<_, i64>(5)? as usize),
                            start_column: row.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                            occurrences: None,
                            target: None,
                            enclosing_symbol_name: None,
                        })
                    })?;
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
                            occurrences: None,
                            target: None,
                            enclosing_symbol_name: None,
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
                            occurrences: None,
                            target: None,
                            enclosing_symbol_name: None,
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
        let identifiers = direct_identifier_rows(conn, symbol_name, symbol_id, remaining)?;
        results.extend(identifiers.into_iter().map(|row| row.site));
        if let Some(sid) = symbol_id.filter(|_| results.len() < limit) {
            let remaining = limit - results.len();
            results.extend(grouped_member_access_rows(conn, sid, remaining)?);
        }
    }

    Ok(merge_same_site(results))
}

/// A pending call reaches a definition, or a class attribute that holds a callable:
/// `self.should_ignore_error(error)` with `should_ignore_error: None = None` in the class.
fn callee_target_kind(parent: &str) -> String {
    format!(
        "(s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
         OR p.kind = 'calls' AND s_to.kind IN ('variable', 'field')
            AND {parent}.kind IN ('class', 'struct', 'interface', 'trait', 'record', 'object', 'protocol'))"
    )
}

/// Names the definitions each callee site can reach: the resolved target, or the definitions a
/// pending call matches. Several matches are listed, so a call to a common name stays clear.
fn describe_callee_targets(
    conn: &Connection,
    sites: &mut [ReferenceSite],
) -> Result<(), QueryError> {
    let owner = "CASE WHEN par.kind IN ('class', 'struct', 'interface', 'trait', 'enum', 'record', 'object', 'protocol', 'union')
                 THEN par.name || '.' ELSE '' END";
    let pending_language = pending_relationship_language_filter("p.kind", "s_from", "s_to");
    let has_pending_namespaces = has_pending_namespace_column(conn);
    let pending_predicate = if has_pending_namespaces {
        let mut probed_names = HashSet::new();
        let pending_receiver_chain = sites.iter().any(|site| {
            if site.to_symbol_name.contains(['.', ':']) {
                return true;
            }
            probed_names.insert(site.to_symbol_name.as_str())
                && pending_receiver_chain_requires_full_for_target_name(conn, &site.to_symbol_name)
        });
        if pending_receiver_chain {
            pending_target_predicate(conn, "s_to", "s_to_parent")
        } else {
            pending_target_predicate_without_typed_receiver_chain(conn, "s_to", "s_to_parent")
        }
    } else {
        String::new()
    };
    let pending = if has_pending_namespaces {
        format!(
            "UNION
             SELECT {owner} || s_to.name, replace(s_to.path, '\\', '/'), s_to.start_line
             FROM pending_relationships p
             JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
             JOIN symbols s_to ON s_to.name = p.target_terminal_name
             LEFT JOIN symbols par ON s_to.parent_symbol_id = par.symbol_id
             LEFT JOIN symbols s_to_parent ON s_to.parent_symbol_id = s_to_parent.symbol_id
             WHERE p.from_symbol_id = ?1 AND p.start_line = ?2
               AND (p.target_terminal_name = ?3 OR p.target_display_name = ?3)
               AND {pending_language}
               AND {kind}
               AND {pred}",
            kind = callee_target_kind("s_to_parent"),
            pred = pending_predicate
        )
    } else {
        String::new()
    };
    let relationship_call_language =
        call_languages_compatible_sql("s_from.language", "s_to.language");
    let mut stmt = conn.prepare(&format!(
        "SELECT {owner} || s_to.name, replace(s_to.path, '\\', '/'), s_to.start_line
         FROM relationships r
         JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
         JOIN symbols s_to ON r.to_symbol_id = s_to.symbol_id
         LEFT JOIN symbols par ON s_to.parent_symbol_id = par.symbol_id
         WHERE r.from_symbol_id = ?1 AND r.start_line = ?2 AND s_to.name = ?3
           AND (r.kind != 'calls' OR {relationship_call_language})
         {pending}
         ORDER BY 2, 3
         LIMIT 4"
    ))?;
    for site in sites.iter_mut() {
        let Some(line) = site.start_line else {
            continue;
        };
        let targets = stmt
            .query_map(
                params![site.from_symbol_id, line as i64, site.to_symbol_name],
                |row| {
                    Ok(format!(
                        "`{}` ({}:{})",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?
                    ))
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        site.target = match targets.len() {
            0 => None,
            1 => targets.into_iter().next(),
            2 | 3 => Some(format!("one of {}", targets.join(", "))),
            _ => Some(format!(
                "more than 3 definitions named `{}`",
                site.to_symbol_name
            )),
        };
    }
    Ok(())
}

/// The import statements of the top-level definition `name` (under `path_filter` when given): an
/// import row with that name whose source module names a folder or file on the definition's path.
/// A row that binds no name ([`import_binds_name_sql`]) imports none, and a Java import line is
/// read from its signature by the edit check.
/// A relative JavaScript or TypeScript source (`./helpers/parseUtil`) must name the definition's
/// file, so a mirror copy elsewhere (`deno/lib/helpers/parseUtil.ts`) does not count.
/// `find_references` lists calls and type uses, so these go in a summary line after them.
pub fn import_sites(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
) -> Result<Vec<(String, usize)>, QueryError> {
    import_sites_from(conn, name, path_filter, false)
}

/// The import statements of the top-level name `name` that the index no longer defines in the file
/// `path`: the same rows as [`import_sites`], for a definition that an edit removed or renamed.
pub fn import_sites_of_absent(
    conn: &Connection,
    name: &str,
    path: &str,
    member: bool,
) -> Result<Vec<(String, usize)>, QueryError> {
    let mut sites = Vec::new();
    if !member {
        sites.extend(import_sites_from(conn, name, Some(path), true)?);
        sites.extend(rust_use_lines(conn, name, path)?);
    }
    sites.extend(java_import_lines(conn, name, path, member)?);
    sites.sort();
    sites.dedup();
    Ok(sites)
}

/// The Rust `use` lines of `name` whose last module segment is the stem of the file `path` or the
/// folder of its `mod.rs`. A Rust import row carries no source, so the pending `imports` rows give
/// the module.
/// The Java import lines that name `name` of the class file `path`. For the class itself:
/// `import p.G;`, and `import static p.G.m;` or `import p.G.Inner;` of its members. For a member:
/// `import static p.G.name;` or `import p.G.name;`. julie's `source` names only the package or
/// the class of a static import, so the path is read from the signature.
fn java_import_lines(
    conn: &Connection,
    name: &str,
    path: &str,
    member: bool,
) -> Result<Vec<(String, usize)>, QueryError> {
    let normalized = format!("/{}", path.replace('\\', "/"));
    let Some(class_file) = normalized.strip_suffix(".java") else {
        return Ok(Vec::new());
    };
    let class = class_file.rsplit('/').next().unwrap_or_default();
    let names_file = |dotted: &str| class_file.ends_with(&format!("/{}", dotted.replace('.', "/")));
    let mut stmt = conn.prepare(
        "SELECT replace(path, '\\', '/'), start_line, COALESCE(signature, '') FROM symbols
         WHERE kind = 'import' AND language = 'java'
           AND (name = ?1 OR ?2 AND signature LIKE '%.' || ?3 || '.%')",
    )?;
    let rows = stmt.query_map(params![name, !member, class], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)? as usize,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut lines = Vec::new();
    for row in rows {
        let (import_path, line, signature) = row?;
        let imported = signature
            .trim()
            .trim_start_matches("import ")
            .trim_start()
            .trim_start_matches("static ")
            .trim()
            .trim_end_matches(';')
            .trim();
        let (owner, last) = imported.rsplit_once('.').unwrap_or(("", imported));
        let named = if member {
            last == name && names_file(owner)
        } else {
            names_file(imported) || names_file(owner)
        };
        if named {
            lines.push((import_path, line));
        }
    }
    Ok(lines)
}

fn rust_use_lines(
    conn: &Connection,
    name: &str,
    path: &str,
) -> Result<Vec<(String, usize)>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT replace(pr.path, '\\', '/'), pr.start_line
         FROM pending_relationships pr
         JOIN files f ON f.file_id = pr.file_id AND f.language = 'rust'
         WHERE pr.kind = 'imports' AND pr.target_terminal_name = ?1
           AND pr.start_line IS NOT NULL AND json_valid(pr.target_namespace_json)
           AND json_extract(pr.target_namespace_json, '$[#-1]') NOT IN ('crate', 'self', 'super')
           AND (?2 GLOB '*/' || json_extract(pr.target_namespace_json, '$[#-1]') || '.rs'
                OR ?2 GLOB '*/' || json_extract(pr.target_namespace_json, '$[#-1]') || '/mod.rs')",
    )?;
    let target_path = format!("/{}", path.replace('\\', "/"));
    let rows = stmt.query_map(params![name, target_path], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn import_sites_from(
    conn: &Connection,
    name: &str,
    path_filter: Option<&str>,
    absent: bool,
) -> Result<Vec<(String, usize)>, QueryError> {
    if !has_column(conn, "symbols", "metadata_json") {
        return Ok(Vec::new());
    }
    let targets = if absent {
        "(SELECT ?1 AS name, ?2 AS path, NULL AS parent_symbol_id, 'function' AS kind)"
    } else {
        "symbols"
    };
    // `./util.ts` (Deno, and TypeScript's `allowImportingTsExtensions`) names the module `util`.
    let extensions: String = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"]
        .iter()
        .map(|ext| {
            format!(
                " WHEN module_path.value LIKE '%.{ext}'
                  THEN substr(module_path.value, 1, length(module_path.value) - {})",
                ext.len() + 1
            )
        })
        .collect();
    let source = format!(
        "(CASE WHEN i.language IN ({JS_FAMILY_LANGUAGES_SQL}) THEN
             CASE{extensions} ELSE module_path.value END
           ELSE module_path.value END)"
    );
    let dotted = format!("replace({source}, '/', '.')");
    let segment = format!("replace({dotted}, rtrim({dotted}, replace({dotted}, '.', '')), '')");
    let target_path = "('/' || replace(target.path, '\\', '/'))";
    // `from . import Flask`: a source of dots only names the importing file's package.
    let dir = |path: &str| {
        format!("rtrim(replace({path}, '\\', '/'), replace(replace({path}, '\\', '/'), '/', ''))")
    };
    let (import_dir, target_dir) = (dir("i.path"), dir("target.path"));
    let relative = is_relative_import_path("module_path.value");
    let relative_match =
        relative_import_matches("module_path.value", "target").replace("p.path", "i.path");
    let binds = import_binds_name_sql("i");
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT replace(i.path, '\\', '/'), i.start_line
         FROM {targets} target
         JOIN symbols i ON i.name = target.name AND i.kind = 'import'
         CROSS JOIN json_each(json_array(json_extract(i.metadata_json, '$.source'))) module_path
         WHERE target.name = ?1
           AND target.parent_symbol_id IS NULL
           AND target.kind NOT IN ({LOW_SIGNAL_KINDS_SQL})
           AND (?2 IS NULL OR replace(target.path, '\\', '/') = ?2
                OR replace(target.path, '\\', '/') LIKE ?3 ESCAPE '\\')
           AND json_valid(i.metadata_json)
           AND i.language NOT IN ({docs})
           AND i.language != 'java'
           AND {binds}
           AND (
               {segment} != ''
               AND ({target_path} GLOB '*/' || {segment} || '/*'
                    OR {target_path} GLOB '*/' || {segment} || '.*')
               OR module_path.value != '' AND replace(module_path.value, '.', '') = ''
               AND substr({import_dir}, 1, length({target_dir})) = {target_dir}
           )
           AND NOT (i.language IN ({JS_FAMILY_LANGUAGES_SQL}) AND {relative} AND NOT {relative_match})
         ORDER BY 1, 2",
        docs = documentation_language_list()
    ))?;
    let norm = path_filter.map(|p| p.replace('\\', "/").trim_matches('/').to_string());
    let prefix = norm.as_ref().map(|p| format!("{}/%", escape_like(p)));
    let rows = stmt.query_map(params![name, norm, prefix], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// One row for each caller, target, kind, and line: `app.wsgi_app = Wrap(app.wsgi_app)` is one
/// site, not two identical rows.
fn merge_same_site(sites: Vec<ReferenceSite>) -> Vec<ReferenceSite> {
    let mut merged: Vec<ReferenceSite> = Vec::with_capacity(sites.len());
    let mut index = HashMap::new();
    for site in sites {
        let key = (
            site.from_symbol_id.clone(),
            site.to_symbol_name.clone(),
            site.kind.clone(),
            site.path.clone(),
            site.start_line,
        );
        match index.get(&key) {
            Some(&at) => {
                let kept: &mut ReferenceSite = &mut merged[at];
                kept.start_column = kept.start_column.min(site.start_column);
            }
            None => {
                index.insert(key, merged.len());
                merged.push(site);
            }
        }
    }
    merged
}

/// Resolve callee signatures directly in a single joined query, avoiding N+1 queries
/// and preserving ambiguous methods across types. Prioritizes functions/methods over enum variants.
/// The kinds whose `type_facts` row is the type they return.
const RETURNING_KINDS: &[&str] = &["function", "method", "constructor"];

/// The one type that `type_facts` records for the row `alias`, or NULL for none, for two, or for
/// an index without the table.
fn recorded_type_sql(conn: &Connection, alias: &str) -> String {
    if !has_table(conn, "type_facts")
        || !has_column(conn, "type_facts", "symbol_id")
        || !has_column(conn, "type_facts", "resolved_type")
    {
        return "NULL".to_string();
    }
    format!(
        "(SELECT MIN(tf.resolved_type) FROM type_facts tf WHERE tf.symbol_id = {alias}.symbol_id
          HAVING COUNT(DISTINCT tf.resolved_type) = 1)"
    )
}

/// `return_type` when a symbol of `kind` returns it and `signature` does not spell it as a whole
/// word outside its first parameter list, as `fn new() -> Self` does not spell `Builder`.
pub(crate) fn unspelled_return_type(
    kind: &str,
    signature: Option<&str>,
    return_type: Option<String>,
) -> Option<String> {
    let return_type = return_type.filter(|ty| !ty.trim().is_empty())?;
    if !RETURNING_KINDS.contains(&kind) {
        return None;
    }
    let signature = signature.unwrap_or_default();
    let outside = match signature.find('(') {
        Some(open) => {
            let mut depth = 0usize;
            let mut close = signature.len();
            for (at, c) in signature[open..].char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            close = open + at + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            format!("{} {}", &signature[..open], &signature[close..])
        }
        None => signature.to_string(),
    };
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let spelled = outside.match_indices(return_type.as_str()).any(|(at, _)| {
        !outside[..at].chars().next_back().is_some_and(is_word)
            && !outside[at + return_type.len()..]
                .chars()
                .next()
                .is_some_and(is_word)
    });
    (!spelled).then_some(return_type)
}

/// The return type of `symbol` from `type_facts` when its signature does not spell it.
pub fn find_unspelled_return_type(
    conn: &Connection,
    symbol: &Symbol,
) -> Result<Option<String>, QueryError> {
    if !RETURNING_KINDS.contains(&symbol.kind.as_str()) {
        return Ok(None);
    }
    let sql = recorded_type_sql(conn, "s");
    let recorded: Option<String> = conn
        .query_row(
            &format!("SELECT {sql} FROM symbols s WHERE s.symbol_id = ?1"),
            params![symbol.symbol_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    Ok(unspelled_return_type(
        &symbol.kind,
        symbol.signature.as_deref(),
        recorded,
    ))
}

/// A callee row: name, signature, path, line, kind, and the one recorded type.
type CalleeRow = (
    String,
    Option<String>,
    String,
    usize,
    String,
    Option<String>,
);

/// One callee line: the signature, the return type it does not spell, and the location.
fn callee_entry(
    signature: &str,
    kind: &str,
    return_type: Option<String>,
    path: &str,
    line: usize,
) -> String {
    match unspelled_return_type(kind, Some(signature), return_type) {
        Some(return_type) => format!("{signature}; Returns: `{return_type}` ({path}:{line})"),
        None => format!("{signature} ({path}:{line})"),
    }
}

pub fn find_callee_signatures(
    conn: &Connection,
    symbol_name: &str,
    symbol_id: &str,
    limit: usize,
    include_external: bool,
) -> Result<Vec<String>, QueryError> {
    let relationship_call_language =
        call_languages_compatible_sql("s_from.language", "s_to.language");
    let return_type = recorded_type_sql(conn, "s_to");
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT s_to.name, {CALLEE_SIGNATURE_SQL}, s_to.path, s_to.start_line, s_to.kind, {return_type}
         FROM relationships r
         JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
         JOIN symbols s_to ON r.to_symbol_id = s_to.symbol_id
         WHERE s_from.name = ?1 AND r.from_symbol_id = ?2
           AND (r.kind != 'calls' OR {relationship_call_language})
         LIMIT ?3"
    ))?;

    let rows = stmt.query_map(params![symbol_name, symbol_id, (limit * 2) as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, String>(2)?.replace('\\', "/"),
            row.get::<_, Option<i64>>(3)?.unwrap_or(1) as usize,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
        ))
    })?;

    let mut signatures = Vec::new();
    let mut variants = Vec::new();

    for r in rows.flatten() {
        let (name, sig_opt, path, line, kind, return_type) = r;
        let sig = sig_opt.unwrap_or(name);
        let entry = callee_entry(&sig, &kind, return_type, &path, line);
        if kind == "variant" {
            if !variants.contains(&entry) {
                variants.push(entry);
            }
        } else if !signatures.contains(&entry) {
            signatures.push(entry);
        }
    }

    let mut owners = None;
    let pending_predicate = if has_pending_namespace_column(conn) {
        let pending_receiver_chain = signatures.len() < limit
            && pending_receiver_chain_requires_full_for_source(conn, symbol_id);
        if pending_receiver_chain {
            if has_typed_receiver_chain_support(conn) {
                owners = Some(receiver_chain_owner_map(
                    conn,
                    "",
                    "",
                    "s_from.name = ?1 AND p.from_symbol_id = ?2",
                    params![symbol_name, symbol_id],
                )?);
            }
            pending_target_predicate_with_receiver_chain_owner(
                conn,
                "s_to",
                "s_parent",
                &bound_receiver_chain_owner_sql("?4"),
            )
        } else {
            pending_target_predicate_without_typed_receiver_chain(conn, "s_to", "s_parent")
        }
    } else {
        String::new()
    };
    if signatures.len() < limit {
        let remaining = (limit - signatures.len()) * 2;
        let pending_language = pending_relationship_language_filter("p.kind", "s_from", "s_to");
        let p_rows: Vec<CalleeRow> = if has_pending_namespace_column(conn) {
            let pending_sql = format!("SELECT DISTINCT s_to.name, {CALLEE_SIGNATURE_SQL}, s_to.path, s_to.start_line, s_to.kind, {return_type}
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 JOIN symbols s_to ON s_to.name = p.target_terminal_name
                 LEFT JOIN symbols s_parent ON s_to.parent_symbol_id = s_parent.symbol_id
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND {pending_language}
                   AND {kind}
                    AND {pred}
                 LIMIT ?3", kind = callee_target_kind("s_parent"), pred = pending_predicate, pending_language = pending_language, return_type = return_type);
            let mut p_stmt = conn.prepare(&pending_sql)?;
            let map_row = |row: &rusqlite::Row<'_>| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?.replace('\\', "/"),
                    row.get::<_, Option<i64>>(3)?.unwrap_or(1) as usize,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            };
            let rows = match &owners {
                Some(owners) => p_stmt.query_map(
                    params![symbol_name, symbol_id, remaining as i64, owners],
                    map_row,
                )?,
                None => {
                    p_stmt.query_map(params![symbol_name, symbol_id, remaining as i64], map_row)?
                }
            };
            rows.flatten().collect()
        } else {
            let mut p_stmt = conn.prepare(&format!(
                "SELECT DISTINCT s_to.name, {CALLEE_SIGNATURE_SQL}, s_to.path, s_to.start_line, s_to.kind, {return_type}
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 JOIN symbols s_to ON s_to.name = p.target_terminal_name
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND {pending_language}
                   AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                 LIMIT ?3"
                ))?;

            let rows =
                p_stmt.query_map(params![symbol_name, symbol_id, remaining as i64], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?.replace('\\', "/"),
                        row.get::<_, Option<i64>>(3)?.unwrap_or(1) as usize,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                })?;
            rows.flatten().collect()
        };

        for r in p_rows {
            let (name, sig_opt, path, line, kind, return_type) = r;
            let sig = sig_opt.unwrap_or(name);
            let entry = callee_entry(&sig, &kind, return_type, &path, line);
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
        let pending_language = pending_relationship_language_filter("p.kind", "s_from", "s_to");
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
                         AND {pending_language}
                         AND {kind}
                         AND {pred}
                   )
                 LIMIT ?3", kind = callee_target_kind("s_parent"), pred = pending_predicate, pending_language = pending_language),
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
            let mut ext_stmt = conn.prepare(&format!(
                "SELECT DISTINCT p.target_terminal_name, p.path, p.start_line
                 FROM pending_relationships p
                 JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
                 WHERE s_from.name = ?1 AND p.from_symbol_id = ?2
                   AND NOT EXISTS (
                       SELECT 1 FROM symbols s_to
                       WHERE s_to.name = p.target_terminal_name
                         AND {pending_language}
                         AND s_to.kind NOT IN ('import', 'variable', 'parameter', 'field', 'property', 'module', 'namespace')
                   )
                 LIMIT ?3"
            ))?;

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

pub(crate) const ROUTE_FAMILIES: &[&str] = &[
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

const PRAGMA_FAMILIES: &[&str] = &[".pragma.", "javascript.qml_directive.v1"];

const PROPERTY_FAMILIES: &[&str] = &[".property_declaration.", ".qt_property."];

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
    ("property", PROPERTY_FAMILIES),
    ("properties", PROPERTY_FAMILIES),
];

fn category_families(category: &str) -> Option<&'static [&'static str]> {
    let wanted = category.trim().to_ascii_lowercase();
    CATEGORY_ALIASES
        .iter()
        .find(|(alias, _)| *alias == wanted)
        .map(|(_, families)| *families)
}

pub(crate) fn matches_family(pattern_id: &str, rule: &str) -> bool {
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
                    CASE WHEN json_extract(sf.metadata_json, '$.normalized_route_template') IS NOT NULL
                         THEN COALESCE(json_extract(sf.metadata_json, '$.verb') || ' ', '')
                              || json_extract(sf.metadata_json, '$.normalized_route_template') END,
                    CASE WHEN sf.pattern_id = 'cpp.qt_property.v1'
                         THEN json_extract(sf.metadata_json, '$.name') END
                ) AS display_key,
                sf.metadata_json
         FROM structural_facts sf
         LEFT JOIN symbols s ON sf.containing_symbol_id = s.symbol_id
         WHERE (:cat IS NOT NULL AND {cat_clause})
           AND (:path IS NULL OR replace(sf.path, '\\', '/') = :path COLLATE NOCASE OR replace(sf.path, '\\', '/') LIKE :dir_prefix ESCAPE '\\')
         ORDER BY sf.path ASC, sf.start_line ASC,
                  COALESCE(
                      json_extract(sf.metadata_json, '$.effective_route_template'),
                      json_extract(sf.metadata_json, '$.route_template'),
                      json_extract(sf.metadata_json, '$.normalized_route_template'),
                      ''
                  ) ASC,
                  COALESCE(json_extract(sf.metadata_json, '$.verb'), '') ASC,
                  sf.structural_fact_id ASC
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
                metadata: row
                    .get::<_, Option<String>>(11)?
                    .map(|json| serde_json::from_str(&json))
                    .transpose()
                    .map_err(|err| {
                        rusqlite::Error::FromSqlConversionFailure(
                            11,
                            rusqlite::types::Type::Text,
                            Box::new(err),
                        )
                    })?,
                containing_symbol_name: row.get(6)?,
                start_line: row.get::<_, i64>(7)? as usize,
                end_line: row.get::<_, i64>(8)? as usize,
                confidence: row.get(9)?,
            })
        },
    )?;

    let mut results = Vec::new();
    for r in rows {
        let mut fact = r?;
        let display = fact.metadata.as_ref().and_then(|metadata| {
            route_display(metadata).or_else(|| mount_display(&fact.capture_name, metadata))
        });
        if let Some(display) = display {
            fact.key = Some(display);
        }
        results.push(fact);
    }
    Ok(results)
}

/// A route as its source spells it, with its verb: `GET /<int:id>/update`, not the normalized
/// `/:id/update`. The prefixed source template is used when the extractor joined a prefix;
/// otherwise the raw template, unless the normalized one holds a prefix the raw one lacks.
fn route_display(metadata: &serde_json::Value) -> Option<String> {
    let text = |key: &str| metadata.get(key).and_then(|value| value.as_str());
    let normalized = text("normalized_route_template")?;
    let template = match (text("effective_route_template"), text("route_template")) {
        (Some(effective), _) => effective,
        (None, Some(raw)) if route_shape(raw) == route_shape(normalized) => raw,
        _ => normalized,
    };
    Some(match text("verb") {
        Some(verb) => format!("{verb} {template}"),
        None => template.to_string(),
    })
}

/// A router or blueprint mount with what it mounts and where: ``blueprint_registration `auth.bp` ``,
/// ``router_mount `router` at /api``. The path includes any enclosing scope the extractor joined.
fn mount_display(capture: &str, metadata: &serde_json::Value) -> Option<String> {
    let text = |key: &str| metadata.get(key).and_then(|value| value.as_str());
    if text("query_family") != Some("framework") {
        return None;
    }
    let target = text("mount_target")?;
    Some(
        match text("normalized_mount_path").or_else(|| text("mount_path")) {
            Some(path) => format!("{capture} `{target}` at {path}"),
            None => format!("{capture} `{target}`"),
        },
    )
}

/// A route template with every parameter (`<int:id>`, `{id}`, `:id`) replaced by `*`.
fn route_shape(template: &str) -> String {
    let mut shape = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '<' | '{' => {
                let close = if c == '<' { '>' } else { '}' };
                for inner in chars.by_ref() {
                    if inner == close {
                        break;
                    }
                }
                shape.push('*');
            }
            ':' => {
                while chars.peek().is_some_and(|next| *next != '/') {
                    chars.next();
                }
                shape.push('*');
            }
            _ => shape.push(c),
        }
    }
    shape
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

const RELATED_TYPE_KINDS: &str = "'struct', 'enum', 'trait', 'class', 'interface', 'type', 'union'";

fn java_import_path_sql(alias: &str) -> String {
    format!(
        "CASE WHEN {alias}.signature LIKE 'import %' THEN trim(substr({alias}.signature, 8))
              ELSE {alias}.name END"
    )
}

fn go_import_path_sql(alias: &str) -> String {
    let import = format!("trim(substr({alias}.signature, 8))");
    format!(
        "CASE
            WHEN {alias}.signature LIKE 'import %' AND {import} LIKE '\"%\"'
                THEN substr({import}, 2, length({import}) - 2)
            WHEN {alias}.signature LIKE 'import %' AND instr({import}, ' ') > 0
                THEN substr({import}, instr({import}, ' ') + 2,
                            length({import}) - instr({import}, ' ') - 2)
         END"
    )
}

fn go_path_directory_sql(path: &str) -> String {
    format!("rtrim({path}, replace({path}, '/', ''))")
}

fn go_import_owner_sql(
    target: &str,
    qualified_type: &str,
    source_path: &str,
    source_language: &str,
) -> String {
    let qualifier = format!("substr({qualified_type}, 1, instr({qualified_type}, '.') - 1)");
    let type_name = format!("substr({qualified_type}, instr({qualified_type}, '.') + 1)");
    let import_path = go_import_path_sql("type_import");
    let target_dir = go_path_directory_sql(&format!("{target}.path"));
    let parent_dir = "rtrim(substr(go_folder.dir, 1, length(go_folder.dir) - 1), replace(substr(go_folder.dir, 1, length(go_folder.dir) - 1), '/', ''))";
    let relative_dir = format!("rtrim(substr({target_dir}, length(go_module.dir) + 1), '/')");
    format!(
        "EXISTS (
            SELECT 1 FROM symbols type_import
            WHERE type_import.kind = 'import'
              AND type_import.path = {source_path}
              AND {source_language} = 'go'
              AND {target}.language = 'go'
              AND type_import.name = {qualifier}
              AND instr({qualified_type}, '.') > 1
              AND {target}.name = {type_name}
              AND EXISTS (
                  WITH RECURSIVE go_folder(dir, depth) AS (
                          SELECT {target_dir}, 0
                          UNION ALL
                          SELECT {parent_dir}, go_folder.depth + 1
                          FROM go_folder
                          WHERE go_folder.dir != '' AND go_folder.depth < 64
                      ), go_modules(dir, depth, name) AS (
                          SELECT go_folder.dir, go_folder.depth, go_module.name
                          FROM go_folder
                          JOIN symbols go_module
                            ON go_module.path = go_folder.dir || 'go.mod'
                           AND go_module.kind = 'module'
                          WHERE CASE WHEN json_valid(go_module.metadata_json)
                                     THEN json_extract(go_module.metadata_json, '$.directive')
                                END = 'module'
                      ), nearest_module(dir, name) AS (
                          SELECT go_modules.dir, go_modules.name
                          FROM go_modules
                          WHERE go_modules.depth = (SELECT MIN(depth) FROM go_modules)
                  )
                  SELECT 1 FROM nearest_module go_module
                  JOIN symbols go_package
                    ON go_package.path = {target}.path
                   AND go_package.language = 'go'
                   AND go_package.kind = 'module'
                  WHERE (SELECT COUNT(*) FROM nearest_module) = 1
                    AND {import_path} = go_module.name ||
                        CASE WHEN {relative_dir} = '' THEN '' ELSE '/' || {relative_dir} END
              )
        )"
    )
}

fn java_import_owner_sql(
    target: &str,
    binding_name: &str,
    source_path: &str,
    source_language: &str,
) -> String {
    let import_path = java_import_path_sql("type_import");
    format!(
        "EXISTS (
            SELECT 1 FROM symbols type_import
            WHERE type_import.kind = 'import'
              AND type_import.path = {source_path}
              AND {source_language} = 'java'
              AND {target}.language = 'java'
              AND {target}.name = {binding_name}
              AND substr({import_path}, -length({target}.name) - 1) = '.' || {target}.name
              AND EXISTS (
                  SELECT 1 FROM symbols target_scope
                  WHERE target_scope.path = {target}.path
                    AND target_scope.language = {target}.language
                    AND target_scope.kind = 'namespace'
                    AND target_scope.name = substr(
                        {import_path},
                        1,
                        length({import_path}) - length({target}.name) - 1
                    )
              )
        )"
    )
}

fn type_fact_import_owner_sql(
    target: &str,
    binding_name: &str,
    source_path: &str,
    source_language: &str,
) -> String {
    let relative = is_relative_import_path("import_source.value");
    let relative_match = relative_import_matches("import_source.value", target);
    let imported_name = "NULLIF(json_extract(type_import.metadata_json, '$.importedName'), '')";
    let package_kind = format!(
        "CASE {target}.language WHEN 'go' THEN 'module'
             WHEN 'java' THEN 'namespace' WHEN 'csharp' THEN 'namespace' END"
    );
    let indexed_package = format!(
        "EXISTS (
            SELECT 1 FROM symbols target_scope
            WHERE target_scope.path = {target}.path
              AND target_scope.language = {target}.language
              AND target_scope.kind = {package_kind}
              AND target_scope.name = replace(import_source.value, '/', '.')
        )"
    );
    let java_import_owner =
        java_import_owner_sql(target, binding_name, source_path, source_language);
    format!(
        "({java_import_owner} OR EXISTS (
            SELECT 1 FROM symbols type_import
            CROSS JOIN json_each(json_array(
                json_extract(type_import.metadata_json, '$.source')
            )) import_source
            WHERE type_import.kind = 'import'
              AND type_import.path = {source_path}
              AND type_import.name = {binding_name}
              AND json_valid(type_import.metadata_json)
              AND ({imported_name} = {target}.name
                   OR ({imported_name} IS NULL AND {target}.name = {binding_name}))
              AND CASE WHEN {relative} THEN {relative_match}
                  ELSE {indexed_package}
                  END
        ))"
    )
}

fn type_fact_import_binding_sql(source_path: &str, resolved_type: &str, language: &str) -> String {
    let import_path = java_import_path_sql("type_import");
    format!(
        "EXISTS (
            SELECT 1 FROM symbols type_import
            WHERE type_import.path = {source_path}
              AND type_import.kind = 'import'
              AND (type_import.name = {resolved_type}
                   OR CASE WHEN json_valid(type_import.metadata_json)
                           THEN NULLIF(json_extract(type_import.metadata_json, '$.importedName'), '')
                      END = {resolved_type}
                   OR ({language} = 'go'
                       AND instr({resolved_type}, '.') > 1
                       AND type_import.name = substr({resolved_type}, 1, instr({resolved_type}, '.') - 1))
                   OR ({language} = 'java'
                       AND substr({import_path}, -length({resolved_type}) - 1)
                           = '.' || {resolved_type}))
        )"
    )
}

/// A scalar owner id for a type fact, or SQL NULL when the index cannot bind it uniquely.
pub(crate) fn type_fact_owner_id_sql(
    source_symbol_id: &str,
    resolved_type: &str,
    language: &str,
) -> String {
    type_fact_owner_id_in_scope_sql(source_symbol_id, resolved_type, language, None)
}

/// `type_fact_owner_id_sql` with the import scope of the fact, as `(import_files,
/// namespaces)`: `import_files` is a JSON array of `[path, name]` definitions that the file's
/// imports reach, and `namespaces` a JSON array of `[namespace, rank]` pairs that it sees,
/// where a lower rank shadows a higher one. The parts of one type in one namespace (a C#
/// partial class) are one candidate, shown at the file named for the type.
fn type_fact_owner_id_in_scope_sql(
    source_symbol_id: &str,
    resolved_type: &str,
    language: &str,
    scope: Option<(&str, &str)>,
) -> String {
    let import_owner = type_fact_import_owner_sql("d", resolved_type, "p.path", "p.language");
    let go_import_owner = go_import_owner_sql("d", resolved_type, "p.path", "p.language");
    let java_import_owner = java_import_owner_sql("d", resolved_type, "p.path", "p.language");
    let import_bindings = type_fact_import_binding_sql("p.path", resolved_type, "p.language");
    let language_match = identifier_language_matches("p", "d");
    let same_directory = path_proximity("p.path", "d.path");
    let generic_scope_ctes = type_fact_generic_scope_ctes();
    let generic_parameter = type_fact_is_generic_parameter_sql("generic_scopes", resolved_type);
    let module_level = module_level_type_sql("d");
    let (scope_files, scope_namespaces, scope_case) = match scope {
        Some((import_files, namespaces)) => (
            format!(
                "UNION
             SELECT DISTINCT d.symbol_id
             FROM fact_source p
             CROSS JOIN json_each({import_files}) scope_file
             JOIN symbols d
               ON d.path = json_extract(scope_file.value, '$[0]')
              AND d.name = json_extract(scope_file.value, '$[1]')
              AND d.kind IN ({RELATED_TYPE_KINDS})
              AND {module_level}
             WHERE {language_match}"
            ),
            format!(
                ", namespace_candidates(symbol_id, namespace, path, rank) AS (
             SELECT DISTINCT d.symbol_id, namespace_row.name, d.path,
                    json_extract(open_namespace.value, '$[1]')
             FROM fact_source p
             CROSS JOIN symbols d
             CROSS JOIN symbols namespace_row
             CROSS JOIN json_each({namespaces}) open_namespace
             WHERE d.name = {resolved_type}
               AND d.kind IN ({RELATED_TYPE_KINDS})
               AND namespace_row.path = d.path
               AND namespace_row.kind = 'namespace'
               AND (d.parent_symbol_id = namespace_row.symbol_id OR d.parent_symbol_id IS NULL)
               AND namespace_row.name = json_extract(open_namespace.value, '$[0]')
               AND {language_match}
         ), namespace_best(symbol_id, namespace, path) AS (
             SELECT DISTINCT symbol_id, namespace, path FROM namespace_candidates
             WHERE rank = (SELECT MIN(rank) FROM namespace_candidates)
         )"
            ),
            format!(
                "WHEN EXISTS (SELECT 1 FROM namespace_candidates) THEN
                 CASE WHEN (SELECT COUNT(DISTINCT namespace) FROM namespace_best) = 1
                      THEN (SELECT symbol_id FROM namespace_best
                            ORDER BY ('/' || path) GLOB '*/' || {resolved_type} || '.cs' DESC, path
                            LIMIT 1) END"
            ),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    let lexical_owner_depth =
        "(SELECT MIN(lexical.depth) FROM lexical WHERE lexical.symbol_id = d.symbol_id)";
    let lexical_parent_depth =
        "(SELECT MIN(lexical.depth) FROM lexical WHERE lexical.symbol_id = d.parent_symbol_id)";
    let query = format!(
        "(WITH RECURSIVE fact_source(symbol_id, path, language) AS (
             SELECT s.symbol_id, s.path, s.language FROM symbols s
             WHERE s.symbol_id = {source_symbol_id} AND s.language = {language}
         ), lexical(symbol_id, depth) AS (
             SELECT fact_source.symbol_id, 0 FROM fact_source
             UNION ALL
             SELECT parent.parent_symbol_id, lexical.depth + 1
             FROM symbols parent JOIN lexical ON parent.symbol_id = lexical.symbol_id
             WHERE parent.parent_symbol_id IS NOT NULL AND lexical.depth < 32
         ), {generic_scope_ctes}, local_candidates(symbol_id, depth) AS (
             SELECT DISTINCT d.symbol_id,
                    CASE WHEN EXISTS (SELECT 1 FROM lexical WHERE lexical.symbol_id = d.symbol_id)
                         THEN {lexical_owner_depth}
                         WHEN d.parent_symbol_id IS NULL THEN 100
                         ELSE {lexical_parent_depth}
                    END
             FROM fact_source p JOIN symbols d ON d.name = {resolved_type}
             WHERE {language_match}
               AND d.kind IN ({RELATED_TYPE_KINDS})
               AND (EXISTS (SELECT 1 FROM lexical WHERE lexical.symbol_id = d.symbol_id)
                    OR (d.path = p.path
                        AND (d.parent_symbol_id IS NULL
                             OR EXISTS (SELECT 1 FROM lexical
                                        WHERE lexical.symbol_id = d.parent_symbol_id))))
         ), local_best(symbol_id) AS (
             SELECT symbol_id FROM local_candidates
             WHERE depth = (SELECT MIN(depth) FROM local_candidates)
         ), imported_candidates(symbol_id) AS (
             SELECT DISTINCT d.symbol_id
             FROM fact_source p
             JOIN symbols type_import
               ON type_import.path = p.path
              AND type_import.kind = 'import'
              AND type_import.name = {resolved_type}
             JOIN symbols d
               ON d.name = COALESCE(
                   NULLIF(json_extract(type_import.metadata_json, '$.importedName'), ''),
                   type_import.name
               )
              AND d.kind IN ({RELATED_TYPE_KINDS})
             WHERE json_valid(type_import.metadata_json)
               AND {language_match}
               AND {import_owner}
             UNION
             SELECT DISTINCT d.symbol_id
             FROM fact_source p JOIN symbols d
               ON d.name = {resolved_type}
              AND d.kind IN ({RELATED_TYPE_KINDS})
             WHERE {language_match}
               AND {java_import_owner}
             UNION
             SELECT DISTINCT d.symbol_id
             FROM fact_source p JOIN symbols d
               ON d.name = substr({resolved_type}, instr({resolved_type}, '.') + 1)
              AND d.kind IN ({RELATED_TYPE_KINDS})
             WHERE {language_match}
               AND {go_import_owner}
             {scope_files}
         ){scope_namespaces}, scoped_candidates(symbol_id) AS (
             SELECT DISTINCT d.symbol_id
             FROM fact_source p JOIN symbols d
               ON d.name = {resolved_type}
              AND d.kind IN ({RELATED_TYPE_KINDS})
              AND {language_match}
             WHERE EXISTS (
                 SELECT 1 FROM symbols source_scope
                 JOIN symbols target_scope
                   ON target_scope.name = source_scope.name
                  AND target_scope.kind = source_scope.kind
                  AND target_scope.path = d.path
                  AND target_scope.language = d.language
                 WHERE source_scope.path = p.path
                   AND source_scope.language = p.language
                   AND source_scope.kind = CASE p.language
                       WHEN 'go' THEN 'module'
                       WHEN 'java' THEN 'namespace'
                       WHEN 'csharp' THEN 'namespace'
                   END
                   AND (p.language != 'go' OR {same_directory} = 1)
             )
         )
         , import_bindings(bound) AS (
             SELECT {import_bindings} FROM fact_source p
         )
         SELECT CASE
             WHEN {generic_parameter} THEN NULL
             WHEN EXISTS (SELECT 1 FROM local_candidates) THEN
                 CASE WHEN (SELECT COUNT(*) FROM local_best) = 1
                      THEN (SELECT symbol_id FROM local_best LIMIT 1) END
             WHEN EXISTS (SELECT 1 FROM imported_candidates) THEN
                 CASE WHEN (SELECT COUNT(*) FROM imported_candidates) = 1
                      THEN (SELECT symbol_id FROM imported_candidates LIMIT 1) END
             WHEN (SELECT bound FROM import_bindings) THEN NULL
             {scope_case}
             WHEN EXISTS (SELECT 1 FROM scoped_candidates) THEN
                 CASE WHEN (SELECT COUNT(*) FROM scoped_candidates) = 1
                      THEN (SELECT symbol_id FROM scoped_candidates LIMIT 1) END
             ELSE NULL
         END)"
    );
    query
}

const TYPE_RECEIVER_CHAIN_MAX_DEPTH: usize = 16;
const TYPE_RECEIVER_ANCESTRY_MAX_DEPTH: usize = 8;

pub(crate) fn has_type_fact_support(conn: &Connection) -> bool {
    has_table(conn, "type_facts")
        && has_column(conn, "type_facts", "symbol_id")
        && has_column(conn, "type_facts", "resolved_type")
        && has_column(conn, "type_facts", "language")
        && has_column(conn, "symbols", "symbol_id")
        && has_column(conn, "symbols", "name")
        && has_column(conn, "symbols", "kind")
        && has_column(conn, "symbols", "path")
        && has_column(conn, "symbols", "language")
        && has_column(conn, "symbols", "parent_symbol_id")
}

pub(crate) fn has_typed_receiver_chain_support(conn: &Connection) -> bool {
    has_type_fact_support(conn)
        && has_column(conn, "symbols", "start_line")
        && has_column(conn, "symbols", "metadata_json")
        && has_column(conn, "symbols", "signature")
        && has_table(conn, "relationships")
        && has_column(conn, "relationships", "from_symbol_id")
        && has_column(conn, "relationships", "to_symbol_id")
        && has_column(conn, "relationships", "kind")
        && has_table(conn, "pending_relationships")
        && has_column(conn, "pending_relationships", "from_symbol_id")
        && has_column(conn, "pending_relationships", "target_terminal_name")
        && has_column(conn, "pending_relationships", "kind")
        && has_column(conn, "pending_relationships", "path")
        && has_column(conn, "pending_relationships", "start_line")
        && has_column(conn, "pending_relationships", "file_id")
        && has_column(conn, "pending_relationships", "target_receiver")
        && has_column(conn, "pending_relationships", "target_namespace_json")
        && has_table(conn, "files")
        && has_column(conn, "files", "file_id")
        && has_column(conn, "files", "language")
}

fn typed_receiver_chain_scope_ctes(
    source_symbol_id: &str,
    source_path: &str,
    source_line: &str,
    source_language: &str,
    chain_json: &str,
) -> String {
    let safe_chain = format!("CASE WHEN json_valid({chain_json}) THEN {chain_json} ELSE '[]' END");
    format!(
        "segments(position, name) AS (
             SELECT CAST(key AS INTEGER), value
             FROM json_each({safe_chain})
             WHERE type = 'text' AND CAST(key AS INTEGER) < {TYPE_RECEIVER_CHAIN_MAX_DEPTH}
         ), lexical(symbol_id, depth) AS (
             SELECT s.symbol_id, 0 FROM symbols s WHERE s.symbol_id = {source_symbol_id}
             UNION ALL
             SELECT parent.parent_symbol_id, lexical.depth + 1
             FROM symbols parent JOIN lexical ON parent.symbol_id = lexical.symbol_id
             WHERE parent.parent_symbol_id IS NOT NULL AND lexical.depth < 32
        ), root_candidates(symbol_id, scope_depth) AS (
             SELECT binding.symbol_id, lexical.depth
             FROM segments root
             CROSS JOIN lexical
             JOIN symbols binding ON binding.parent_symbol_id = lexical.symbol_id
             WHERE root.position = 0
               AND binding.name = root.name
               AND binding.path = {source_path}
               AND binding.language = {source_language}
               AND binding.kind IN ('variable', 'parameter', 'field', 'property', 'attribute')
               AND COALESCE(binding.start_line, 0) <= {source_line}
         ), root_bindings(symbol_id) AS (
             SELECT candidate.symbol_id
             FROM root_candidates candidate
             WHERE candidate.scope_depth = (SELECT MIN(scope_depth) FROM root_candidates)
         )"
    )
}

fn generic_parameter_list_matches_sql(parameters: &str, resolved_type: &str) -> String {
    format!(
        "(instr(',' || replace(trim({parameters}), ', ', ',') || ',',
                ',' || {resolved_type} || ',') > 0
         OR instr(',' || replace(trim({parameters}), ', ', ',') || ',',
                  ',' || {resolved_type} || ' ') > 0
         OR instr(',' || replace(trim({parameters}), ', ', ',') || ',',
                  ',' || {resolved_type} || ':') > 0
         OR instr(',' || replace(trim({parameters}), ', ', ',') || ',',
                  ',' || {resolved_type} || '=') > 0)"
    )
}

fn generic_prefix_modifier_guard(
    open: &str,
    token_cte: &str,
    allowed_modifiers: &[&str],
) -> String {
    let allowed = allowed_modifiers
        .iter()
        .map(|modifier| format!("'{modifier}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "NOT EXISTS (
             WITH RECURSIVE {token_cte}(rest, token) AS (
                 SELECT trim(lower(replace(replace(replace(
                     substr(generic_owner.signature, 1, {open} - 1),
                     char(9), ' '), char(10), ' '), char(13), ' '))), NULL
                 UNION ALL
                 SELECT CASE WHEN instr(rest, ' ') = 0 THEN ''
                             ELSE ltrim(substr(rest, instr(rest, ' ') + 1)) END,
                        CASE WHEN instr(rest, ' ') = 0 THEN rest
                             ELSE substr(rest, 1, instr(rest, ' ') - 1) END
                 FROM {token_cte}
                 WHERE rest != ''
             )
             SELECT 1 FROM {token_cte}
             WHERE token IS NOT NULL AND token NOT IN ({allowed})
         )"
    )
}

fn type_fact_generic_scope_ctes() -> String {
    let java_open = "instr(generic_owner.signature, '<')";
    let java_close =
        format!("({java_open} + instr(substr(generic_owner.signature, {java_open}), '>') - 1)");
    let java_prefix_modifiers = generic_prefix_modifier_guard(
        java_open,
        "java_prefix_tokens",
        &[
            "public",
            "protected",
            "private",
            "static",
            "final",
            "abstract",
            "synchronized",
            "native",
            "strictfp",
            "default",
        ],
    );
    let csharp_open = "instr(generic_owner.signature, '<')";
    let csharp_close =
        format!("({csharp_open} + instr(substr(generic_owner.signature, {csharp_open}), '>') - 1)");
    let csharp_prefix_modifiers = generic_prefix_modifier_guard(
        csharp_open,
        "csharp_prefix_tokens",
        &[
            "public",
            "protected",
            "internal",
            "private",
            "new",
            "static",
            "virtual",
            "abstract",
            "override",
            "sealed",
            "extern",
            "async",
            "unsafe",
            "partial",
            "readonly",
        ],
    );
    let kotlin_open = "instr(lower(generic_owner.signature), 'fun <') + 4";
    let cpp_tail = "ltrim(substr(generic_owner.signature, instr(lower(generic_owner.signature), 'template') + 8))";
    format!(
        "generic_scope_headers(owner_id, name, kind, language, signature, metadata_json, after_name) AS (
             SELECT generic_owner.symbol_id, generic_owner.name, generic_owner.kind,
                    generic_owner.language,
                    COALESCE(generic_owner.signature, ''), COALESCE(generic_owner.metadata_json, ''),
                    ltrim(substr(COALESCE(generic_owner.signature, ''),
                                 instr(COALESCE(generic_owner.signature, ''), generic_owner.name)
                                 + length(generic_owner.name)))
             FROM lexical
             JOIN symbols generic_owner ON generic_owner.symbol_id = lexical.symbol_id
             WHERE lexical.depth > 0
               AND instr(COALESCE(generic_owner.signature, ''), generic_owner.name) > 0
         ), generic_scopes(owner_id, language, metadata_json, parameters) AS (
             SELECT generic_owner.owner_id, generic_owner.language, generic_owner.metadata_json,
                    CASE
                        WHEN generic_owner.language = 'go'
                             AND substr(generic_owner.after_name, 1, 1) = '['
                             AND instr(generic_owner.after_name, ']') > 1
                             AND (generic_owner.kind NOT IN ('method', 'function', 'constructor')
                                  OR substr(ltrim(substr(generic_owner.after_name,
                                      instr(generic_owner.after_name, ']') + 1)), 1, 1) = '(')
                            THEN substr(generic_owner.after_name, 2,
                                        instr(generic_owner.after_name, ']') - 2)
                        WHEN generic_owner.language = 'scala'
                             AND substr(generic_owner.after_name, 1, 1) = '['
                             AND instr(generic_owner.after_name, ']') > 1
                             AND (generic_owner.kind NOT IN ('method', 'function', 'constructor')
                                  OR substr(ltrim(substr(generic_owner.after_name,
                                      instr(generic_owner.after_name, ']') + 1)), 1, 1) = '(')
                            THEN substr(generic_owner.after_name, 2,
                                        instr(generic_owner.after_name, ']') - 2)
                        WHEN generic_owner.language IN ('csharp', 'dart', 'java', 'kotlin',
                                                       'rust', 'swift', 'typescript', 'tsx',
                                                       'cpp')
                             AND substr(generic_owner.after_name, 1, 1) = '<'
                             AND instr(generic_owner.after_name, '>') > 1
                             AND (generic_owner.kind NOT IN ('method', 'function', 'constructor')
                                  OR substr(ltrim(substr(generic_owner.after_name,
                                      instr(generic_owner.after_name, '>') + 1)), 1, 1) = '(')
                            THEN substr(generic_owner.after_name, 2,
                                        instr(generic_owner.after_name, '>') - 2)
                        WHEN generic_owner.language = 'vbnet'
                             AND lower(substr(generic_owner.after_name, 1, 4)) = '(of '
                             AND instr(generic_owner.after_name, ')') > 4
                            THEN substr(generic_owner.after_name, 5,
                                        instr(generic_owner.after_name, ')') - 5)
                        WHEN generic_owner.language = 'java'
                             AND generic_owner.kind IN ('method', 'constructor')
                             AND {java_open} > 0
                             AND instr(substr(generic_owner.signature, {java_close} + 1),
                                       generic_owner.name) > 0
                             AND instr(substr(generic_owner.signature, {java_close} + 1), '(') > 0
                             AND {java_prefix_modifiers}
                            THEN substr(generic_owner.signature, {java_open} + 1,
                                        instr(substr(generic_owner.signature, {java_open}), '>') - 2)
                        WHEN generic_owner.language = 'csharp'
                             AND generic_owner.kind IN ('method', 'constructor')
                             AND {csharp_open} > 0
                             AND instr(substr(generic_owner.signature, {csharp_close} + 1),
                                       generic_owner.name) > 0
                             AND instr(substr(generic_owner.signature, {csharp_close} + 1), '(') > 0
                             AND {csharp_prefix_modifiers}
                            THEN substr(generic_owner.signature, {csharp_open} + 1,
                                        instr(substr(generic_owner.signature, {csharp_open}), '>') - 2)
                        WHEN generic_owner.language = 'kotlin'
                             AND instr(lower(generic_owner.signature), 'fun <') > 0
                             AND instr(generic_owner.signature, generic_owner.name)
                                 > {kotlin_open}
                                   + instr(substr(generic_owner.signature, {kotlin_open}), '>') - 1
                            THEN substr(generic_owner.signature, {kotlin_open} + 1,
                                        instr(substr(generic_owner.signature, {kotlin_open}), '>') - 2)
                        WHEN generic_owner.language = 'cpp'
                             AND instr(lower(generic_owner.signature), 'template') > 0
                             AND substr({cpp_tail}, 1, 1) = '<'
                             AND instr({cpp_tail}, '>') > 1
                             AND instr(generic_owner.signature, generic_owner.name)
                                 > instr(generic_owner.signature, 'template') + 7
                                   + instr({cpp_tail}, '>')
                            THEN replace(replace(replace(
                                     substr({cpp_tail}, 2, instr({cpp_tail}, '>') - 2),
                                     'typename... ', ''), 'typename ', ''), 'class ', '')
                    END
             FROM generic_scope_headers generic_owner
         )"
    )
}

fn type_fact_is_generic_parameter_sql(generic_scopes: &str, resolved_type: &str) -> String {
    let parameters_match =
        generic_parameter_list_matches_sql("generic_scope.parameters", resolved_type);
    format!(
        "EXISTS (
             SELECT 1 FROM {generic_scopes} generic_scope
             WHERE EXISTS (
                 SELECT 1
                 FROM json_each(
                     CASE WHEN json_valid(generic_scope.metadata_json)
                               AND json_type(generic_scope.metadata_json, '$.typeParameters') = 'array'
                          THEN json_extract(generic_scope.metadata_json, '$.typeParameters')
                          ELSE '[]'
                     END
                 ) declared_parameter
                 WHERE CASE
                     WHEN instr(declared_parameter.value, ' ') > 0
                         THEN substr(declared_parameter.value, 1, instr(declared_parameter.value, ' ') - 1)
                     WHEN instr(declared_parameter.value, ':') > 0
                         THEN substr(declared_parameter.value, 1, instr(declared_parameter.value, ':') - 1)
                     WHEN instr(declared_parameter.value, '=') > 0
                         THEN substr(declared_parameter.value, 1, instr(declared_parameter.value, '=') - 1)
                     ELSE trim(declared_parameter.value)
                 END = {resolved_type}
             )
             OR (generic_scope.parameters IS NOT NULL AND {parameters_match})
         )"
    )
}

/// The status of a receiver chain whose owner holds the target as a member.
pub(crate) const RECEIVER_CHAIN_STATUS_RECEIVER_TYPE: i64 = 1;
/// The status of a receiver chain that starts at a type and whose owner holds the target.
pub(crate) const RECEIVER_CHAIN_STATUS_QUALIFIED_PATH: i64 = 2;
/// The status of a receiver chain whose owner does not hold the target.
pub(crate) const RECEIVER_CHAIN_STATUS_DIFFERENT_OWNER: i64 = 0;
/// The status of a receiver chain that the name-path rules judge ([`RECEIVER_CHAIN_LEGACY_ROOT`]).
pub(crate) const RECEIVER_CHAIN_STATUS_LEGACY: i64 = -1;

/// The receiver names that stand for the instance of the caller's class.
const SELF_RECEIVER_NAMES: &str = "'self', 'this', 'cls'";

/// Languages where an enum constant (`Color.RED`) is a value of its enum, so a receiver chain
/// steps from the constant to the enum.
const ENUM_INSTANCE_LANGUAGES: &str =
    "'java', 'kotlin', 'python', 'swift', 'scala', 'dart', 'rust', 'csharp', 'vbnet', 'php'";

/// What the owner query of a receiver chain returns.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReceiverChainProjection {
    /// The owner id, or NULL.
    Owner,
    /// `<root kind>:<owner id>` (root kind `value`, `self`, or `type`), [`RECEIVER_CHAIN_LEGACY_ROOT`]
    /// when the first name binds no value and no type, or NULL when the evidence is incomplete.
    RootAndOwner,
}

/// The owner-query value for a chain whose first name binds neither a value nor a type, such as
/// a package or module (`app.Outer.Inner.build()`): the name-path rules judge it.
pub(crate) const RECEIVER_CHAIN_LEGACY_ROOT: &str = "legacy";

/// A scalar owner id for the receiver chain `chain_json` of a call, or SQL NULL when the index
/// cannot bind it. The first name binds, in order, a typed value in the caller's scope, the
/// caller's class for `self`, `this`, or `cls`, or a type that the caller's scope names.
pub(crate) fn typed_receiver_chain_owner_id_sql(
    source_symbol_id: &str,
    source_path: &str,
    source_line: &str,
    source_language: &str,
    chain_json: &str,
) -> String {
    typed_receiver_chain_owner_sql(
        source_symbol_id,
        source_path,
        source_line,
        source_language,
        chain_json,
        ReceiverChainProjection::Owner,
    )
}

fn typed_receiver_chain_owner_sql(
    source_symbol_id: &str,
    source_path: &str,
    source_line: &str,
    source_language: &str,
    chain_json: &str,
    projection: ReceiverChainProjection,
) -> String {
    let safe_chain = format!("CASE WHEN json_valid({chain_json}) THEN {chain_json} ELSE '[]' END");
    let scope_ctes = typed_receiver_chain_scope_ctes(
        source_symbol_id,
        source_path,
        source_line,
        source_language,
        chain_json,
    );
    let root_owner = type_fact_owner_id_sql(
        "root_fact.symbol_id",
        "root_fact.resolved_type",
        "root_fact.language",
    );
    let member_owner = type_fact_owner_id_sql(
        "member.symbol_id",
        "member_fact.resolved_type",
        "member_fact.language",
    );
    let member_kinds = "'field', 'property', 'variable', 'attribute', 'constant'";
    let owner_value = match projection {
        ReceiverChainProjection::Owner => "receiver_chain.owner_id",
        ReceiverChainProjection::RootAndOwner => {
            "receiver_chain.root_kind || ':' || receiver_chain.owner_id"
        }
    };
    let resolved_owner = format!(
        "SELECT {owner_value}
         FROM receiver_chain
         WHERE receiver_chain.position = (SELECT MAX(position) FROM segments)
           AND receiver_chain.owner_id IS NOT NULL
           AND (SELECT COUNT(*) FROM segments) = json_array_length({safe_chain})
           AND json_array_length({safe_chain}) BETWEEN 1 AND {depth_limit_plus_one}
         LIMIT 1",
        depth_limit_plus_one = TYPE_RECEIVER_CHAIN_MAX_DEPTH,
    );
    let selected = match projection {
        ReceiverChainProjection::Owner => resolved_owner,
        ReceiverChainProjection::RootAndOwner => format!(
            "SELECT COALESCE(({resolved_owner}),
                    CASE WHEN EXISTS (SELECT 1 FROM type_root)
                              AND NOT EXISTS (SELECT 1 FROM receiver_chain
                                              WHERE receiver_chain.position = 0
                                                AND receiver_chain.owner_id IS NOT NULL)
                         THEN '{RECEIVER_CHAIN_LEGACY_ROOT}' END)"
        ),
    };
    let query = format!(
        "(WITH RECURSIVE {scope_ctes}, root_facts(symbol_id, resolved_type, language) AS (
             SELECT binding.symbol_id, fact.resolved_type, fact.language
             FROM root_bindings binding
             JOIN type_facts fact ON fact.symbol_id = binding.symbol_id
             WHERE (SELECT COUNT(*) FROM root_bindings) = 1
               AND (SELECT COUNT(*) FROM type_facts duplicate
                    WHERE duplicate.symbol_id = binding.symbol_id) = 1
         ), type_root(symbol_id, resolved_type, language) AS (
             SELECT {source_symbol_id}, root.name, {source_language}
             FROM segments root
             WHERE root.position = 0
               AND root.name NOT IN ({SELF_RECEIVER_NAMES})
               AND NOT EXISTS (SELECT 1 FROM root_bindings)
         ), self_root(owner_id) AS (
             SELECT caller_class.symbol_id
             FROM segments root
             JOIN symbols caller ON caller.symbol_id = {source_symbol_id}
             JOIN symbols caller_class ON caller_class.symbol_id = caller.parent_symbol_id
             WHERE root.position = 0
               AND root.name IN ({SELF_RECEIVER_NAMES})
               AND caller.kind IN ('method', 'constructor', 'property')
               AND caller_class.kind IN ('class', 'struct', 'record', 'object', 'enum')
               AND caller_class.language = caller.language
               AND NOT EXISTS (SELECT 1 FROM root_facts)
         ), root_owner_facts(symbol_id, resolved_type, language, root_kind) AS (
             SELECT symbol_id, resolved_type, language, 'value' FROM root_facts
             UNION ALL
             SELECT symbol_id, resolved_type, language, 'type' FROM type_root
         ), receiver_chain(position, owner_id, root_kind) AS (
             SELECT 0, {root_owner}, root_fact.root_kind FROM root_owner_facts root_fact
             UNION ALL
             SELECT 0, self_root.owner_id, 'self' FROM self_root
             UNION ALL
             SELECT segment.position,
                    CASE
                        WHEN (SELECT COUNT(*) FROM symbols candidate
                              WHERE candidate.parent_symbol_id = receiver_chain.owner_id
                                AND candidate.name = segment.name
                                AND candidate.kind IN ({member_kinds})) = 1
                         AND (SELECT COUNT(*) FROM type_facts fact
                              JOIN symbols candidate ON candidate.symbol_id = fact.symbol_id
                              WHERE candidate.parent_symbol_id = receiver_chain.owner_id
                                AND candidate.name = segment.name
                                AND candidate.kind IN ({member_kinds})) = 1
                        THEN (SELECT {member_owner}
                              FROM symbols member
                              JOIN type_facts member_fact ON member_fact.symbol_id = member.symbol_id
                              WHERE member.parent_symbol_id = receiver_chain.owner_id
                                AND member.name = segment.name
                                AND member.kind IN ({member_kinds})
                              LIMIT 1)
                        WHEN NOT EXISTS (SELECT 1 FROM symbols candidate
                                         WHERE candidate.parent_symbol_id = receiver_chain.owner_id
                                           AND candidate.name = segment.name
                                           AND candidate.kind IN ({member_kinds}))
                         AND EXISTS (SELECT 1 FROM symbols owner
                                     WHERE owner.symbol_id = receiver_chain.owner_id
                                       AND owner.kind = 'enum'
                                       AND owner.language IN ({ENUM_INSTANCE_LANGUAGES}))
                         AND (SELECT COUNT(*) FROM symbols constant
                              WHERE constant.parent_symbol_id = receiver_chain.owner_id
                                AND constant.name = segment.name
                                AND constant.kind = 'enum_member') = 1
                         AND NOT EXISTS (SELECT 1 FROM symbols constant
                                         JOIN symbols body ON body.parent_symbol_id = constant.symbol_id
                                         WHERE constant.parent_symbol_id = receiver_chain.owner_id
                                           AND constant.name = segment.name
                                           AND constant.kind = 'enum_member')
                        THEN receiver_chain.owner_id
                        WHEN NOT EXISTS (SELECT 1 FROM symbols candidate
                                         WHERE candidate.parent_symbol_id = receiver_chain.owner_id
                                           AND candidate.name = segment.name
                                           AND candidate.kind IN ({member_kinds}))
                         AND (SELECT COUNT(*) FROM symbols nested
                              WHERE nested.parent_symbol_id = receiver_chain.owner_id
                                AND nested.name = segment.name
                                AND nested.kind IN ({NESTED_TYPE_KINDS})) = 1
                        THEN (SELECT nested.symbol_id FROM symbols nested
                              WHERE nested.parent_symbol_id = receiver_chain.owner_id
                                AND nested.name = segment.name
                                AND nested.kind IN ({NESTED_TYPE_KINDS})
                              LIMIT 1)
                        ELSE NULL
                    END,
                    receiver_chain.root_kind
             FROM receiver_chain
             JOIN segments segment ON segment.position = receiver_chain.position + 1
             WHERE receiver_chain.owner_id IS NOT NULL
               AND receiver_chain.position < {depth_limit}
         )
         {selected})",
        depth_limit = TYPE_RECEIVER_CHAIN_MAX_DEPTH - 1,
    );
    query
}

/// Each pending row of `?1` with a receiver chain and its owner value in the form of
/// [`ReceiverChainProjection::RootAndOwner`].
pub(crate) fn typed_receiver_chain_owner_rows_sql() -> String {
    let receiver_chain_json = "json_insert(
        CASE WHEN json_valid(pending.target_namespace_json)
                   AND json_type(pending.target_namespace_json) = 'array'
             THEN pending.target_namespace_json ELSE '[]' END,
        '$[#]', pending.target_receiver
    )";
    let owner_id = typed_receiver_chain_owner_sql(
        "pending.from_symbol_id",
        "pending.path",
        "pending.start_line",
        "COALESCE(source.language, source_file.language, '')",
        receiver_chain_json,
        ReceiverChainProjection::RootAndOwner,
    );
    format!(
        "SELECT 'P:' || CAST(pending.rowid AS TEXT), {owner_id}
         FROM pending_relationships pending
         LEFT JOIN symbols source ON source.symbol_id = pending.from_symbol_id
         LEFT JOIN files source_file ON source_file.file_id = pending.file_id
         WHERE pending.rowid IN (SELECT CAST(value AS INTEGER) FROM json_each(?1))
           AND pending.kind != 'imports'
           AND json_array_length({receiver_chain_json}) > 1
           AND pending.target_receiver IS NOT NULL AND pending.target_receiver != ''"
    )
}

pub(crate) fn typed_receiver_chain_pending_base_owner_rows_sql() -> String {
    let pending_namespace = "CASE WHEN json_valid(pending.target_namespace_json)
                                   AND json_type(pending.target_namespace_json) = 'array'
                              THEN pending.target_namespace_json ELSE '[]' END";
    let pending_import = type_fact_import_binding_sql(
        "pending.path",
        "pending.target_terminal_name",
        "current_owner.language",
    );
    let pending_owner = type_fact_owner_id_sql(
        "pending.from_symbol_id",
        "pending.target_terminal_name",
        "current_owner.language",
    );
    let bound_owner = format!(
        "CASE WHEN json_array_length({pending_namespace}) > 0 AND NOT ({pending_import})
              THEN NULL ELSE {pending_owner} END"
    );
    format!(
        "SELECT CAST(pending.rowid AS TEXT), {bound_owner}
         FROM pending_relationships pending
         JOIN symbols current_owner ON current_owner.symbol_id = pending.from_symbol_id
         WHERE pending.kind IN ('extends', 'implements')
           AND pending.from_symbol_id IN (SELECT value FROM json_each(?1))
         LIMIT ?2"
    )
}

pub(crate) fn typed_receiver_chain_resolved_base_rows_sql() -> &'static str {
    "SELECT to_symbol_id FROM relationships
     WHERE kind IN ('extends', 'implements')
       AND from_symbol_id IN (SELECT value FROM json_each(?1))
     LIMIT ?2"
}

pub(crate) fn typed_receiver_chain_has_more_ancestors_sql() -> &'static str {
    "SELECT EXISTS (
        SELECT 1 FROM (
            SELECT 1 FROM relationships
            WHERE kind IN ('extends', 'implements')
              AND from_symbol_id IN (SELECT value FROM json_each(?1))
            UNION ALL
            SELECT 1 FROM pending_relationships
            WHERE kind IN ('extends', 'implements')
              AND from_symbol_id IN (SELECT value FROM json_each(?1))
        ) ancestry_edge
        LIMIT 1
    )"
}

pub(crate) fn typed_receiver_chain_target_status_sql(
    receiver_owner_id: &str,
    target: &str,
) -> String {
    typed_receiver_chain_target_status_sql_with_base_owner_map(receiver_owner_id, target, None)
}

pub(crate) fn typed_receiver_chain_target_status_with_base_owner_map_sql(
    receiver_owner_id: &str,
    target: &str,
    pending_base_owner_map: &str,
) -> String {
    typed_receiver_chain_target_status_sql_with_base_owner_map(
        receiver_owner_id,
        target,
        Some(pending_base_owner_map),
    )
}

fn typed_receiver_chain_target_status_sql_with_base_owner_map(
    receiver_owner_id: &str,
    target: &str,
    pending_base_owner_map: Option<&str>,
) -> String {
    let owner_kinds = "'class', 'interface', 'struct', 'trait', 'protocol', 'record', 'object'";
    let candidate_language = identifier_language_matches("current_owner", "candidate");
    let unresolved_base_language = identifier_language_matches("unresolved_base", "possible_base");
    let pending_base = match pending_base_owner_map {
        Some(owner_map) => format!(
            "JOIN json_each({owner_map}) bound_base
               ON bound_base.key = CAST(pending.rowid AS TEXT)"
        ),
        None => {
            let pending_import = type_fact_import_binding_sql(
                "pending.path",
                "pending.target_terminal_name",
                "current_owner.language",
            );
            let pending_namespace = "CASE WHEN json_valid(pending.target_namespace_json)
                                           AND json_type(pending.target_namespace_json) = 'array'
                                      THEN pending.target_namespace_json ELSE '[]' END";
            let pending_owner = type_fact_owner_id_sql(
                "pending.from_symbol_id",
                "pending.target_terminal_name",
                "current_owner.language",
            );
            let pending_owner = format!(
                "CASE WHEN json_array_length({pending_namespace}) > 0 AND NOT ({pending_import})
                      THEN NULL ELSE {pending_owner} END"
            );
            format!("CROSS JOIN json_each(json_array({pending_owner})) bound_base")
        }
    };
    let callable_target = format!(
        "{target}.kind IN ('method', 'function', 'constructor')
         AND nearer_member.kind IN ('method', 'function', 'constructor')"
    );
    let value_target = format!(
        "{target}.kind IN ('field', 'property', 'attribute', 'variable')
         AND nearer_member.kind IN ('field', 'property', 'attribute', 'variable')"
    );
    format!(
        "(WITH RECURSIVE receiver_owners(symbol_id, depth, ambiguous, language) AS (
                SELECT {receiver_owner_id}, 0, 0, NULL
                UNION
                SELECT relationship.to_symbol_id, receiver_owners.depth + 1, receiver_owners.ambiguous, NULL
                FROM relationships relationship
                JOIN receiver_owners ON relationship.from_symbol_id = receiver_owners.symbol_id
                WHERE relationship.kind IN ('extends', 'implements')
                  AND receiver_owners.depth < {ancestry_depth}
                UNION
                SELECT bound_base.value, receiver_owners.depth + 1,
                       CASE WHEN bound_base.value IS NULL THEN 1 ELSE receiver_owners.ambiguous END,
                       CASE WHEN bound_base.value IS NULL THEN current_owner.language END
                FROM pending_relationships pending
                JOIN receiver_owners ON pending.from_symbol_id = receiver_owners.symbol_id
                JOIN symbols current_owner ON current_owner.symbol_id = receiver_owners.symbol_id
                {pending_base}
                LEFT JOIN symbols candidate ON candidate.symbol_id = bound_base.value
                WHERE pending.kind IN ('extends', 'implements')
                  AND receiver_owners.depth < {ancestry_depth}
                  AND (bound_base.value IS NULL
                       OR candidate.kind IN ({owner_kinds}) AND {candidate_language})
            ), unresolved_base_paths(depth) AS (
                SELECT unresolved_base.depth
                FROM receiver_owners unresolved_base
                JOIN symbols possible_base ON possible_base.symbol_id = {target}.parent_symbol_id
                WHERE unresolved_base.symbol_id IS NULL
                  AND unresolved_base.ambiguous = 1
                  AND possible_base.kind IN ({owner_kinds})
                  AND {unresolved_base_language}
                  AND {target}.parent_symbol_id IS NOT NULL
                  AND {target}.kind IN ('method', 'function', 'constructor', 'field', 'property', 'attribute', 'variable')
                  AND NOT EXISTS (
                      SELECT 1 FROM receiver_owners known_owner
                      JOIN symbols nearer_member ON nearer_member.parent_symbol_id = known_owner.symbol_id
                      WHERE known_owner.symbol_id IS NOT NULL
                        AND known_owner.ambiguous = 0
                        AND nearer_member.name = {target}.name
                        AND ({callable_target} OR {value_target})
                  )
            ), target_paths(depth, ambiguous) AS (
                SELECT matched_owner.depth, matched_owner.ambiguous
                FROM receiver_owners matched_owner
                WHERE matched_owner.symbol_id = {target}.parent_symbol_id
                  AND {target}.parent_symbol_id IS NOT NULL
                  AND {target}.kind IN ('method', 'function', 'constructor', 'field', 'property', 'attribute', 'variable')
                  AND ({target}.kind != 'constructor' OR matched_owner.depth = 0)
            ), proven_path(depth) AS (
                SELECT target_paths.depth
                FROM target_paths
                WHERE target_paths.ambiguous = 0
                  AND NOT EXISTS (
                      SELECT 1 FROM receiver_owners closer_owner
                      JOIN symbols nearer_member ON nearer_member.parent_symbol_id = closer_owner.symbol_id
                      WHERE closer_owner.depth < target_paths.depth
                        AND closer_owner.ambiguous = 0
                        AND nearer_member.name = {target}.name
                        AND ({callable_target} OR {value_target})
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM receiver_owners closer_owner
                      JOIN symbols nearer_member ON nearer_member.parent_symbol_id = closer_owner.symbol_id
                      WHERE closer_owner.depth < target_paths.depth
                        AND closer_owner.ambiguous = 1
                        AND nearer_member.name = {target}.name
                        AND ({callable_target} OR {value_target})
                  )
            ), uncertain_path AS (
                SELECT 1 FROM target_paths WHERE target_paths.ambiguous = 1
                UNION ALL
                SELECT 1 FROM target_paths
                JOIN receiver_owners closer_owner ON closer_owner.depth < target_paths.depth
                JOIN symbols nearer_member ON nearer_member.parent_symbol_id = closer_owner.symbol_id
                WHERE target_paths.ambiguous = 0
                  AND closer_owner.ambiguous = 1
                  AND nearer_member.name = {target}.name
                  AND ({callable_target} OR {value_target})
                UNION ALL
                SELECT 1 FROM unresolved_base_paths
            )
            SELECT CASE
                WHEN EXISTS (SELECT 1 FROM uncertain_path) THEN NULL
                WHEN EXISTS (SELECT 1 FROM proven_path) THEN 1
                WHEN {related_part} THEN NULL
                ELSE 0
            END)",
        ancestry_depth = TYPE_RECEIVER_ANCESTRY_MAX_DEPTH,
        related_part = receiver_owner_related_part_sql(target),
    )
}

/// SQL that holds when `target` belongs to a part of a receiver owner that the index does not
/// link to it: a Kotlin companion object or a Java enum constant body nested in the owner, another
/// part with the owner's name (a C# or VB.NET partial type, a reopened Ruby class, a Scala
/// companion), or an extension of a type with the owner's name. Such a target may be reached, so
/// a receiver chain that ends at the owner does not exclude it. Reads `receiver_owners`.
fn receiver_owner_related_part_sql(target: &str) -> String {
    let extended_type = "CASE WHEN json_valid(target_source.metadata_json)
                               THEN json_extract(target_source.metadata_json, '$.extendedType') END,
                          CASE WHEN json_valid(target_owner.metadata_json)
                               THEN json_extract(target_owner.metadata_json, '$.extendedType') END,
                          (SELECT json_extract(parameter.metadata_json, '$.variableType')
                           FROM symbols parameter
                           WHERE parameter.parent_symbol_id = target_source.symbol_id
                             AND parameter.signature LIKE 'this %'
                             AND json_valid(parameter.metadata_json)
                             AND json_extract(parameter.metadata_json, '$.role') = 'parameter'
                           LIMIT 1)";
    format!(
        "EXISTS (
            SELECT 1
            FROM receiver_owners related_owner
            JOIN symbols chain_owner ON chain_owner.symbol_id = related_owner.symbol_id
            LEFT JOIN symbols target_source ON target_source.symbol_id = {target}.symbol_id
            LEFT JOIN symbols target_owner ON target_owner.symbol_id = {target}.parent_symbol_id
            WHERE (target_owner.parent_symbol_id = chain_owner.symbol_id
                   AND (target_owner.kind = 'enum_member'
                        OR json_valid(target_owner.metadata_json)
                           AND json_extract(target_owner.metadata_json, '$.type') = 'companion-object'))
               OR (target_owner.symbol_id != chain_owner.symbol_id
                   AND target_owner.name = chain_owner.name
                   AND target_owner.language = chain_owner.language
                   AND COALESCE((SELECT parent.name FROM symbols parent
                                 WHERE parent.symbol_id = target_owner.parent_symbol_id), '')
                       = COALESCE((SELECT parent.name FROM symbols parent
                                   WHERE parent.symbol_id = chain_owner.parent_symbol_id), '')
                   AND (chain_owner.language IN ('ruby', 'scala')
                        OR ' ' || lower(target_owner.signature) || ' ' LIKE '% partial %'
                        OR ' ' || lower(chain_owner.signature) || ' ' LIKE '% partial %'))
               OR instr('.' || replace(replace(replace(COALESCE({extended_type}, ''),
                                                       '::', '.'), '<', '.'), '?', '') || '.',
                        '.' || chain_owner.name || '.') > 0
        )"
    )
}

pub(crate) fn typed_receiver_chain_matches_target_sql(
    receiver_owner_id: &str,
    target: &str,
) -> String {
    format!(
        "({} = 1)",
        typed_receiver_chain_target_status_sql(receiver_owner_id, target)
    )
}

const MAX_RELATED_TYPE_FACTS: usize = 4096;

/// True when the type `alias` is declared at the level of its file: with no parent, or in a
/// namespace, such as C's `extern "C"` block.
fn module_level_type_sql(alias: &str) -> String {
    format!(
        "({alias}.parent_symbol_id IS NULL OR EXISTS (
            SELECT 1 FROM symbols type_scope
            WHERE type_scope.symbol_id = {alias}.parent_symbol_id
              AND type_scope.kind = 'namespace'))"
    )
}

/// The workspace type definitions bound by the type facts of `target` and its direct members.
/// Unbound or ambiguous names are omitted; each owner appears once as `signature (path:line)`.
pub fn find_related_types(
    conn: &Connection,
    target: &Symbol,
    limit: usize,
) -> Result<Vec<String>, QueryError> {
    find_related_types_with_fact_limit(conn, target, limit, MAX_RELATED_TYPE_FACTS)
}

fn find_related_types_with_fact_limit(
    conn: &Connection,
    target: &Symbol,
    limit: usize,
    fact_limit: usize,
) -> Result<Vec<String>, QueryError> {
    if !has_table(conn, "type_facts") {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT t.type_fact_id, t.symbol_id, t.language, t.resolved_type,
                t.generic_params_json, s.path
         FROM type_facts t JOIN symbols s ON s.symbol_id = t.symbol_id
         WHERE s.symbol_id = ?1 OR s.parent_symbol_id = ?1
         ORDER BY s.path, s.start_line, s.start_column, t.type_fact_id
         LIMIT ?2",
    )?;
    let mut rows = stmt.query(params![target.symbol_id, fact_limit as i64])?;
    let mut owner_statement = None;
    let mut scopes = import_scope::ScopeReader::new(conn);
    let mut seen = HashSet::new();
    let mut types = Vec::new();
    while let Some(row) = rows.next()? {
        if types.len() >= limit {
            break;
        }
        let fact = TypeFact {
            type_fact_id: row.get(0)?,
            symbol_id: row.get(1)?,
            language: row.get(2)?,
            resolved_type: row.get(3)?,
            generic_params: row.get(4)?,
        };
        if owner_statement.is_none() {
            let owner_query = format!(
                "SELECT {}",
                type_fact_owner_id_in_scope_sql("?1", "?2", "?3", Some(("?4", "?5")))
            );
            owner_statement = Some(conn.prepare(&owner_query)?);
        }
        let path: String = row.get(5)?;
        let scope = scopes.scope(&fact.symbol_id, &path, &fact.language, &fact.resolved_type)?;
        let owner_id = owner_statement
            .as_mut()
            .expect("owner query was just prepared")
            .query_row(
                params![
                    fact.symbol_id,
                    fact.resolved_type,
                    fact.language,
                    scope.files,
                    scope.namespaces
                ],
                |row| row.get::<_, Option<String>>(0),
            )?;
        let Some(owner) = owner_id
            .map(|symbol_id| get_symbol_by_id(conn, &symbol_id))
            .transpose()?
            .flatten()
        else {
            continue;
        };
        if !seen.insert(owner.symbol_id) {
            continue;
        }
        let signature = owner
            .signature
            .unwrap_or_else(|| format!("{} {}", owner.kind, owner.name));
        types.push(format!(
            "{} ({}:{})",
            type_declaration_line(&signature),
            owner.path,
            owner.start_line
        ));
    }
    Ok(types)
}

/// A type signature without leading attributes, on one line of at most 120 characters.
fn type_declaration_line(signature: &str) -> String {
    let mut declaration = signature;
    while declaration.starts_with("#[")
        && let Some((_, rest)) = declaration.split_once("] ")
    {
        declaration = rest;
    }
    let line = declaration.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= 120 {
        return line;
    }
    line.chars().take(119).collect::<String>() + "…"
}

/// True when a repository-relative path looks like a test file. Directory rules and file-name
/// rules are kept apart: a `test`, `tests`, `autotests`, or `__tests__` directory anywhere
/// including the repository root, or a file name that starts with Qt's `tst_`, starts with
/// `test_` in Python or Ruby, contains `_test.`,
/// `.test.`, or `.spec.`, is exactly `test.rs` or `tests.rs`, or ends with the C# `Tests.cs`
/// (case-sensitive, so `Contests.cs` is a production file).
/// A name that reads like a test (`test_run`, `TestRoutes`, `runSpec`), for a symbol in a test file
/// that julie did not flag and that is not in a test class: an app factory such as `create_app` in
/// `tests/test_apps` is not a test, but a helper method of a test class leads to that class.
/// The SQL mirror of [`names_a_test`] for the row `alias`; `LIKE` ignores ASCII case.
fn names_a_test_sql(alias: &str) -> String {
    ["test%", "%test", "%tests", "%spec", "%specs"]
        .map(|pattern| format!("{alias}.name LIKE '{pattern}'"))
        .join(" OR ")
}

/// The test a related-test row stands for: the row, or the test that encloses a helper defined
/// inside it (`@app.url_defaults def add_language_code` in `test_url_processors`).
fn enclosing_test(conn: &Connection, row: Symbol) -> Result<Symbol, QueryError> {
    if row.is_test || row.test_container || names_a_test(&row.name) {
        return Ok(row);
    }
    let parent = row
        .parent_symbol_id
        .as_deref()
        .map(|id| get_symbol_by_id(conn, id))
        .transpose()?
        .flatten();
    Ok(match parent {
        Some(parent) if matches!(parent.kind.as_str(), "function" | "method") => parent,
        _ => row,
    })
}

pub(crate) fn names_a_test(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.starts_with("test")
        || ["test", "tests", "spec", "specs"]
            .iter()
            .any(|suffix| lowered.ends_with(suffix))
}

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
    compute_blast_radius_scoped_with_ids(
        conn,
        seed_symbols,
        &[],
        symbol_path_filter,
        seed_paths,
        max_depth,
        limit,
    )
}

pub(crate) const MAX_BLAST_DEPTH: usize = 5;
const MAX_ENCLOSING_SCOPES: usize = 8;

/// Compute blast radius with exact current-index IDs kept as traversal seeds.
pub fn compute_blast_radius_scoped_with_ids(
    conn: &Connection,
    seed_symbols: &[&str],
    seed_ids: &[&str],
    symbol_path_filter: Option<&str>,
    seed_paths: &[&str],
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, QueryError> {
    blast_radius_walk(
        conn,
        seed_symbols,
        seed_ids,
        symbol_path_filter,
        seed_paths,
        None,
        max_depth,
        limit,
        Some(MAX_BLAST_DEPTH),
    )
}

/// Compute a blast radius and optionally include import, module, and namespace rows.
#[allow(clippy::too_many_arguments)]
pub fn compute_blast_radius_scoped_with_ids_and_low_signal(
    conn: &Connection,
    seed_symbols: &[&str],
    seed_ids: &[&str],
    symbol_path_filter: Option<&str>,
    seed_paths: &[&str],
    max_depth: usize,
    limit: usize,
    include_low_signal: bool,
) -> Result<BlastRadiusResult, QueryError> {
    blast_radius_walk_with_options(
        conn,
        seed_symbols,
        seed_ids,
        symbol_path_filter,
        seed_paths,
        None,
        max_depth,
        limit,
        Some(MAX_BLAST_DEPTH),
        include_low_signal,
        &[],
    )
}

/// Compute blast radius for files or folders, with the tests that the callers reports of their
/// symbols tie to them. Such a test is listed on its own, never in a constructor-only fold.
pub(crate) fn compute_blast_radius_for_files(
    conn: &Connection,
    seed_paths: &[&str],
    max_depth: usize,
    limit: usize,
    include_low_signal: bool,
    report_tests: &[TestTarget],
) -> Result<BlastRadiusResult, QueryError> {
    blast_radius_walk_with_options(
        conn,
        &[],
        &[],
        None,
        seed_paths,
        None,
        max_depth,
        limit,
        Some(MAX_BLAST_DEPTH),
        include_low_signal,
        report_tests,
    )
}

/// The symbols of the files or folders `seed_paths` whose callers report can tie a test that the
/// walk does not follow, such as a read of a decorated command or a module-level object: every
/// function, method, constructor, and type, and every variable or constant at module level,
/// outside tests and documentation, in path and line order.
///
/// The rows stream from one query and `visit` takes one symbol at a time, so no list of them is
/// held. Once `deadline` passes, the query only counts the rest, and the count of symbols that
/// `visit` never took is returned.
pub(crate) fn visit_file_report_targets<E: From<QueryError>>(
    conn: &Connection,
    seed_paths: &[&str],
    deadline: std::time::Instant,
    mut visit: impl FnMut(Symbol) -> Result<(), E>,
) -> Result<usize, E> {
    let mut params = Vec::new();
    let condition = seed_path_condition(seed_paths, &mut params);
    let lifecycle = if has_column(conn, "symbols", "test_lifecycle") {
        "COALESCE(s.test_lifecycle, 0) = 0"
    } else {
        "1"
    };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT s.symbol_id FROM symbols s
             WHERE {condition}
               AND (s.kind IN ('function', 'method', 'constructor')
                    OR s.kind IN {OWNER_KINDS}
                    OR (s.kind IN ('variable', 'constant') AND s.parent_symbol_id IS NULL))
               AND s.is_test = 0 AND s.test_container = 0 AND {lifecycle}
               AND {not_documentation}
               AND NOT {test_path}
             ORDER BY s.path, s.start_line",
            not_documentation = not_documentation(conn, "s"),
            test_path = test_path_predicate("s"),
        ))
        .map_err(QueryError::from)?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(&params))
        .map_err(QueryError::from)?;
    let mut unvisited = 0;
    while let Some(row) = rows.next().map_err(QueryError::from)? {
        if unvisited > 0 || std::time::Instant::now() >= deadline {
            unvisited += 1;
            continue;
        }
        let id: String = row.get(0).map_err(QueryError::from)?;
        if let Some(symbol) = get_symbol_by_id(conn, &id)? {
            visit(symbol)?;
        }
    }
    Ok(unvisited)
}

/// Whether `path` lies in one of the files or folders `seed_paths`, by the rule of
/// [`seed_path_condition`].
pub(crate) fn in_seed_paths(path: &str, seed_paths: &[&str]) -> bool {
    let path = path.replace('\\', "/");
    seed_paths.iter().any(|seed| {
        let seed = seed.replace('\\', "/");
        let seed = seed.trim_start_matches("./").trim_matches('/');
        path.eq_ignore_ascii_case(seed)
            || path.get(..seed.len()).is_some_and(|head| {
                head.eq_ignore_ascii_case(seed) && path[seed.len()..].starts_with('/')
            })
    })
}

/// The condition that selects the symbols of the files or folders `seed_paths`, with its values
/// appended to `params`.
fn seed_path_condition(seed_paths: &[&str], params: &mut Vec<rusqlite::types::Value>) -> String {
    let mut path_conds = Vec::new();
    for p in seed_paths {
        let raw = p
            .replace('\\', "/")
            .trim_start_matches("./")
            .trim_matches('/')
            .to_string();
        let exact_idx = params.len() + 1;
        params.push(rusqlite::types::Value::Text(raw.clone()));
        let dir_pattern = format!("{}/%", escape_like(&raw));
        let like_idx = params.len() + 1;
        params.push(rusqlite::types::Value::Text(dir_pattern));
        path_conds.push(format!(
            "replace(path, '\\', '/') = ?{exact_idx} COLLATE NOCASE OR replace(path, '\\', '/') LIKE ?{like_idx} ESCAPE '\\'"
        ));
    }
    format!("({})", path_conds.join(" OR "))
}

/// Compute blast radius from the callers of a target that only its callers report found, one
/// level below the target, so a walk to the largest depth counts the tests past `max_depth`.
pub(crate) fn compute_blast_radius_from_callers(
    conn: &Connection,
    caller_ids: &[&str],
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, QueryError> {
    blast_radius_walk(
        conn,
        &[],
        caller_ids,
        None,
        &[],
        None,
        max_depth,
        limit,
        Some(MAX_BLAST_DEPTH - 1),
    )
}

pub(crate) fn compute_blast_radius_from_callers_with_low_signal(
    conn: &Connection,
    caller_ids: &[&str],
    max_depth: usize,
    limit: usize,
    include_low_signal: bool,
) -> Result<BlastRadiusResult, QueryError> {
    blast_radius_walk_with_options(
        conn,
        &[],
        caller_ids,
        None,
        &[],
        None,
        max_depth,
        limit,
        Some(MAX_BLAST_DEPTH - 1),
        include_low_signal,
        &[],
    )
}

/// Compute blast radius for the files a diff changed, each with the lines it touched. The walk
/// starts from the innermost symbol that holds each touched line, or from every symbol of a file
/// when a hunk touches only module-level lines, and a test, fixture, or setup member counts as
/// changed only when its lines hold a touched line.
pub fn compute_blast_radius_for_changes(
    conn: &Connection,
    changes: &BTreeMap<String, Vec<(usize, usize)>>,
    max_depth: usize,
    limit: usize,
) -> Result<BlastRadiusResult, QueryError> {
    let seed_paths: Vec<&str> = changes.keys().map(String::as_str).collect();
    blast_radius_walk(
        conn,
        &[],
        &[],
        None,
        &seed_paths,
        Some(changes),
        max_depth,
        limit,
        Some(MAX_BLAST_DEPTH),
    )
}

/// Compute blast radius for changed files, optionally including import, module, and namespace rows.
pub fn compute_blast_radius_for_changes_with_low_signal(
    conn: &Connection,
    changes: &BTreeMap<String, Vec<(usize, usize)>>,
    max_depth: usize,
    limit: usize,
    include_low_signal: bool,
) -> Result<BlastRadiusResult, QueryError> {
    let seed_paths: Vec<&str> = changes.keys().map(String::as_str).collect();
    blast_radius_walk_with_options(
        conn,
        &[],
        &[],
        None,
        &seed_paths,
        Some(changes),
        max_depth,
        limit,
        Some(MAX_BLAST_DEPTH),
        include_low_signal,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn blast_radius_walk(
    conn: &Connection,
    seed_symbols: &[&str],
    seed_ids: &[&str],
    symbol_path_filter: Option<&str>,
    seed_paths: &[&str],
    changed: Option<&BTreeMap<String, Vec<(usize, usize)>>>,
    max_depth: usize,
    limit: usize,
    probe_depth: Option<usize>,
) -> Result<BlastRadiusResult, QueryError> {
    blast_radius_walk_with_options(
        conn,
        seed_symbols,
        seed_ids,
        symbol_path_filter,
        seed_paths,
        changed,
        max_depth,
        limit,
        probe_depth,
        false,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
fn blast_radius_walk_with_options(
    conn: &Connection,
    seed_symbols: &[&str],
    seed_ids: &[&str],
    symbol_path_filter: Option<&str>,
    seed_paths: &[&str],
    changed: Option<&BTreeMap<String, Vec<(usize, usize)>>>,
    max_depth: usize,
    limit: usize,
    probe_depth: Option<usize>,
    include_low_signal: bool,
    report_tests: &[TestTarget],
) -> Result<BlastRadiusResult, QueryError> {
    validate_result_limit(limit)?;
    let max_depth = max_depth.min(MAX_BLAST_DEPTH);
    let mut resolved_seed_symbols = seed_symbols
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
    for id in seed_ids {
        let symbol = get_symbol_by_id(conn, id)?.ok_or_else(|| {
            let workspace = workspace_name(conn);
            QueryError::SymbolNotFound {
                name: format!("symbol id {id}"),
                workspace,
                hint: "Run lookup_symbol again and select a current id.".to_string(),
            }
        })?;
        resolved_seed_symbols.push(symbol);
    }
    let mut seed_names = Vec::new();
    let seed_type = if (!seed_symbols.is_empty() || !seed_ids.is_empty()) && !seed_paths.is_empty()
    {
        for s in seed_symbols {
            seed_names.push(s.to_string());
        }
        for id in seed_ids {
            seed_names.push(id.to_string());
        }
        for p in seed_paths {
            seed_names.push(p.to_string());
        }
        "mixed".to_string()
    } else if !seed_symbols.is_empty() || !seed_ids.is_empty() {
        for s in seed_symbols {
            seed_names.push(s.to_string());
        }
        for id in seed_ids {
            seed_names.push(id.to_string());
        }
        "symbol".to_string()
    } else if !seed_paths.is_empty() {
        for p in seed_paths {
            seed_names.push(p.to_string());
        }
        "file".to_string()
    } else {
        return Ok(BlastRadiusResult {
            seed_type: "none".to_string(),
            seeds: Vec::new(),
            likely_tests: Vec::new(),
            impacted_symbols: Vec::new(),
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            traversal_ceiling_reached: false,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
            stopped_at_depth: None,
            callers_report: None,
            open_ends: Vec::new(),
            tests_past_depth: None,
            tests_past_depth_cut: false,
            past_depth_tests: Vec::new(),
            report_symbols_unchecked: None,
            report_symbols_failed: None,
            report_sites_unchecked: None,
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
        where_clauses.push(seed_path_condition(seed_paths, &mut params_vec));
    }

    let seed_condition = where_clauses.join(" OR ");

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

    let seeds = WalkSeeds {
        condition: &seed_condition,
        params: &params_vec,
        target_ids: resolved_seed_symbols
            .iter()
            .map(|symbol| symbol.symbol_id.as_str())
            .collect(),
        changed,
        file_seed: seed_type == "file",
        report_tests,
    };
    let mut walk = CallerWalk::start(
        conn,
        has_relationships,
        has_pending,
        &seeds,
        include_low_signal,
    )?;
    walk.extend(conn, walk_bound(max_depth))?;
    let collected = collect_walk(conn, &walk, &seeds, max_depth, include_low_signal)?;
    let mut likely_tests = collected.likely_tests;
    let mut impacted_symbols = collected.impacted_symbols;
    let mut reached = collected.reached;
    let mut tests_past_depth = None;
    let mut tests_past_depth_cut = false;
    let mut past_depth_tests = Vec::new();
    if let Some(probe_depth) =
        probe_depth.filter(|&depth| collected.callers_beyond_depth && depth > max_depth)
    {
        walk.extend(conn, walk_bound(probe_depth))?;
        let deeper = collect_walk(conn, &walk, &seeds, probe_depth, include_low_signal)?;
        reached = deeper.reached;
        let listed: HashSet<(&str, usize)> = likely_tests
            .iter()
            .map(|test| (test.path.as_str(), test.line))
            .collect();
        past_depth_tests = deeper
            .likely_tests
            .iter()
            .filter(|test| !listed.contains(&(test.path.as_str(), test.line)))
            .map(|test| (test.path.clone(), test.line))
            .collect();
        tests_past_depth = Some(past_depth_tests.len());
        tests_past_depth_cut = deeper.traversal_ceiling_reached;
    }
    let open_ends = if probe_depth.is_none() {
        Vec::new()
    } else {
        walk.open_ends(conn, reached)?
    };
    let likely_tests_found = likely_tests.len();
    let impacted_symbols_found = impacted_symbols.len();
    let likely_tests_truncated = likely_tests.len() > limit;
    let impacted_symbols_truncated = impacted_symbols.len() > limit;
    if likely_tests.len() > limit {
        likely_tests.truncate(limit);
    }
    if impacted_symbols.len() > limit {
        impacted_symbols.truncate(limit);
    }

    Ok(BlastRadiusResult {
        seed_type,
        seeds: seed_names,
        likely_tests,
        impacted_symbols,
        likely_tests_truncated,
        impacted_symbols_truncated,
        traversal_ceiling_reached: collected.traversal_ceiling_reached,
        limit_at_maximum: limit >= MAX_RESULT_LIMIT,
        likely_tests_found,
        impacted_symbols_found,
        stopped_at_depth: collected.callers_beyond_depth.then_some(max_depth),
        callers_report: None,
        open_ends,
        tests_past_depth,
        tests_past_depth_cut,
        past_depth_tests,
        report_symbols_unchecked: None,
        report_symbols_failed: None,
        report_sites_unchecked: None,
    })
}

/// The project root that the index records, or an empty path when it records none.
fn index_root(conn: &Connection) -> std::path::PathBuf {
    conn.query_row(
        "SELECT value FROM artifact_metadata WHERE key = 'root_path'",
        [],
        |row| row.get::<_, String>(0),
    )
    .map(std::path::PathBuf::from)
    .unwrap_or_default()
}

/// The walk depth for a `max_depth`: one level past it tells a walk that a deeper call would
/// extend from one that ran out.
fn walk_bound(max_depth: usize) -> usize {
    if max_depth < MAX_BLAST_DEPTH {
        max_depth + 1
    } else {
        max_depth
    }
}

/// The seeds of one walk: the SQL condition that selects them and how each one came in.
struct WalkSeeds<'a> {
    condition: &'a str,
    params: &'a [rusqlite::types::Value],
    target_ids: HashSet<&'a str>,
    changed: Option<&'a BTreeMap<String, Vec<(usize, usize)>>>,
    file_seed: bool,
    report_tests: &'a [TestTarget],
}

/// What a walk reached up to one depth, before `limit` cuts the lists.
struct CollectedWalk {
    likely_tests: Vec<TestTarget>,
    impacted_symbols: Vec<ImpactedSymbol>,
    traversal_ceiling_reached: bool,
    callers_beyond_depth: bool,
    reached: Vec<ReachedSymbol>,
}

/// The symbols that a caller walk reached, found level by level: each symbol once per `via` at
/// its least depth, so a symbol that several paths reach is expanded once. `via` 1 marks a row
/// that only a constructor step reached.
struct CallerWalk {
    level_sql: Option<String>,
    legacy_level_sql: Option<String>,
    rows: Vec<(String, usize, i64)>,
    seen: HashSet<(String, i64, bool)>,
    frontier: Vec<(String, usize, i64)>,
    depth: usize,
    expanded: HashSet<String>,
    called: HashSet<String>,
}

impl CallerWalk {
    fn start(
        conn: &Connection,
        has_relationships: bool,
        has_pending: bool,
        seeds: &WalkSeeds,
        include_low_signal: bool,
    ) -> Result<Self, QueryError> {
        let build_level_sql = |include_typed_receiver_chain| {
            let branches = walk_caller_branches(
                conn,
                has_relationships,
                has_pending,
                "?1",
                include_low_signal,
                include_typed_receiver_chain,
            );
            (!branches.is_empty()).then(|| {
                format!(
                    "WITH impact_walk(symbol_id, depth, via) AS ({WALK_ROWS_SQL})
                     {}",
                    branches.join("\n UNION \n")
                )
            })
        };
        let level_sql = build_level_sql(true);
        let legacy_level_sql = build_level_sql(false);
        let mut walk = CallerWalk {
            level_sql,
            legacy_level_sql,
            rows: Vec::new(),
            seen: HashSet::new(),
            frontier: Vec::new(),
            depth: 0,
            expanded: HashSet::new(),
            called: HashSet::new(),
        };
        let spans = seed_spans(conn, seeds.condition, seeds.params)?;
        let ids = match seeds.changed {
            Some(changed) => changed_symbols(spans, changed).0,
            None => spans.into_iter().map(|span| span.symbol_id).collect(),
        };
        for id in ids {
            if walk.seen.insert((id.clone(), 0, false)) {
                walk.rows.push((id.clone(), 0, 0));
                walk.frontier.push((id, 0, 0));
            }
        }
        Ok(walk)
    }

    /// Walks on until `bound` levels are done or no level adds a row.
    fn extend(&mut self, conn: &Connection, bound: usize) -> Result<(), QueryError> {
        let (Some(full_sql), Some(legacy_sql)) = (&self.level_sql, &self.legacy_level_sql) else {
            return Ok(());
        };
        if self.depth >= bound || self.frontier.is_empty() {
            return Ok(());
        }
        let mut full_stmt = None;
        let mut legacy_stmt = None;
        let mut overloads = crate::reference_report::overloads::OverloadSteps::new(conn);
        let root = index_root(conn);
        let mut includes = crate::reference_report::include_reach::IncludeSteps::new(conn, &root);
        while self.depth < bound && !self.frontier.is_empty() {
            let frontier = std::mem::take(&mut self.frontier);
            let needs_full = pending_receiver_chain_requires_full_for_frontier(conn, &frontier);
            let steps = if needs_full {
                if full_stmt.is_none() {
                    full_stmt = Some(conn.prepare(full_sql)?);
                }
                let owners = pending_receiver_chain_owners(conn, &frontier)?;
                level_callers(
                    full_stmt.as_mut().expect("prepared full walk"),
                    bound,
                    &frontier,
                    Some(&owners),
                )?
            } else {
                if legacy_stmt.is_none() {
                    legacy_stmt = Some(conn.prepare(legacy_sql)?);
                }
                level_callers(
                    legacy_stmt.as_mut().expect("prepared legacy walk"),
                    bound,
                    &frontier,
                    None,
                )?
            };
            for step in steps {
                let Some(caller) = step.caller else {
                    continue;
                };
                if !overloads.keeps(&caller, &step.origin) || !includes.keeps(&caller, &step.origin)
                {
                    continue;
                }
                if caller != step.origin {
                    self.called.insert(step.origin);
                }
                if self.seen.insert((caller.clone(), step.via, true)) {
                    self.rows.push((caller.clone(), step.depth, step.via));
                    self.frontier.push((caller, step.depth, step.via));
                }
            }
            self.expanded
                .extend(frontier.into_iter().map(|(symbol_id, _, _)| symbol_id));
            self.depth += 1;
        }
        Ok(())
    }

    /// The reached symbols that no caller step of the walk leaves, other than through a call to
    /// themselves: the runtime, a framework, a library, or a call the index cannot tie may call
    /// them, or nothing does.
    fn open_ends(
        &mut self,
        conn: &Connection,
        reached: Vec<ReachedSymbol>,
    ) -> Result<Vec<OpenEnd>, QueryError> {
        let (Some(full_sql), Some(legacy_sql)) = (&self.level_sql, &self.legacy_level_sql) else {
            return Ok(Vec::new());
        };
        let unexpanded: Vec<(String, usize, i64)> = reached
            .iter()
            .filter(|symbol| !self.expanded.contains(&symbol.symbol_id))
            .map(|symbol| (symbol.symbol_id.clone(), symbol.depth, 0))
            .collect();
        if !unexpanded.is_empty() {
            let (sql, owners) =
                if pending_receiver_chain_requires_full_for_frontier(conn, &unexpanded) {
                    (
                        full_sql,
                        Some(pending_receiver_chain_owners(conn, &unexpanded)?),
                    )
                } else {
                    (legacy_sql, None)
                };
            let mut stmt = conn.prepare(sql)?;
            let mut overloads = crate::reference_report::overloads::OverloadSteps::new(conn);
            let root = index_root(conn);
            let mut includes =
                crate::reference_report::include_reach::IncludeSteps::new(conn, &root);
            for step in level_callers(
                &mut stmt,
                MAX_BLAST_DEPTH + 1,
                &unexpanded,
                owners.as_deref(),
            )? {
                if step.caller.is_some_and(|caller| {
                    caller != step.origin
                        && overloads.keeps(&caller, &step.origin)
                        && includes.keeps(&caller, &step.origin)
                }) {
                    self.called.insert(step.origin);
                }
            }
        }
        let mut owner = conn.prepare(&format!(
            "SELECT name FROM symbols WHERE symbol_id = ?1 AND kind IN {OWNER_KINDS}"
        ))?;
        let mut seen = HashSet::new();
        let mut open_ends = Vec::new();
        for symbol in reached {
            if !seen.insert(symbol.symbol_id.clone()) || self.called.contains(&symbol.symbol_id) {
                continue;
            }
            let owner = match symbol.parent.as_deref() {
                Some(parent) => owner
                    .query_row(params![parent], |row| row.get::<_, String>(0))
                    .optional()?,
                None => None,
            };
            open_ends.push(OpenEnd {
                name: symbol.name,
                kind: symbol.kind,
                path: symbol.path,
                line: symbol.line,
                depth: symbol.depth,
                owner,
            });
        }
        Ok(open_ends)
    }

    fn rows_json(&self) -> String {
        walk_rows_json(&self.rows)
    }
}

/// The rows of `impact_walk` from a JSON array of `[symbol_id, depth, via]` in `?2`.
const WALK_ROWS_SQL: &str = "SELECT json_extract(value, '$[0]'), json_extract(value, '$[1]'),
                                    json_extract(value, '$[2]')
                             FROM json_each(?2)";

fn walk_rows_json(rows: &[(String, usize, i64)]) -> String {
    serde_json::Value::Array(
        rows.iter()
            .map(|(symbol_id, depth, via)| serde_json::json!([symbol_id, depth, via]))
            .collect(),
    )
    .to_string()
}

/// The lines of one seed symbol. A `value` (a constant, an enum member, an export) has no
/// callers of its own for the walk to follow.
struct SeedSpan {
    symbol_id: String,
    path: String,
    start_line: usize,
    end_line: usize,
    value: bool,
}

/// The symbols a diff changed: for each hunk, the innermost symbol that holds each of its lines,
/// then every other symbol of a file where only a value or no symbol holds a hunk's lines, such
/// as a changed import or module-level constant. The count is how many innermost holders lead.
fn changed_symbols(
    spans: Vec<SeedSpan>,
    changed: &BTreeMap<String, Vec<(usize, usize)>>,
) -> (Vec<String>, usize) {
    let mut by_path: BTreeMap<&str, Vec<&SeedSpan>> = BTreeMap::new();
    for span in &spans {
        by_path.entry(span.path.as_str()).or_default().push(span);
    }
    let mut holders = Vec::new();
    let mut whole_files = Vec::new();
    let mut seen = HashSet::new();
    for (path, file_spans) in by_path {
        let hunks = changed.get(path).map(Vec::as_slice).unwrap_or_default();
        let mut whole_file = hunks.is_empty();
        for &(first, last) in hunks {
            let held: Vec<&SeedSpan> = (first..=last)
                .filter_map(|line| {
                    file_spans
                        .iter()
                        .filter(|span| {
                            !span.value && span.start_line <= line && line <= span.end_line
                        })
                        .min_by_key(|span| span.end_line - span.start_line)
                        .copied()
                })
                .collect();
            whole_file |= held.is_empty();
            for span in held {
                if seen.insert(span.symbol_id.as_str()) {
                    holders.push(span.symbol_id.clone());
                }
            }
        }
        if whole_file {
            whole_files.extend(file_spans);
        }
    }
    let held = holders.len();
    for span in whole_files {
        if seen.insert(span.symbol_id.as_str()) {
            holders.push(span.symbol_id.clone());
        }
    }
    (holders, held)
}

fn seed_spans(
    conn: &Connection,
    condition: &str,
    params: &[rusqlite::types::Value],
) -> Result<Vec<SeedSpan>, QueryError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT symbol_id, replace(path, '\\', '/'), start_line, end_line,
                kind IN ('constant', 'enum_member', 'export')
         FROM symbols
         WHERE ({condition}) AND kind NOT IN ({LOW_SIGNAL_KINDS_SQL})
         ORDER BY path, start_line"
    ))?;
    let spans = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(SeedSpan {
            symbol_id: row.get(0)?,
            path: row.get(1)?,
            start_line: row.get::<_, i64>(2)? as usize,
            end_line: row.get::<_, i64>(3)? as usize,
            value: row.get(4)?,
        })
    })?;
    Ok(spans.collect::<Result<Vec<_>, _>>()?)
}

/// Whether a member read of `target`'s name (`cls.handle_error` with no call) sits outside
/// `target` and outside every symbol that starts at a `listed` path and line. The walk follows
/// calls only, so only the callers report can tie such a read to the target.
pub(crate) fn has_member_read_outside(
    conn: &Connection,
    target: &Symbol,
    listed: &HashSet<(String, usize)>,
) -> Result<bool, QueryError> {
    if !has_column(conn, "identifiers", "containing_symbol_id") {
        return Ok(false);
    }
    let mut stmt = conn.prepare(
        "SELECT replace(c.path, '\\', '/'), c.start_line
         FROM identifiers i
         LEFT JOIN symbols c ON c.symbol_id = i.containing_symbol_id
         WHERE i.name = ?1 AND i.kind = 'member_access'
           AND (i.containing_symbol_id IS NULL OR i.containing_symbol_id != ?2)",
    )?;
    let mut rows = stmt.query(params![target.name, target.symbol_id])?;
    while let Some(row) = rows.next()? {
        let holder = row
            .get::<_, Option<String>>(0)?
            .zip(row.get::<_, Option<i64>>(1)?)
            .map(|(path, line)| (path, line as usize));
        if !holder.is_some_and(|holder| listed.contains(&holder)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Diffs that change more functions than this get no callers report for each one.
const MAX_DIFF_TARGETS: usize = 5;

/// The functions, methods, and constructors outside tests that hold a line of the diff, each the
/// innermost symbol that holds it; none when there are more than [`MAX_DIFF_TARGETS`].
pub(crate) fn diff_targets(
    conn: &Connection,
    changed: &BTreeMap<String, Vec<(usize, usize)>>,
) -> Result<Vec<Symbol>, QueryError> {
    let mut spans = Vec::new();
    for path in changed.keys() {
        spans.extend(seed_spans(
            conn,
            "replace(path, '\\', '/') = ?1",
            &[rusqlite::types::Value::Text(path.clone())],
        )?);
    }
    let (ids, held) = changed_symbols(spans, changed);
    let mut targets = Vec::new();
    for id in &ids[..held] {
        let Some(symbol) = get_symbol_by_id(conn, id)? else {
            continue;
        };
        if can_be_open_end(&symbol.kind)
            && !symbol.is_test
            && !symbol.test_container
            && !is_test_path(&symbol.path)
        {
            targets.push(symbol);
        }
    }
    if targets.len() > MAX_DIFF_TARGETS {
        targets.clear();
    }
    Ok(targets)
}

/// One caller step of a walk level: the caller, its depth and `via`, and the row it leaves.
struct CallerStep {
    caller: Option<String>,
    depth: usize,
    via: i64,
    origin: String,
}

/// The caller steps that leave each row of `frontier`. A statement with the typed receiver-chain
/// rule also takes `receiver_chain_owners`, the map of [`pending_receiver_chain_owners`].
fn level_callers(
    stmt: &mut rusqlite::Statement,
    bound: usize,
    frontier: &[(String, usize, i64)],
    receiver_chain_owners: Option<&str>,
) -> Result<Vec<CallerStep>, QueryError> {
    let map_row = |row: &rusqlite::Row<'_>| {
        Ok(CallerStep {
            caller: row.get(0)?,
            depth: row.get::<_, i64>(1)? as usize,
            via: row.get(2)?,
            origin: row.get(3)?,
        })
    };
    let frontier_json = walk_rows_json(frontier);
    let rows = match receiver_chain_owners {
        Some(owners) if stmt.parameter_count() >= 3 => {
            stmt.query_map(params![bound as i64, frontier_json, owners], map_row)?
        }
        _ => stmt.query_map(params![bound as i64, frontier_json], map_row)?,
    };
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The receiver-chain owner of the pending row `p`, read from the JSON map bound as `param`.
fn bound_receiver_chain_owner_sql(param: &str) -> String {
    format!("json_extract({param}, '$.\"' || p.rowid || '\"')")
}

/// The receiver-chain owners of the pending rows `p` that `joins` and `filter` select, as a JSON
/// object from the pending row id to the owner id, computed with the owner query of
/// [`pending_target_predicate`]. `ctes` are common table expressions for `joins`, each followed
/// by a comma. It holds only calls whose chain has two or more names, the only calls the typed
/// receiver-chain rule reads, and leaves out a call with no owner.
fn receiver_chain_owner_map(
    conn: &Connection,
    ctes: &str,
    joins: &str,
    filter: &str,
    params: impl rusqlite::Params,
) -> Result<String, QueryError> {
    let sql = format!(
        "WITH {ctes} owners(pending_id, owner_id) AS (
             SELECT CAST(p.rowid AS TEXT), {owner}
             FROM pending_relationships p
             {joins}
             LEFT JOIN symbols s_from ON s_from.symbol_id = p.from_symbol_id
             WHERE ({filter})
               AND json_array_length({PENDING_RECEIVER_CHAIN_JSON}) > 1
               AND p.target_receiver IS NOT NULL AND p.target_receiver != ''
         )
         SELECT json_group_object(pending_id, owner_id)
         FROM owners WHERE owner_id IS NOT NULL",
        owner = pending_receiver_chain_owner_sql(),
    );
    Ok(conn.query_row(&sql, params, |row| row.get(0))?)
}

/// The receiver-chain owners of the pending calls that can reach a row of `frontier`: the calls
/// to the name of a frontier symbol, or of the class of a frontier constructor.
fn pending_receiver_chain_owners(
    conn: &Connection,
    frontier: &[(String, usize, i64)],
) -> Result<String, QueryError> {
    let frontier_ids: Vec<&str> = frontier
        .iter()
        .map(|(symbol_id, _, _)| symbol_id.as_str())
        .collect();
    let frontier_json = serde_json::to_string(&frontier_ids).unwrap_or_else(|_| "[]".into());
    receiver_chain_owner_map(
        conn,
        "frontier_ids(symbol_id) AS (
             SELECT value FROM json_each(?1)
         ), target_names(name) AS (
             SELECT s.name
             FROM frontier_ids f JOIN symbols s ON s.symbol_id = f.symbol_id
             UNION
             SELECT parent.name
             FROM frontier_ids f
             JOIN symbols ctor ON ctor.symbol_id = f.symbol_id AND ctor.kind = 'constructor'
             JOIN symbols parent ON parent.symbol_id = ctor.parent_symbol_id
             WHERE parent.kind IN ('class', 'struct', 'record')
         ),",
        "JOIN target_names target ON target.name = p.target_terminal_name",
        "1",
        [&frontier_json],
    )
}

fn collect_walk(
    conn: &Connection,
    walk: &CallerWalk,
    seeds: &WalkSeeds,
    max_depth: usize,
    include_low_signal: bool,
) -> Result<CollectedWalk, QueryError> {
    let mut traversal_ceiling_reached = false;
    let mut callers_beyond_depth = false;
    let mut likely_tests = Vec::new();
    let mut impacted_symbols = Vec::new();
    let mut seen_test_keys = HashSet::new();

    let lifecycle = if has_column(conn, "symbols", "test_lifecycle") {
        "COALESCE(s.test_lifecycle, 0) != 0"
    } else {
        "0"
    };
    let mut fixtures = Vec::new();
    let mut setups = Vec::new();
    let mut reached = Vec::new();
    let mut constructor_only_test_keys = HashSet::new();
    let mut non_constructor_test_keys = HashSet::new();
    let creators = anonymous_class_creators(conn, &walk.rows_json())?;
    for seed in seed_rows(conn, seeds.condition, seeds.params, lifecycle)? {
        let origin = if seeds.target_ids.contains(seed.symbol_id.as_str()) {
            SeedOrigin::Target
        } else if let Some(changed) = seeds.changed {
            let lines = changed
                .get(&seed.path)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if !lines
                .iter()
                .any(|&(first, last)| first <= seed.end_line && seed.start_line <= last)
            {
                continue;
            }
            SeedOrigin::Diff
        } else {
            SeedOrigin::TargetFile
        };
        let callable = matches!(seed.kind.as_str(), "function" | "method");
        let is_test_target = seed.is_test
            || seed.test_container
            || seed.is_fixture
            || (callable && is_test_path(&seed.path) && names_a_test(&seed.name));
        if !is_test_target && can_be_open_end(&seed.kind) {
            reached.push(ReachedSymbol {
                symbol_id: seed.symbol_id.clone(),
                name: seed.name.clone(),
                kind: seed.kind.clone(),
                path: seed.path.clone(),
                line: seed.start_line,
                depth: 0,
                parent: seed.parent.clone(),
            });
        }
        if !is_test_target || (origin != SeedOrigin::Target && !callable) {
            continue;
        }
        for test in seed_tests(conn, &seed, origin)? {
            if seen_test_keys.insert(format!("{}:{}", test.path, test.line)) {
                non_constructor_test_keys.insert((test.path.clone(), test.line));
                likely_tests.push(test);
            }
        }
    }
    if walk.level_sql.is_some() {
        let not_documentation = not_documentation(conn, "s");
        // A seed that the walk reaches again counts only as a test to run: a test in a changed test
        // file that calls changed code. Its own change is the seed step's to report.
        let reached_depth = "MIN(CASE WHEN iw.depth > 0 THEN iw.depth END)";
        let reached_via = "MIN(CASE WHEN iw.depth > 0 THEN iw.via END)";
        let test_path = test_path_predicate("s");
        let test_name = names_a_test_sql("s");
        let walk_rows = WALK_ROWS_SQL.replace("?2", "?1");
        let low_signal_filter = if include_low_signal {
            "1".to_string()
        } else {
            format!("s.kind NOT IN ({LOW_SIGNAL_KINDS_SQL})")
        };
        let sql = format!(
            "WITH impact_walk(symbol_id, depth, via) AS ({walk_rows})
            SELECT s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container,
                   {reached_depth} as min_depth, {reached_via} as min_via,
                   {lifecycle} AS is_fixture, s.parent_symbol_id,
                   EXISTS (SELECT 1 FROM symbols owner
                           WHERE owner.symbol_id = s.parent_symbol_id
                             AND COALESCE(owner.test_container, 0) != 0) AS in_test_class,
                   s.language, s.signature, MIN(iw.depth) = 0 AS is_seed, s.symbol_id,
                   EXISTS (SELECT 1 FROM symbols owner
                           WHERE owner.symbol_id = s.parent_symbol_id AND {anonymous_owner})
                       AS in_anonymous_class
            FROM impact_walk iw
            CROSS JOIN symbols s ON iw.symbol_id = s.symbol_id
            WHERE {low_signal_filter}
              AND {not_documentation}
            GROUP BY s.symbol_id, s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container
            HAVING {reached_depth} IS NOT NULL
               AND (MIN(iw.depth) > 0 OR s.is_test != 0 OR s.test_container != 0 OR is_fixture
                    OR ({test_path} AND (in_test_class OR {test_name})))
            ORDER BY {reached_depth} > {max_depth} ASC, {reached_via} ASC, min_depth ASC, s.path ASC, s.name ASC
            LIMIT 201",
            anonymous_owner = anonymous_class_predicate("owner"),
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![walk.rows_json()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? as usize,
                row.get::<_, bool>(4)?,
                row.get::<_, bool>(5)?,
                row.get::<_, i64>(6)? as usize,
                row.get::<_, i64>(7)? as usize,
                row.get::<_, bool>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, bool>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, bool>(13)?,
                row.get::<_, String>(14)?,
                row.get::<_, bool>(15)?,
            ))
        })?;

        for (index, r) in rows.enumerate() {
            let (
                name,
                kind,
                raw_path,
                line,
                is_test,
                test_container,
                depth,
                min_via,
                is_fixture,
                parent,
                in_test_class,
                language,
                signature,
                is_seed,
                symbol_id,
                in_anonymous_class,
            ) = r?;
            if depth > max_depth {
                callers_beyond_depth = true;
                break;
            }
            if index == 200 {
                traversal_ceiling_reached = true;
                break;
            }
            let path = raw_path.replace('\\', "/");
            let is_test_target = !in_anonymous_class
                && (is_test
                    || test_container
                    || is_fixture
                    || (is_test_path(&path) && (in_test_class || names_a_test(&name))));
            if is_seed && !is_test_target {
                continue;
            }
            let constructor_only = min_via > 0;
            if is_test_target {
                let key = format!("{}:{}", path, line);
                let test_key = (path.clone(), line);
                if constructor_only {
                    constructor_only_test_keys.insert(test_key);
                } else {
                    non_constructor_test_keys.insert(test_key);
                }
                if seen_test_keys.insert(key) {
                    let pytest_fixture = is_pytest_fixture(&name, &path);
                    let caller = if depth == 1 {
                        "direct caller".to_string()
                    } else {
                        format!("indirect caller [depth {depth}]")
                    };
                    let reason = if is_fixture && pytest_fixture {
                        fixtures.push((name.clone(), path.clone(), line, constructor_only));
                        format!("fixture ({caller})")
                    } else if is_fixture {
                        if let Some(class_id) = parent {
                            setups.push((
                                name.clone(),
                                class_id,
                                path.clone(),
                                line,
                                constructor_only,
                            ));
                        }
                        format!("setup ({caller})")
                    } else {
                        caller
                    };
                    likely_tests.push(TestTarget {
                        name,
                        path,
                        line,
                        reason,
                    });
                }
            } else if let Some(test) = enclosing_walk_test(conn, parent.as_deref())? {
                let test_key = (test.path.clone(), test.line);
                if constructor_only {
                    constructor_only_test_keys.insert(test_key);
                } else {
                    non_constructor_test_keys.insert(test_key);
                }
                if seen_test_keys.insert(format!("{}:{}", test.path, test.line)) {
                    let caller = if depth == 1 {
                        "direct caller".to_string()
                    } else {
                        format!("indirect caller [depth {depth}]")
                    };
                    let nested = if is_generated_lambda_name(
                        &name,
                        &language,
                        &kind,
                        signature.as_deref(),
                    ) {
                        "<lambda>"
                    } else {
                        name.as_str()
                    };
                    likely_tests.push(TestTarget {
                        reason: format!("encloses `{nested}` ({caller})"),
                        ..test
                    });
                }
            } else {
                let reason = creators
                    .get(&symbol_id)
                    .map(|anonymous| format!("creates `{anonymous}`"));
                if can_be_open_end(&kind) {
                    reached.push(ReachedSymbol {
                        symbol_id,
                        name: name.clone(),
                        kind: kind.clone(),
                        path: path.clone(),
                        line,
                        depth,
                        parent: parent.clone(),
                    });
                }
                let enclosing_symbol_name =
                    if is_generated_lambda_name(&name, &language, &kind, signature.as_deref()) {
                        match parent.as_deref() {
                            Some(parent_id) => {
                                get_symbol_by_id(conn, parent_id)?.map(|parent| parent.name)
                            }
                            None => None,
                        }
                    } else {
                        None
                    };
                impacted_symbols.push(ImpactedSymbol {
                    name,
                    kind,
                    path,
                    line,
                    depth,
                    enclosing_symbol_name,
                    reason,
                });
            }
        }
    }

    // A fixture or setup member is not a test to run; the tests it serves stand in for it.
    let mut replaced = HashSet::new();
    for (fixture, fixture_path, line, constructor_only) in &fixtures {
        let shown = fixture_name(conn, fixture, fixture_path);
        let users = fixture_users(conn, fixture, fixture_path)?;
        if !users.is_empty() {
            replaced.insert((fixture_path.clone(), *line));
        }
        for test in users {
            let test_key = (test.path.clone(), test.line);
            if *constructor_only {
                constructor_only_test_keys.insert(test_key);
            } else {
                non_constructor_test_keys.insert(test_key);
            }
            if seen_test_keys.insert(format!("{}:{}", test.path, test.line)) {
                likely_tests.push(TestTarget {
                    reason: format!("uses fixture `{shown}`"),
                    ..test
                });
            }
        }
    }

    for (setup, class_id, setup_path, line, constructor_only) in &setups {
        let tests = tests_in_class(conn, class_id)?;
        if !tests.is_empty() {
            replaced.insert((setup_path.clone(), *line));
        }
        for test in tests {
            let test_key = (test.path.clone(), test.line);
            if *constructor_only {
                constructor_only_test_keys.insert(test_key);
            } else {
                non_constructor_test_keys.insert(test_key);
            }
            if seen_test_keys.insert(format!("{}:{}", test.path, test.line)) {
                likely_tests.push(TestTarget {
                    reason: format!("setup `{setup}` runs before it"),
                    ..test
                });
            }
        }
    }
    likely_tests.retain(|test| !replaced.contains(&(test.path.clone(), test.line)));
    for test in seeds.report_tests {
        let key = (test.path.clone(), test.line);
        let constructor_only =
            constructor_only_test_keys.contains(&key) && !non_constructor_test_keys.contains(&key);
        non_constructor_test_keys.insert(key);
        if seen_test_keys.insert(format!("{}:{}", test.path, test.line)) {
            likely_tests.push(test.clone());
        } else if constructor_only
            && let Some(row) = likely_tests
                .iter_mut()
                .find(|row| row.path == test.path && row.line == test.line)
        {
            row.reason = test.reason.clone();
        }
    }
    constructor_only_test_keys.retain(|key| !non_constructor_test_keys.contains(key));

    qualify_test_methods(conn, &mut likely_tests)?;
    // A whole-file target often reaches many unrelated tests only because they construct an app;
    // keep that coverage visible while collapsing each affected test file to one run target.
    if seeds.file_seed {
        let mut constructor_tests_by_file =
            std::collections::BTreeMap::<String, Vec<TestTarget>>::new();
        let mut other_tests = Vec::with_capacity(likely_tests.len());
        for test in likely_tests.drain(..) {
            if constructor_only_test_keys.contains(&(test.path.clone(), test.line)) {
                constructor_tests_by_file
                    .entry(test.path.clone())
                    .or_default()
                    .push(test);
            } else {
                other_tests.push(test);
            }
        }
        for (path, mut tests) in constructor_tests_by_file {
            if tests.len() > 1 {
                let count = tests.len();
                other_tests.push(TestTarget {
                    name: path.clone(),
                    path,
                    line: 1,
                    reason: format!("{count} tests reach it only through a constructor"),
                });
            } else if let Some(test) = tests.pop() {
                other_tests.push(test);
            }
        }
        likely_tests = other_tests;
    }
    Ok(CollectedWalk {
        likely_tests,
        impacted_symbols,
        traversal_ceiling_reached,
        callers_beyond_depth,
        reached,
    })
}

/// A symbol the walk reached whose callers it may not have seen.
struct ReachedSymbol {
    symbol_id: String,
    name: String,
    kind: String,
    path: String,
    line: usize,
    depth: usize,
    parent: Option<String>,
}

fn can_be_open_end(kind: &str) -> bool {
    matches!(kind, "function" | "method" | "constructor")
}

/// The caller steps of a walk over `impact_walk`: each selects the caller, its depth and `via`,
/// and the row it leaves.
fn walk_caller_branches(
    conn: &Connection,
    has_relationships: bool,
    has_pending: bool,
    depth_bound: &str,
    include_low_signal: bool,
    include_typed_receiver_chain: bool,
) -> Vec<String> {
    let mut recursive_branches = Vec::new();
    let from_signal_filter = if include_low_signal {
        String::new()
    } else {
        format!("AND s_from.kind NOT IN ({LOW_SIGNAL_KINDS_SQL})")
    };
    let target_signal_filter = if include_low_signal {
        String::new()
    } else {
        format!("AND s_target.kind NOT IN ({LOW_SIGNAL_KINDS_SQL})")
    };
    let call_language_filter =
        call_languages_compatible_sql("s_from.language", "s_target.language");

    if has_relationships {
        recursive_branches.push(format!(
            "SELECT r.from_symbol_id, iw.depth + 1, iw.via, iw.symbol_id
             FROM relationships r
             JOIN impact_walk iw ON r.to_symbol_id = iw.symbol_id
             JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
             JOIN symbols s_target ON r.to_symbol_id = s_target.symbol_id
             WHERE iw.depth < {depth_bound}
               AND (r.kind != 'calls' OR {call_language_filter})
               {from_signal_filter}"
        ));
    }

    if has_pending {
        let has_target_namespace = has_pending_namespace_column(conn);
        let pending_language_filter =
            pending_relationship_language_filter("p.kind", "s_from", "s_target");
        let parent_join = if has_target_namespace {
            "LEFT JOIN symbols s_target_parent ON s_target.parent_symbol_id = s_target_parent.symbol_id"
        } else {
            ""
        };
        let ns_condition = if has_target_namespace {
            let predicate = if include_typed_receiver_chain {
                pending_target_predicate_with_receiver_chain_owner(
                    conn,
                    "s_target",
                    "s_target_parent",
                    &bound_receiver_chain_owner_sql("?3"),
                )
            } else {
                pending_target_predicate_without_typed_receiver_chain(
                    conn,
                    "s_target",
                    "s_target_parent",
                )
            };
            format!("AND {pred}", pred = predicate)
        } else {
            String::new()
        };

        recursive_branches.push(format!(
            "SELECT p.from_symbol_id, iw.depth + 1, iw.via, iw.symbol_id
             FROM pending_relationships p
             JOIN symbols s_target ON p.target_terminal_name = s_target.name
             JOIN impact_walk iw ON s_target.symbol_id = iw.symbol_id
             JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
             {parent_join}
             WHERE iw.depth < {depth_bound}
               AND {pending_language_filter}
               {target_signal_filter}
               {ns_condition}"
        ));
    }

    // julie records no namespace that a macro opens (`NLOHMANN_JSON_NAMESPACE_BEGIN`) and keeps
    // the functions and types inside it top-level, so a C or C++ call qualified only by project
    // namespaces (`detail::input_adapter()`) can reach a top-level one. The include check of the
    // walk then keeps only a file that the caller's file can see.
    if has_pending
        && has_pending_namespace_column(conn)
        && has_column(conn, "pending_relationships", "target_receiver")
    {
        let qualifiers = "json_insert(
            CASE WHEN json_valid(p.target_namespace_json) THEN p.target_namespace_json ELSE '[]' END,
            '$[#]', p.target_receiver)";
        recursive_branches.push(format!(
            "SELECT p.from_symbol_id, iw.depth + 1, iw.via, iw.symbol_id
             FROM pending_relationships p
             JOIN symbols s_target ON p.target_terminal_name = s_target.name
             JOIN impact_walk iw ON s_target.symbol_id = iw.symbol_id
             JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
             WHERE iw.depth < {depth_bound}
               AND s_target.parent_symbol_id IS NULL
               AND s_target.language IN ('c', 'cpp')
               AND s_from.language IN ('c', 'cpp')
               AND s_target.kind IN ('function', 'class', 'struct', 'union', 'enum', 'type')
               AND p.target_receiver IS NOT NULL AND p.target_receiver != ''
               AND NOT EXISTS (
                   SELECT 1 FROM json_each({qualifiers}) q
                   WHERE q.value = 'std' OR NOT EXISTS (
                       SELECT 1 FROM symbols n
                       WHERE n.name = q.value AND n.kind = 'namespace'
                         AND n.language IN ('c', 'cpp')))
               {target_signal_filter}"
        ));
    }

    // julie stores a JavaScript call on an object (`res.send()`) only as an identifier. It reaches
    // a function assigned on an object of the receiver's name (`res.send = function`), or on the
    // caller's own object through `this`, when no other definition is assigned on that object.
    if has_column(conn, "identifiers", "metadata_json") {
        let at = "instr(t.signature, '.' || t.name || ' =')";
        let assigned = format!("substr(t.signature, 1, {at} + length(t.name) + 2)");
        recursive_branches.push(format!(
            "SELECT i.containing_symbol_id AS from_symbol_id, iw.depth + 1, iw.via, iw.symbol_id
             FROM impact_walk iw
             CROSS JOIN symbols t ON t.symbol_id = iw.symbol_id
             JOIN identifiers i ON i.name = t.name AND i.kind = 'call'
             LEFT JOIN symbols c ON c.symbol_id = i.containing_symbol_id
             WHERE iw.depth < {depth_bound}
               AND t.kind IN ('function', 'method')
               AND t.language IN ({JS_FAMILY_LANGUAGES_SQL})
               AND i.language IN ({JS_FAMILY_LANGUAGES_SQL})
               AND i.containing_symbol_id IS NOT NULL
               AND {at} > 1
               AND (json_extract(i.metadata_json, '$.receiver') || '.' || t.name || ' =' = {assigned}
                    OR (json_extract(i.metadata_json, '$.receiver') = 'this'
                        AND substr(c.signature, 1, {at}) = substr(t.signature, 1, {at})))
               AND NOT EXISTS (
                   SELECT 1 FROM symbols o
                   WHERE o.name = t.name AND o.symbol_id != t.symbol_id
                     AND o.language IN ({JS_FAMILY_LANGUAGES_SQL})
                     AND substr(o.signature, 1, {at} + length(t.name) + 2) = {assigned})"
        ));
    }

    // An anonymous class is built where it is written, so a change to its member reaches the
    // symbol that creates the object (`make` for `return new Task() { … }`), and the callers of
    // that symbol get the object.
    if has_relationships || has_pending {
        recursive_branches.push(format!(
            "SELECT {ANONYMOUS_CLASS_CREATOR}, iw.depth + 1, iw.via, iw.symbol_id
             FROM impact_walk iw
             CROSS JOIN symbols member ON member.symbol_id = iw.symbol_id
             JOIN symbols anon ON anon.symbol_id = member.parent_symbol_id
             JOIN symbols holder ON holder.symbol_id = anon.parent_symbol_id
             WHERE iw.depth < {depth_bound}
               AND {anonymous}",
            anonymous = anonymous_class_predicate("anon"),
        ));
    }

    // A constructor runs wherever its class is built: `Flask()` reaches `Flask.__init__`. Rows
    // reached only this way (`via` 1) sort last: nearly every test builds the app.
    if has_relationships {
        let constructor_call_language =
            call_languages_compatible_sql("s_from.language", "ctor.language");
        recursive_branches.push(format!(
            "SELECT r.from_symbol_id, iw.depth + 1, 1, iw.symbol_id
             FROM impact_walk iw
             CROSS JOIN symbols ctor ON ctor.symbol_id = iw.symbol_id
             JOIN relationships r ON r.to_symbol_id = ctor.parent_symbol_id
             JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
             WHERE iw.depth < {depth_bound}
               AND ctor.kind = 'constructor'
               AND +r.kind = 'calls'
               AND {constructor_call_language}"
        ));
    }
    if has_pending && has_pending_namespace_column(conn) {
        let constructor_call_language =
            call_languages_compatible_sql("s_from.language", "s_target.language");
        let predicate = if include_typed_receiver_chain {
            pending_target_predicate_with_receiver_chain_owner(
                conn,
                "s_target",
                "s_target_parent",
                &bound_receiver_chain_owner_sql("?3"),
            )
        } else {
            pending_target_predicate_without_typed_receiver_chain(
                conn,
                "s_target",
                "s_target_parent",
            )
        };
        recursive_branches.push(format!(
            "SELECT p.from_symbol_id, iw.depth + 1, 1, iw.symbol_id
             FROM impact_walk iw
             CROSS JOIN symbols ctor ON ctor.symbol_id = iw.symbol_id
             CROSS JOIN symbols s_target ON s_target.symbol_id = ctor.parent_symbol_id
             JOIN pending_relationships p ON p.target_terminal_name = s_target.name
             LEFT JOIN symbols s_target_parent ON s_target.parent_symbol_id = s_target_parent.symbol_id
             JOIN symbols s_from ON p.from_symbol_id = s_from.symbol_id
             WHERE iw.depth < {depth_bound}
               AND ctor.kind = 'constructor'
               AND s_target.kind IN ('class', 'struct', 'record')
               AND +p.kind = 'calls'
               AND {constructor_call_language}
               AND {pred}",
            pred = predicate
        ));
    }
    recursive_branches
}

/// The symbol that creates the anonymous class `anon`, whose parent is `holder`: that parent, or
/// the function around it when the parent is a local variable that holds the object
/// (`val r = object : Runnable { … }`).
const ANONYMOUS_CLASS_CREATOR: &str = "CASE WHEN holder.kind = 'variable'
         AND EXISTS (SELECT 1 FROM symbols outer_fn
                     WHERE outer_fn.symbol_id = holder.parent_symbol_id
                       AND outer_fn.kind IN ('function', 'method', 'constructor'))
         THEN holder.parent_symbol_id ELSE holder.symbol_id END";

/// The symbols that create an anonymous class with a member among the walk rows in `rows_json`,
/// each with the name of that class: such a symbol is impacted because it creates the object.
fn anonymous_class_creators(
    conn: &Connection,
    rows_json: &str,
) -> Result<HashMap<String, String>, QueryError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ANONYMOUS_CLASS_CREATOR}, MIN(anon.name)
         FROM json_each(?1) w
         JOIN symbols member ON member.symbol_id = json_extract(w.value, '$[0]')
         JOIN symbols anon ON anon.symbol_id = member.parent_symbol_id
         JOIN symbols holder ON holder.symbol_id = anon.parent_symbol_id
         WHERE {anonymous}
         GROUP BY 1",
        anonymous = anonymous_class_predicate("anon"),
    ))?;
    let rows = stmt.query_map(params![rows_json], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The test that encloses a function defined inside it, as `index` inside `test_url_processors`,
/// also through a class defined inside it or a value that holds an anonymous class
/// (`val r = object : Runnable { … }`): the nearest enclosing function or method that is a test,
/// never one that is setup or a fixture.
pub(crate) fn enclosing_walk_test(
    conn: &Connection,
    parent: Option<&str>,
) -> Result<Option<TestTarget>, QueryError> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    let lifecycle = if has_column(conn, "symbols", "test_lifecycle") {
        "COALESCE(s.test_lifecycle, 0) != 0"
    } else {
        "0"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT s.name, s.kind, s.path, s.start_line, s.is_test, {lifecycle}, s.parent_symbol_id,
                EXISTS (SELECT 1 FROM symbols owner
                        WHERE owner.symbol_id = s.parent_symbol_id
                          AND COALESCE(owner.test_container, 0) != 0)
         FROM symbols s WHERE s.symbol_id = ?1"
    ))?;
    let mut next = Some(parent.to_string());
    for _ in 0..MAX_ENCLOSING_SCOPES {
        let Some(id) = next.take() else {
            return Ok(None);
        };
        let Some((name, kind, path, line, is_test, is_fixture, parent, in_test_class)) = stmt
            .query_row([&id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?.replace('\\', "/"),
                    row.get::<_, i64>(3)? as usize,
                    row.get::<_, bool>(4)?,
                    row.get::<_, bool>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, bool>(7)?,
                ))
            })
            .optional()?
        else {
            return Ok(None);
        };
        if matches!(
            kind.as_str(),
            "class" | "variable" | "property" | "field" | "constant"
        ) {
            next = parent;
            continue;
        }
        if !matches!(kind.as_str(), "function" | "method") || is_fixture {
            return Ok(None);
        }
        if is_test || (is_test_path(&path) && (in_test_class || names_a_test(&name))) {
            return Ok(Some(TestTarget {
                name,
                path,
                line,
                reason: String::new(),
            }));
        }
        next = parent;
    }
    Ok(None)
}

/// Prefixes a test method with its class, as `TestRoutes::test_simple`, so the name selects the
/// test the way pytest and most runners address it.
fn qualify_test_methods(conn: &Connection, tests: &mut [TestTarget]) -> Result<(), QueryError> {
    let mut stmt = conn.prepare(
        "SELECT class.name FROM symbols t
         JOIN symbols class ON class.symbol_id = t.parent_symbol_id
         WHERE replace(t.path, '\\', '/') = ?1 AND t.start_line = ?2 AND t.name = ?3
           AND class.kind = 'class'
         LIMIT 1",
    )?;
    for test in tests.iter_mut().filter(|test| test.name != test.path) {
        if let Some(class) = stmt
            .query_row(params![test.path, test.line as i64, test.name], |row| {
                row.get::<_, String>(0)
            })
            .optional()?
        {
            test.name = format!("{class}::{}", test.name);
        }
    }
    Ok(())
}

/// The name tests use for the fixture function `function` in `path`: the `name=` argument of its
/// `@pytest.fixture(...)` decorator when it has one, else the function name.
fn fixture_name(conn: &Connection, function: &str, path: &str) -> String {
    let signature: Option<String> = conn
        .query_row(
            "SELECT signature FROM symbols
             WHERE name = ?1 AND replace(path, '\\', '/') = ?2 AND signature LIKE '%fixture(%'
             LIMIT 1",
            params![function, path],
            |row| row.get(0),
        )
        .optional()
        .ok()
        .flatten();
    signature
        .as_deref()
        .and_then(declared_fixture_name)
        .unwrap_or_else(|| function.to_string())
}

/// The `name=` argument of a `fixture(...)` decorator in `signature`.
pub(crate) fn declared_fixture_name(signature: &str) -> Option<String> {
    let args = &signature[signature.find("fixture(")? + "fixture(".len()..];
    let args = &args[..args.find(')').unwrap_or(args.len())];
    let value = args[args.find("name=")? + "name=".len()..].trim_start();
    let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let end = value[1..].find(quote)?;
    Some(value[1..1 + end].to_string())
}

/// The test functions that take the fixture `name` as a parameter: in the fixture's own file,
/// or anywhere under the directory of the `conftest.py` that defines it. A fixture that takes it
/// is not a test, so its own users are listed in its place.
fn fixture_users(
    conn: &Connection,
    name: &str,
    fixture_path: &str,
) -> Result<Vec<TestTarget>, QueryError> {
    let lifecycle = if has_column(conn, "symbols", "test_lifecycle") {
        "COALESCE(t.test_lifecycle, 0) != 0"
    } else {
        "0"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT DISTINCT t.name, t.path, t.start_line, {lifecycle}
         FROM symbols parameter
         JOIN symbols t ON t.symbol_id = parameter.parent_symbol_id
         WHERE parameter.name = ?1
           AND (parameter.kind = 'parameter'
                OR (parameter.kind = 'variable' AND parameter.start_byte < t.body_start_byte))
           AND t.is_test = 1
           AND replace(t.path, '\\', '/') LIKE ?2 ESCAPE '\\'
         ORDER BY t.path, t.start_line
         LIMIT 2000"
    ))?;
    // ponytail: the callers rank these rows by name, so the cap must hold a whole test suite's
    // users of one fixture; raise it if a suite passes 2,000 users of a single fixture.
    let mut users = Vec::new();
    let mut listed = HashSet::new();
    let mut visited = HashSet::new();
    let mut pending = vec![(name.to_string(), fixture_path.to_string())];
    while let Some((function, fixture_path)) = pending.pop() {
        if !visited.insert((function.clone(), fixture_path.clone())) {
            continue;
        }
        let name = fixture_name(conn, &function, &fixture_path);
        let scope = match fixture_path.rsplit_once('/') {
            Some((dir, "conftest.py")) => format!("{}/%", escape_like(dir)),
            None if fixture_path == "conftest.py" => "%".to_string(),
            _ => escape_like(&fixture_path),
        };
        let rows = stmt.query_map(params![name, scope], |row| {
            Ok((
                TestTarget {
                    name: row.get(0)?,
                    path: row.get::<_, String>(1)?.replace('\\', "/"),
                    line: row.get::<_, i64>(2)? as usize,
                    reason: String::new(),
                },
                row.get::<_, bool>(3)?,
            ))
        })?;
        for row in rows {
            let (user, is_fixture) = row?;
            if is_fixture {
                pending.push((user.name, user.path));
            } else if listed.insert((user.path.clone(), user.line)) {
                users.push(user);
            }
        }
    }
    Ok(users)
}

/// Why a seed symbol of a walk is there: it is the target, it is in a target file, or a diff
/// changed its lines.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SeedOrigin {
    Target,
    TargetFile,
    Diff,
}

impl SeedOrigin {
    fn test_reason(self) -> &'static str {
        match self {
            SeedOrigin::Target => "the target is a test",
            SeedOrigin::TargetFile => "in the target file",
            SeedOrigin::Diff => "changed in the diff",
        }
    }

    fn label(self) -> &'static str {
        match self {
            SeedOrigin::Target => "the target",
            SeedOrigin::TargetFile | SeedOrigin::Diff => self.test_reason(),
        }
    }
}

struct SeedRow {
    symbol_id: String,
    name: String,
    kind: String,
    path: String,
    start_line: usize,
    end_line: usize,
    is_test: bool,
    test_container: bool,
    is_fixture: bool,
    parent: Option<String>,
}

/// The symbols a walk starts from, in path and line order.
fn seed_rows(
    conn: &Connection,
    seed_condition: &str,
    params: &[rusqlite::types::Value],
    lifecycle: &str,
) -> Result<Vec<SeedRow>, QueryError> {
    let not_documentation = not_documentation(conn, "s");
    let mut stmt = conn.prepare(&format!(
        "SELECT s.symbol_id, s.name, s.kind, s.path, s.start_line, s.end_line, s.is_test,
                s.test_container, {lifecycle}, s.parent_symbol_id
         FROM symbols s
         WHERE ({seed_condition})
           AND s.kind NOT IN ({LOW_SIGNAL_KINDS_SQL})
           AND {not_documentation}
         ORDER BY s.path, s.start_line"
    ))?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
        Ok(SeedRow {
            symbol_id: row.get(0)?,
            name: row.get(1)?,
            kind: row.get(2)?,
            path: row.get::<_, String>(3)?.replace('\\', "/"),
            start_line: row.get::<_, i64>(4)? as usize,
            end_line: row.get::<_, i64>(5)? as usize,
            is_test: row.get(6)?,
            test_container: row.get(7)?,
            is_fixture: row.get(8)?,
            parent: row.get(9)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The tests to run because the seed `seed` changed: the seed itself when it is a test, the tests
/// that take it when it is a pytest fixture, and the tests of its class when it is a setup member.
/// A fixture or setup member that no test uses stays as its own row.
fn seed_tests(
    conn: &Connection,
    seed: &SeedRow,
    origin: SeedOrigin,
) -> Result<Vec<TestTarget>, QueryError> {
    let own_row = |reason: String| TestTarget {
        name: seed.name.clone(),
        path: seed.path.clone(),
        line: seed.start_line,
        reason,
    };
    if !seed.is_fixture {
        return Ok(vec![own_row(origin.test_reason().to_string())]);
    }
    if is_pytest_fixture(&seed.name, &seed.path) {
        let shown = fixture_name(conn, &seed.name, &seed.path);
        let users = fixture_users(conn, &seed.name, &seed.path)?;
        if users.is_empty() {
            return Ok(vec![own_row(format!("fixture ({})", origin.label()))]);
        }
        return Ok(users
            .into_iter()
            .map(|test| TestTarget {
                reason: format!("uses fixture `{shown}`"),
                ..test
            })
            .collect());
    }
    let tests = match seed.parent.as_deref() {
        Some(class_id) => tests_in_class(conn, class_id)?,
        None => Vec::new(),
    };
    if tests.is_empty() {
        return Ok(vec![own_row(format!("setup ({})", origin.label()))]);
    }
    Ok(tests
        .into_iter()
        .map(|test| TestTarget {
            reason: format!("setup `{}` runs before it", seed.name),
            ..test
        })
        .collect())
}

/// Whether a lifecycle member is a pytest fixture rather than a setup or teardown method.
fn is_pytest_fixture(name: &str, path: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    path.ends_with(".py")
        && !["setup", "teardown", "asyncsetup", "asyncteardown"]
            .iter()
            .any(|prefix| lowered.starts_with(prefix))
}

/// The tests declared in the class `class_id`, without its setup and teardown members.
fn tests_in_class(conn: &Connection, class_id: &str) -> Result<Vec<TestTarget>, QueryError> {
    let not_lifecycle = if has_column(conn, "symbols", "test_lifecycle") {
        "COALESCE(test_lifecycle, 0) = 0"
    } else {
        "1"
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT name, path, start_line FROM symbols
         WHERE parent_symbol_id = ?1 AND is_test = 1 AND {not_lifecycle}
         ORDER BY start_line"
    ))?;
    let rows = stmt.query_map([class_id], |row| {
        Ok(TestTarget {
            name: row.get(0)?,
            path: row.get::<_, String>(1)?.replace('\\', "/"),
            line: row.get::<_, i64>(2)? as usize,
            reason: String::new(),
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
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
    fn a_type_declaration_drops_attributes_and_fits_on_one_short_line() {
        assert_eq!(
            super::type_declaration_line(
                "#[derive(Debug)] #[serde(tag = \"t\")] pub struct Workspace"
            ),
            "pub struct Workspace"
        );
        let record = format!(
            "public sealed record Node(\n{})",
            "    string Id,\n".repeat(20)
        );
        let line = super::type_declaration_line(&record);
        assert!(
            line.starts_with("public sealed record Node( string Id, string Id,"),
            "{line}"
        );
        assert_eq!(line.chars().count(), 120);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn a_mount_names_what_it_mounts_and_where() {
        let mount = |capture: &str, json: &str| {
            super::mount_display(capture, &serde_json::from_str(json).unwrap())
        };
        assert_eq!(
            mount(
                "blueprint_registration",
                r#"{"framework":"flask","mount_target":"auth.bp","query_family":"framework"}"#
            )
            .as_deref(),
            Some("blueprint_registration `auth.bp`")
        );
        assert_eq!(
            mount(
                "mount",
                r#"{"mount_path":"/jobs","mount_target":"Sidekiq::Web","normalized_mount_path":"/admin/jobs","query_family":"framework"}"#
            )
            .as_deref(),
            Some("mount `Sidekiq::Web` at /admin/jobs")
        );
        assert_eq!(
            mount(
                "property",
                r#"{"mount_target":"x","query_family":"metadata"}"#
            ),
            None
        );
    }

    #[test]
    fn a_route_shows_its_source_template_and_keeps_a_joined_prefix() {
        let route = |json: &str| super::route_display(&serde_json::from_str(json).unwrap());
        assert_eq!(
            route(
                r#"{"verb":"GET","route_template":"/<int:id>/update","normalized_route_template":"/:id/update"}"#
            ),
            Some("GET /<int:id>/update".into())
        );
        assert_eq!(
            route(
                r#"{"verb":"POST","route_template":"/register","effective_route_template":"/auth/register","normalized_route_template":"/auth/register"}"#
            ),
            Some("POST /auth/register".into())
        );
        assert_eq!(
            route(
                r#"{"verb":"GET","route_template":"/users/{id}","normalized_route_template":"/api/v1/users/:id"}"#
            ),
            Some("GET /api/v1/users/:id".into())
        );
        assert_eq!(route(r#"{"key":"x"}"#), None);
    }

    #[test]
    fn a_fixture_declared_with_a_name_is_used_by_that_name() {
        assert_eq!(
            super::declared_fixture_name("@pytest.fixture(name=\"async_app\") def _async_app()"),
            Some("async_app".into())
        );
        assert_eq!(
            super::declared_fixture_name(
                "@pytest.fixture(scope='session', name='db') def make_db()"
            ),
            Some("db".into())
        );
        assert_eq!(
            super::declared_fixture_name("@pytest.fixture def app()"),
            None
        );
        assert_eq!(
            super::declared_fixture_name("@pytest.fixture(params=[1]) def f(name='x')"),
            None
        );
    }

    #[test]
    fn result_limit_rejects_values_above_the_shared_ceiling() {
        assert!(validate_result_limit(MAX_RESULT_LIMIT).is_ok());
        assert!(matches!(
            validate_result_limit(usize::MAX),
            Err(QueryError::InvalidResultLimit(usize::MAX))
        ));
    }

    #[test]
    fn report_pages_bound_page_size_and_allow_large_offsets_until_overflow() {
        assert!(validate_result_page(200, 1).is_ok_and(|end| end == 201));
        assert!(validate_result_page(usize::MAX, 0).is_ok_and(|end| end == usize::MAX));
        assert!(matches!(
            validate_result_page(usize::MAX, 1),
            Err(QueryError::InvalidResultPage {
                offset: usize::MAX,
                limit: 1
            })
        ));
        assert!(matches!(
            validate_result_page(0, MAX_RESULT_LIMIT + 1),
            Err(QueryError::InvalidResultLimit(limit)) if limit == MAX_RESULT_LIMIT + 1
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
    fn callee_page_query_uses_source_indexes_before_offset() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (symbol_id TEXT, name TEXT, kind TEXT, parent_symbol_id TEXT, language TEXT);
             CREATE INDEX idx_symbols_name_kind ON symbols(name, kind);
             CREATE INDEX idx_symbols_id ON symbols(symbol_id);
             CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT, path TEXT, start_line INTEGER, start_column INTEGER);
             CREATE INDEX idx_relationships_from ON relationships(from_symbol_id);
             CREATE TABLE pending_relationships (from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT, start_line INTEGER, start_column INTEGER, target_receiver TEXT, target_namespace_json TEXT, target_display_name TEXT);
             CREATE INDEX idx_pending_from ON pending_relationships(from_symbol_id);
             WITH RECURSIVE n(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM n WHERE value < 1000)
             INSERT INTO symbols (symbol_id, name, kind, parent_symbol_id, language)
             SELECT 'noise-' || value, 'noise_' || value, 'function', NULL, 'rust' FROM n;
             INSERT INTO symbols (symbol_id, name, kind, parent_symbol_id, language)
             VALUES ('caller', 'caller', 'function', NULL, 'rust'), ('callee', 'callee', 'function', NULL, 'rust');
             WITH RECURSIVE n(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM n WHERE value < 1000)
             INSERT INTO relationships SELECT 'noise-' || value, 'noise-' || value, 'calls', 'noise.py', value, 0 FROM n;
             INSERT INTO relationships VALUES ('caller', 'callee', 'calls', 'app.py', 2, 4);
             WITH RECURSIVE n(value) AS (VALUES(1) UNION ALL SELECT value + 1 FROM n WHERE value < 1000)
             INSERT INTO pending_relationships (from_symbol_id, target_terminal_name, kind, path, start_line, start_column) SELECT 'noise-' || value, 'noise', 'calls', 'noise.py', value, 0 FROM n;
             INSERT INTO pending_relationships (from_symbol_id, target_terminal_name, kind, path, start_line, start_column) VALUES ('caller', 'callee', 'calls', 'app.py', 2, 4);
             ANALYZE;",
        )
        .unwrap();

        let (cte, _) = callee_page_cte(&conn, true, None);
        let mut stmt = conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {cte}
                 SELECT from_symbol_name, from_symbol_id, to_symbol_name, kind, path,
                        start_line, start_column
                 FROM merged_sites
                 ORDER BY path, start_line, start_column, to_symbol_name, kind, from_symbol_id
                 LIMIT ?3 OFFSET ?4"
            ))
            .unwrap();
        let plan = stmt
            .query_map(params!["caller", "caller", 1, 200], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(plan.contains("idx_symbols_name_kind"), "{plan}");
        assert!(plan.contains("idx_relationships_from"), "{plan}");
        assert!(plan.contains("idx_pending_from"), "{plan}");
    }

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

    fn selected_type_owner_id(
        conn: &Connection,
        source_id: &str,
        resolved_type: &str,
        language: &str,
    ) -> Option<String> {
        conn.query_row(
            &format!("SELECT {}", type_fact_owner_id_sql("?1", "?2", "?3")),
            params![source_id, resolved_type, language],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn related_type_owner_sql_uses_import_and_name_indexes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                language TEXT NOT NULL,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                parent_symbol_id TEXT,
                metadata_json TEXT NOT NULL,
                signature TEXT
            );
            CREATE INDEX idx_symbols_import_path_name ON symbols(path, name) WHERE kind = 'import';
            CREATE INDEX idx_symbols_name_kind ON symbols(name, kind);
            CREATE INDEX idx_symbols_path ON symbols(path);
            CREATE INDEX idx_symbols_parent ON symbols(parent_symbol_id);",
        )
        .unwrap();
        let plan_sql = format!(
            "EXPLAIN QUERY PLAN SELECT {}",
            type_fact_owner_id_sql("?1", "?2", "?3")
        );
        let mut statement = conn.prepare(&plan_sql).unwrap();
        let plan: Vec<String> = statement
            .query_map(params!["source", "Service", "typescript"], |row| row.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(plan.iter().any(|row| row.contains("idx_symbols_name_kind")));
        assert!(
            plan.iter()
                .any(|row| row.contains("idx_symbols_import_path_name"))
        );
        assert!(plan.iter().any(|row| row.contains("idx_symbols_path")));
        assert!(
            !plan
                .iter()
                .any(|row| row == "SCAN d" || row.starts_with("SCAN d ")),
            "{plan:?}"
        );
    }

    #[test]
    fn typed_receiver_chain_support_requires_all_proof_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT,
                path TEXT,
                language TEXT,
                name TEXT,
                kind TEXT,
                parent_symbol_id TEXT,
                start_line INTEGER,
                metadata_json TEXT,
                signature TEXT
            );
            CREATE TABLE type_facts (symbol_id TEXT, resolved_type TEXT, language TEXT);
            CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);
            CREATE TABLE files (file_id TEXT, language TEXT);
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, file_id TEXT, target_receiver TEXT, target_namespace_json TEXT
            );",
        )
        .unwrap();
        assert!(has_typed_receiver_chain_support(&conn));

        for (table, column) in [
            ("symbols", "symbol_id"),
            ("symbols", "path"),
            ("symbols", "language"),
            ("symbols", "name"),
            ("symbols", "kind"),
            ("symbols", "parent_symbol_id"),
            ("symbols", "signature"),
            ("symbols", "metadata_json"),
            ("symbols", "start_line"),
            ("type_facts", "symbol_id"),
            ("type_facts", "resolved_type"),
            ("type_facts", "language"),
            ("relationships", "from_symbol_id"),
            ("relationships", "to_symbol_id"),
            ("relationships", "kind"),
            ("pending_relationships", "from_symbol_id"),
            ("pending_relationships", "target_terminal_name"),
            ("pending_relationships", "kind"),
            ("pending_relationships", "path"),
            ("pending_relationships", "start_line"),
            ("pending_relationships", "file_id"),
            ("pending_relationships", "target_receiver"),
            ("pending_relationships", "target_namespace_json"),
            ("files", "file_id"),
            ("files", "language"),
        ] {
            conn.execute(&format!("ALTER TABLE {table} DROP COLUMN {column}"), [])
                .unwrap();
            assert!(
                !has_typed_receiver_chain_support(&conn),
                "missing {table}.{column} must disable the typed chain SQL"
            );
            let data_type = if column == "start_line" {
                "INTEGER"
            } else {
                "TEXT"
            };
            conn.execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {data_type}"),
                [],
            )
            .unwrap();
            assert!(has_typed_receiver_chain_support(&conn));
        }

        conn.execute_batch("DROP TABLE relationships").unwrap();
        assert!(!has_typed_receiver_chain_support(&conn));
        conn.execute(
            "CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT)",
            [],
        )
        .unwrap();
        conn.execute_batch("DROP TABLE pending_relationships")
            .unwrap();
        assert!(!has_typed_receiver_chain_support(&conn));
    }

    #[test]
    fn receiver_chain_probe_tracks_current_targets_constructor_parents_and_malformed_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                parent_symbol_id TEXT, start_line INTEGER, metadata_json TEXT, signature TEXT
            );
            CREATE TABLE type_facts (symbol_id TEXT, resolved_type TEXT, language TEXT);
            CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);
            CREATE TABLE files (file_id TEXT, language TEXT);
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, file_id TEXT, target_receiver TEXT, target_namespace_json TEXT,
                target_display_name TEXT
            );
            INSERT INTO symbols VALUES
                ('send', 'service.go', 'go', 'Send', 'method', NULL, 1, NULL, NULL),
                ('ping', 'service.go', 'go', 'Ping', 'method', NULL, 2, NULL, NULL),
                ('ctor', 'widget.py', 'python', '__init__', 'constructor', 'owner', 1, NULL, NULL),
                ('owner', 'widget.py', 'python', 'Widget', 'class', NULL, 1, NULL, NULL);
            INSERT INTO pending_relationships VALUES
                ('caller', 'Send', 'calls', 'caller.go', 1, NULL, 'service', '[\"group\"]', 'group.service.Send'),
                ('caller', 'Widget', 'calls', 'caller.py', 2, NULL, 'service', '[\"group\"]', 'service.Widget'),
                ('caller', 'Ping', 'calls', 'caller.go', 3, NULL, 'value', '[]', 'value.Ping');",
        )
        .unwrap();
        assert!(has_typed_receiver_chain_support(&conn));

        let send_frontier = vec![("send".to_string(), 0, 0)];
        let ping_frontier = vec![("ping".to_string(), 0, 0)];
        let constructor_frontier = vec![("ctor".to_string(), 0, 0)];

        assert!(pending_receiver_chain_requires_full_for_frontier(
            &conn,
            &send_frontier
        ));
        assert!(!pending_receiver_chain_requires_full_for_frontier(
            &conn,
            &ping_frontier
        ));
        assert!(pending_receiver_chain_requires_full_for_frontier(
            &conn,
            &constructor_frontier
        ));

        conn.execute(
            "UPDATE pending_relationships SET target_namespace_json = 'broken' WHERE target_terminal_name = 'Ping'",
            [],
        )
        .unwrap();
        assert!(pending_receiver_chain_requires_full_for_frontier(
            &conn,
            &ping_frontier
        ));

        let full = pending_target_predicate(&conn, "s_target", "s_parent");
        let legacy =
            pending_target_predicate_without_typed_receiver_chain(&conn, "s_target", "s_parent");
        assert!(full.len() > legacy.len() * 2);
    }

    #[test]
    fn graph_queries_read_receiver_chain_owners_from_a_bound_map() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                parent_symbol_id TEXT, start_line INTEGER, metadata_json TEXT, signature TEXT
            );
            CREATE TABLE type_facts (symbol_id TEXT, resolved_type TEXT, language TEXT);
            CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);
            CREATE TABLE files (file_id TEXT, language TEXT);
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, file_id TEXT, target_receiver TEXT, target_namespace_json TEXT,
                target_display_name TEXT
            );
            INSERT INTO symbols VALUES
                ('send', 'service.go', 'go', 'Send', 'method', NULL, 1, NULL, NULL);
            INSERT INTO pending_relationships VALUES
                ('caller', 'Send', 'calls', 'caller.go', 1, NULL, 'service', '[\"group\"]', 'group.service.Send');",
        )
        .unwrap();
        let owner = pending_receiver_chain_owner_sql();
        let inline = pending_target_predicate(&conn, "s_target", "s_parent");
        let bound = bound_receiver_chain_owner_sql("?3");
        let walk = pending_target_predicate_with_receiver_chain_owner(
            &conn, "s_target", "s_parent", &bound,
        );

        assert!(inline.contains(&owner));
        assert!(!walk.contains(&owner));
        assert!(walk.contains(&bound));
        assert_eq!(
            pending_receiver_chain_owners(&conn, &[("send".to_string(), 0, 0)]).unwrap(),
            "{}"
        );
    }

    #[test]
    fn typed_receiver_chain_owner_rows_do_not_scan_unrelated_pending_rows() {
        fn vm_steps(unrelated_rows: usize) -> i32 {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(
                "CREATE TABLE symbols (
                    symbol_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                    parent_symbol_id TEXT, start_line INTEGER, metadata_json TEXT,
                    signature TEXT, file_id TEXT
                );
                CREATE TABLE type_facts (symbol_id TEXT, resolved_type TEXT, language TEXT);
                CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);
                CREATE TABLE pending_relationships (
                    from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                    start_line INTEGER, file_id TEXT, target_receiver TEXT, target_namespace_json TEXT,
                    target_display_name TEXT
                );
                CREATE TABLE files (file_id TEXT, language TEXT);
                INSERT INTO symbols VALUES
                    ('caller', 'caller.go', 'go', 'Call', 'function', NULL, 1, NULL, NULL, NULL);
                INSERT INTO pending_relationships VALUES
                    ('caller', 'Send', 'calls', 'caller.go', 1, NULL, 'service', '[\"group\"]', 'group.service.Send');",
            )
            .unwrap();
            conn.execute(
                "WITH RECURSIVE rows(row_number) AS (
                    SELECT 1 WHERE ?1 > 0
                    UNION ALL
                    SELECT row_number + 1 FROM rows WHERE row_number < ?1
                )
                INSERT INTO pending_relationships
                    (from_symbol_id, target_terminal_name, kind, path, start_line,
                     target_receiver, target_namespace_json)
                SELECT 'unrelated', 'Other', 'calls', 'other.go', row_number,
                       NULL, '[]'
                FROM rows",
                [unrelated_rows as i64],
            )
            .unwrap();

            let mut statement = conn
                .prepare(&typed_receiver_chain_owner_rows_sql())
                .unwrap();
            let rows = statement
                .query_map(["[\"1\"]"], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].0, "P:1");
            statement.get_status(rusqlite::StatementStatus::VmStep)
        }

        let one_row = vm_steps(0);
        let many_rows = vm_steps(2_000);
        assert!(
            many_rows <= one_row + 128,
            "receiver owner lookup executed {many_rows} VM steps with unrelated rows versus {one_row} without them"
        );
    }

    #[test]
    fn member_receiver_match_keeps_its_single_segment_predicate_compact() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
                parent_symbol_id TEXT, start_line INTEGER, start_column INTEGER, end_line INTEGER,
                metadata_json TEXT, signature TEXT, visibility TEXT
            );
            CREATE TABLE type_facts (symbol_id TEXT, resolved_type TEXT, language TEXT);
            CREATE TABLE relationships (
                from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT, file_id TEXT, path TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, metadata_json TEXT
            );
            CREATE TABLE pending_relationships (
                from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT,
                start_line INTEGER, start_column INTEGER, end_line INTEGER, file_id TEXT,
                target_receiver TEXT, target_namespace_json TEXT, target_display_name TEXT
            );
            CREATE TABLE files (file_id TEXT, language TEXT);",
        )
        .unwrap();

        assert!(has_typed_receiver_chain_support(&conn));
        let predicate = member_receiver_match(&conn);

        assert!(
            predicate.len() < 100_000,
            "single-segment resolver predicate expanded to {} bytes",
            predicate.len()
        );
    }

    #[test]
    fn pending_receiver_ancestry_query_accepts_a_bound_row_limit() {
        assert!(typed_receiver_chain_pending_base_owner_rows_sql().contains("LIMIT ?2"));
    }

    #[test]
    fn resolved_receiver_ancestry_rows_stop_at_the_supplied_limit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);",
        )
        .unwrap();
        for _ in 0..20 {
            conn.execute(
                "INSERT INTO relationships VALUES ('root', 'base', 'extends')",
                [],
            )
            .unwrap();
        }
        let rows = conn
            .prepare(typed_receiver_chain_resolved_base_rows_sql())
            .unwrap()
            .query_map(params!["[\"root\"]", 3], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn receiver_ancestry_depth_probe_distinguishes_a_leaf() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT);
            CREATE TABLE pending_relationships (from_symbol_id TEXT, kind TEXT);
            INSERT INTO relationships VALUES ('root', 'middle', 'extends');
            INSERT INTO relationships VALUES ('middle', 'leaf', 'implements');",
        )
        .unwrap();
        let mut stmt = conn
            .prepare(typed_receiver_chain_has_more_ancestors_sql())
            .unwrap();

        assert!(
            stmt.query_row(params!["[\"middle\"]"], |row| row.get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            !stmt
                .query_row(params!["[\"leaf\"]"], |row| row.get::<_, bool>(0))
                .unwrap()
        );
    }

    #[test]
    fn related_type_fact_scan_limit_bounds_unresolved_facts() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                file_id TEXT NOT NULL,
                path TEXT NOT NULL,
                language TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                signature TEXT,
                doc_comment TEXT,
                visibility TEXT,
                parent_symbol_id TEXT,
                start_line INTEGER NOT NULL,
                start_column INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                end_column INTEGER NOT NULL,
                start_byte INTEGER NOT NULL,
                end_byte INTEGER NOT NULL,
                body_start_line INTEGER,
                body_start_column INTEGER,
                body_end_line INTEGER,
                body_end_column INTEGER,
                body_start_byte INTEGER,
                body_end_byte INTEGER,
                body_hash TEXT,
                semantic_group TEXT,
                is_test INTEGER NOT NULL,
                test_container INTEGER NOT NULL,
                metadata_json TEXT NOT NULL
            );
            CREATE TABLE type_facts (
                type_fact_id TEXT NOT NULL,
                symbol_id TEXT NOT NULL,
                language TEXT NOT NULL,
                resolved_type TEXT NOT NULL,
                generic_params_json TEXT
            );
            INSERT INTO symbols
                (symbol_id, file_id, path, language, name, kind, parent_symbol_id,
                 start_line, start_column, end_line, end_column, start_byte, end_byte,
                 is_test, test_container, metadata_json)
            VALUES
                ('target', 'file', 'src/handler.ts', 'typescript', 'submit', 'function', NULL, 1, 0, 1, 0, 0, 0, 0, 0, '{}'),
                ('first', 'file', 'src/handler.ts', 'typescript', 'first', 'variable', 'target', 2, 0, 2, 0, 0, 0, 0, 0, '{}'),
                ('second', 'file', 'src/handler.ts', 'typescript', 'second', 'variable', 'target', 3, 0, 3, 0, 0, 0, 0, 0, '{}'),
                ('late', 'file', 'src/handler.ts', 'typescript', 'late', 'variable', 'target', 4, 0, 4, 0, 0, 0, 0, 0, '{}'),
                ('late_type', 'file', 'src/handler.ts', 'typescript', 'LateType', 'class', NULL, 5, 0, 5, 0, 0, 0, 0, 0, '{}');
            INSERT INTO type_facts VALUES
                ('fact1', 'first', 'typescript', 'UnknownOne', '[]'),
                ('fact2', 'second', 'typescript', 'UnknownTwo', '[]'),
                ('fact3', 'late', 'typescript', 'LateType', '[]');",
        )
        .unwrap();
        let target = get_symbol_by_id(&conn, "target").unwrap().unwrap();

        assert!(
            find_related_types_with_fact_limit(&conn, &target, 10, 2)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            find_related_types_with_fact_limit(&conn, &target, 10, 3).unwrap(),
            ["class LateType (src/handler.ts:5)"]
        );
    }

    #[test]
    fn a_full_java_import_name_blocks_current_namespace_fallback() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                language TEXT NOT NULL,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                parent_symbol_id TEXT,
                metadata_json TEXT NOT NULL,
                signature TEXT
            );
            INSERT INTO symbols VALUES
                ('java_source', 'java/current/Handler.java', 'java', 'field', 'service', NULL, '{}', NULL),
                ('source_namespace', 'java/current/Handler.java', 'java', 'namespace', 'current', NULL, '{}', 'package current'),
                ('target_namespace', 'java/current/Service.java', 'java', 'namespace', 'current', NULL, '{}', 'package current'),
                ('service', 'java/current/Service.java', 'java', 'class', 'Service', NULL, '{}', 'class Service'),
                ('external_import', 'java/current/Handler.java', 'java', 'import', 'external.Service', NULL, '{}', 'import external.Service');",
        )
        .unwrap();

        assert_eq!(
            selected_type_owner_id(&conn, "java_source", "Service", "java"),
            None
        );
    }

    #[test]
    fn related_type_owner_uses_identifier_language_eligibility() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE symbols (
                symbol_id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                language TEXT NOT NULL,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                parent_symbol_id TEXT,
                metadata_json TEXT NOT NULL,
                signature TEXT
            );
            INSERT INTO symbols VALUES
                ('qml_source', 'qml/use.qml', 'qml', 'function', 'useWidget', NULL, '{}', NULL),
                ('cpp_target', 'qml/use.qml', 'cpp', 'class', 'Widget', NULL, '{}', NULL),
                ('cpp_source', 'cpp/use.cpp', 'cpp', 'function', 'useWidget', NULL, '{}', NULL),
                ('qml_target', 'cpp/use.cpp', 'qml', 'class', 'Widget', NULL, '{}', NULL),
                ('razor_source', 'razor/use.razor', 'razor', 'function', 'useWidget', NULL, '{}', NULL),
                ('csharp_target', 'razor/use.razor', 'csharp', 'class', 'Widget', NULL, '{}', NULL),
                ('csharp_source', 'csharp/use.cs', 'csharp', 'function', 'useWidget', NULL, '{}', NULL),
                ('razor_target', 'csharp/use.cs', 'razor', 'class', 'Widget', NULL, '{}', NULL);",
        )
        .unwrap();

        assert_eq!(
            selected_type_owner_id(&conn, "qml_source", "Widget", "qml").as_deref(),
            Some("cpp_target")
        );
        assert_eq!(
            selected_type_owner_id(&conn, "cpp_source", "Widget", "cpp"),
            None
        );
        assert_eq!(
            selected_type_owner_id(&conn, "razor_source", "Widget", "razor").as_deref(),
            Some("csharp_target")
        );
        assert_eq!(
            selected_type_owner_id(&conn, "csharp_source", "Widget", "csharp").as_deref(),
            Some("razor_target")
        );
    }

    #[test]
    fn js_family_sql_lists_the_js_family_languages() {
        let quoted: Vec<String> = JS_FAMILY_LANGUAGES
            .iter()
            .map(|language| format!("'{language}'"))
            .collect();
        assert_eq!(JS_FAMILY_LANGUAGES_SQL, quoted.join(", "));
        assert!(JS_FAMILY_LANGUAGES.iter().all(|l| is_js_family(l)));
        assert!(!is_js_family("python"));
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

    #[test]
    fn ranked_owner_context_recovers_members_and_enriches_existing_rows() {
        let mut rows = vec![
            code_row(
                "planner",
                "src/planner.rs",
                "rust",
                "EditPlanner",
                "Plans edit operations for a function body",
            ),
            code_row(
                "target",
                "src/planner.rs",
                "rust",
                "ReplaceBodyImplementation",
                "",
            ),
            code_row("existing", "src/planner.rs", "rust", "Body", ""),
            code_row(
                "unrelated",
                "src/planner.rs",
                "rust",
                "SaveConfiguration",
                "",
            ),
        ];
        // Full-text and short-name hits exhaust both ordinary admission branches.
        rows.extend((0..200).map(|i| {
            code_row(
                &format!("noise{i}"),
                "src/noise.rs",
                "rust",
                &format!("body{i:03}"),
                "edit planner body",
            )
        }));
        let conn = search_fixture(&rows.join(","));
        conn.execute_batch(
            "UPDATE symbols SET kind = 'class' WHERE symbol_id = 'planner';
             UPDATE symbols SET kind = 'method', parent_symbol_id = 'planner'
             WHERE symbol_id IN ('target', 'existing', 'unrelated');",
        )
        .unwrap();
        let ordinary =
            collect_search_candidates(&conn, "edit planner body", None, None, false, 200).unwrap();
        assert!(
            !ordinary
                .iter()
                .any(|c| c.result.symbol.symbol_id == "target")
        );
        assert!(
            ordinary
                .iter()
                .any(|c| c.result.symbol.symbol_id == "existing")
        );
        let found =
            fts_search_symbols_explained(&conn, "edit planner body", None, None, false, 200, true)
                .unwrap();
        for id in ["target", "existing"] {
            let members: Vec<_> = found.iter().filter(|r| r.symbol.symbol_id == id).collect();
            assert_eq!(members.len(), 1);
            let explain = members[0].explain.as_ref().unwrap();
            assert!(explain.branches.iter().any(|b| b == "owner_context"));
            for word in ["edit", "planner"] {
                assert!(
                    explain
                        .terms
                        .contains(&(word.into(), "owner_context".into(), 0.5))
                );
            }
            assert!(explain.terms.contains(&("body".into(), "name".into(), 3.0)));
        }
        assert!(!found.iter().any(|r| r.symbol.symbol_id == "unrelated"));
        for exact in ["EditPlanner", "Body"] {
            assert_eq!(search_names(&conn, exact)[0], exact);
        }
        let scoped = fts_search_symbols_scoped(
            &conn,
            "edit planner body",
            None,
            Some("src/noise.rs"),
            false,
            200,
        )
        .unwrap();
        assert!(scoped.iter().all(|r| r.symbol.path == "src/noise.rs"));
        let methods =
            fts_search_symbols_scoped(&conn, "edit planner body", Some("method"), None, false, 200)
                .unwrap();
        assert!(methods.iter().all(|r| r.symbol.kind == "method"));
        // A method-only query has no owner types in its ordinary top twenty.
        assert!(methods.iter().all(|r| r.explain.is_none()));
    }

    #[test]
    fn ranked_owner_expansion_is_bounded_and_preserves_member_filters() {
        let mut rows = Vec::new();
        for owner in 0..4 {
            rows.push(code_row(
                &format!("o{owner}"),
                "src/lib.rs",
                "rust",
                &format!("Planner{owner}"),
                "body planner",
            ));
            for member in 0..41 {
                rows.push(code_row(
                    &format!("o{owner}m{member:02}"),
                    "src/lib.rs",
                    "rust",
                    &format!("Body{member:02}"),
                    "",
                ));
            }
        }
        rows.push(doc_row("docowner", "PlannerDocs", "body planner"));
        rows.push(code_row(
            "docchild",
            "src/lib.rs",
            "rust",
            "BodyFromDocs",
            "",
        ));
        rows.push(code_row(
            "testchild",
            "tests/lib.rs",
            "rust",
            "BodyTest",
            "",
        ));
        rows.push(code_row(
            "linkchild",
            "docs/link.md",
            "markdown",
            "BodyLink",
            "",
        ));
        rows.push(code_row(
            "nestedchild",
            "src/lib.rs",
            "rust",
            "BodyNested",
            "",
        ));
        let conn = search_fixture(&rows.join(","));
        for owner in 0..4 {
            conn.execute(
                "UPDATE symbols SET kind = 'class' WHERE symbol_id = ?1",
                [format!("o{owner}")],
            )
            .unwrap();
            conn.execute(
                "UPDATE symbols SET kind = 'method', parent_symbol_id = ?1 WHERE symbol_id LIKE ?2",
                params![format!("o{owner}"), format!("o{owner}m%")],
            )
            .unwrap();
        }
        conn.execute_batch(
            "UPDATE symbols SET kind = 'class' WHERE symbol_id = 'docowner';
             UPDATE symbols SET parent_symbol_id = 'docowner' WHERE symbol_id = 'docchild';
             UPDATE symbols SET parent_symbol_id = 'o0' WHERE symbol_id IN ('testchild', 'linkchild');
             UPDATE symbols SET kind = 'import' WHERE symbol_id = 'linkchild';
             UPDATE symbols SET parent_symbol_id = 'o0m00' WHERE symbol_id = 'nestedchild';",
        ).unwrap();
        let mut candidates =
            collect_search_candidates(&conn, "planner body", None, None, false, 200).unwrap();
        // Isolate owner-driven admission so existing word/name hits cannot hide a cap failure.
        candidates.retain(|c| c.result.symbol.parent_symbol_id.is_none());
        let ranked = rerank_with(candidates.clone(), "planner body", false, None);
        assert!(
            expand_ranked_owner_members(
                &conn,
                &mut candidates,
                &ranked,
                "planner body",
                None,
                None,
                false
            )
            .unwrap()
        );
        let expanded: Vec<_> = candidates
            .iter()
            .filter(|c| c.owner_context.is_some())
            .collect();
        assert_eq!(expanded.len(), 120);
        for owner in 0..3 {
            assert_eq!(
                expanded
                    .iter()
                    .filter(|c| c.result.symbol.parent_symbol_id.as_deref()
                        == Some(&format!("o{owner}")))
                    .count(),
                40
            );
            assert!(
                !expanded
                    .iter()
                    .any(|c| c.result.symbol.symbol_id == format!("o{owner}m40"))
            );
        }
        for excluded in ["docchild", "testchild", "linkchild", "nestedchild"] {
            assert!(
                !expanded
                    .iter()
                    .any(|c| c.result.symbol.symbol_id == excluded)
            );
        }
        // Twenty ordinary non-type rows put a valid type outside the expansion window.
        let mut late_ranked = rerank_with(
            (0..20).map(|i| function(&format!("body{i}"))).collect(),
            "body",
            false,
            None,
        );
        late_ranked.push(ranked[0].clone());
        assert!(
            !expand_ranked_owner_members(
                &conn,
                &mut Vec::new(),
                &late_ranked,
                "planner body",
                None,
                None,
                false
            )
            .unwrap()
        );
    }

    #[test]
    fn lookup_and_search_skip_documentation_links_recorded_as_imports() {
        let conn = search_fixture(
            &[
                "('i1', 'f_i1', 'src/app.py', 'python', 'Flask', 'import', 'from flask import Flask', NULL, NULL, NULL, 1, 0, 1, 20, 0, 20, NULL, NULL, NULL, NULL, NULL, NULL, 'h_i1', NULL, 0, 0, 'code')".to_string(),
                "('i2', 'f_i2', 'README.md', 'markdown', 'Flask', 'import', '[Flask](https://flask.dev)', NULL, NULL, NULL, 1, 0, 1, 20, 0, 20, NULL, NULL, NULL, NULL, NULL, NULL, 'h_i2', NULL, 0, 0, 'documentation')".to_string(),
            ]
            .join(","),
        );

        let lookup: Vec<String> = search_symbols(&conn, "Flask", Some("import"), true, 10)
            .unwrap()
            .into_iter()
            .map(|s| s.path)
            .collect();
        assert_eq!(lookup, vec!["src/app.py"]);
        let search: Vec<String> =
            fts_search_symbols_scoped(&conn, "Flask", Some("import"), None, true, 10)
                .unwrap()
                .into_iter()
                .map(|r| r.symbol.path)
                .collect();
        assert_eq!(search, vec!["src/app.py"]);
    }

    #[test]
    fn lookup_ranks_a_prefix_match_above_a_substring_match() {
        let conn = search_fixture(
            &[
                code_row("c1", "tests/a.rs", "rust", "test_test_client", ""),
                code_row("c2", "tests/b.rs", "rust", "test_client_open_environ", ""),
            ]
            .join(","),
        );

        let names: Vec<String> = search_symbols(&conn, "test_cli", None, true, 10)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();

        assert_eq!(names, vec!["test_client_open_environ", "test_test_client"]);
    }

    #[test]
    fn lookup_ranks_a_declared_property_above_a_self_assignment() {
        let property = |id: &str, path: &str, signature: &str| {
            format!(
                "('{id}', 'f_{id}', '{path}', 'python', 'debug', 'property', '{signature}', NULL, NULL, NULL, 1, 0, 1, 20, 0, 20, NULL, NULL, NULL, NULL, NULL, NULL, 'h_{id}', NULL, 0, 0, 'code')"
            )
        };
        let conn = search_fixture(
            &[
                property("p1", "src/flask/app.py", "self.debug = get_debug_flag()"),
                property(
                    "p2",
                    "src/flask/sansio/app.py",
                    "@property def debug(self) -> bool",
                ),
            ]
            .join(","),
        );

        let paths: Vec<String> = search_symbols(&conn, "debug", None, false, 10)
            .unwrap()
            .into_iter()
            .map(|s| s.path)
            .collect();

        assert_eq!(paths, vec!["src/flask/sansio/app.py", "src/flask/app.py"]);
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
    fn test_name_rule_and_its_sql_mirror_agree_on_every_name() {
        let conn = Connection::open_in_memory().unwrap();
        let sql = format!(
            "SELECT {} FROM (SELECT :name AS name) s",
            names_a_test_sql("s")
        );
        let mut stmt = conn.prepare(&sql).unwrap();
        for name in [
            "test_run",
            "TestRoutes",
            "RoutesTest",
            "RoutesTests",
            "runSpec",
            "Specs",
            "create_app",
            "attest_value",
            "inspection",
        ] {
            let from_sql: bool = stmt
                .query_row(rusqlite::named_params! { ":name": name }, |row| row.get(0))
                .unwrap();
            assert_eq!(from_sql, names_a_test(name), "sql mirror: {name}");
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
    fn lookup_trigram_candidates_match_the_scan_path_for_literal_substrings() {
        let rows = [
            code_row("a", "src/lib.rs", "rust", "needle", ""),
            code_row("b", "src/lib.rs", "rust", "prefixneedlepostfix", ""),
            code_row("c", "src/lib.rs", "rust", "quoted \"needle\"", ""),
            code_row("d", "src/lib.rs", "rust", "under_score", ""),
            code_row("e", "src/lib.rs", "rust", "percent%wild", ""),
            code_row("f", "src/lib.rs", "rust", "two  spaces", ""),
            code_row("g", "src/lib.rs", "rust", "λambda", ""),
            code_row("h", "src/lib.rs", "rust", " padded ", ""),
        ]
        .join(", ");
        let indexed = search_fixture(&rows);
        let scan_path = search_fixture(&rows);
        scan_path
            .execute_batch("DROP TABLE symbol_names_tri;")
            .unwrap();

        for (query, kind, path) in [
            ("needle", None, None),
            ("NeEdLe", None, None),
            ("\"needle\"", Some("function"), None),
            ("_score", None, None),
            ("%wild", None, None),
            ("two  s", None, None),
            ("λam", None, None),
            (" padded ", None, None),
            ("not-present", None, None),
            ("needle", Some("function"), Some("src")),
        ] {
            let indexed_rows: Vec<_> =
                search_symbols_scoped(&indexed, query, kind, path, false, 20)
                    .unwrap()
                    .into_iter()
                    .map(|symbol| (symbol.symbol_id, symbol.path, symbol.name))
                    .collect();
            let scan_rows: Vec<_> = search_symbols_scoped(&scan_path, query, kind, path, false, 20)
                .unwrap()
                .into_iter()
                .map(|symbol| (symbol.symbol_id, symbol.path, symbol.name))
                .collect();
            assert_eq!(indexed_rows, scan_rows, "query: {query}");
        }
    }

    #[test]
    fn lookup_uses_trigram_candidates_for_plain_substring_queries() {
        let conn = search_fixture(
            &[
                code_row("a", "src/lib.rs", "rust", "needle", ""),
                code_row("b", "src/lib.rs", "rust", "prefixneedlepostfix", ""),
            ]
            .join(", "),
        );
        LOOKUP_TRIGRAM_EXECUTIONS.with(|count| count.set(0));

        let rows = search_symbols_scoped(&conn, "needle", None, None, false, 20).unwrap();

        assert_eq!(
            rows.into_iter()
                .map(|symbol| symbol.name)
                .collect::<Vec<_>>(),
            vec!["needle".to_string(), "prefixneedlepostfix".to_string()]
        );
        let executions = LOOKUP_TRIGRAM_EXECUTIONS.with(std::cell::Cell::get);
        assert_eq!(executions, 1);
    }

    #[test]
    fn lookup_falls_back_when_trigram_search_is_unsupported_or_unready() {
        let conn = search_fixture(&code_row("a", "src/lib.rs", "rust", "needle", ""));
        LOOKUP_TRIGRAM_EXECUTIONS.with(|count| count.set(0));
        for (query, kind) in [
            ("ab", None),
            ("needle.part", None),
            ("need\0le", None),
            ("needle", Some("variable")),
        ] {
            search_symbols_scoped(&conn, query, kind, None, false, 20).unwrap();
        }
        assert_eq!(LOOKUP_TRIGRAM_EXECUTIONS.with(std::cell::Cell::get), 0);

        conn.execute_batch("DROP TABLE symbol_names_tri;").unwrap();
        search_symbols_scoped(&conn, "needle", None, None, false, 20).unwrap();
        assert_eq!(LOOKUP_TRIGRAM_EXECUTIONS.with(std::cell::Cell::get), 0);

        let stale = search_fixture(&code_row("b", "src/lib.rs", "rust", "needle", ""));
        stale
            .execute(
                "UPDATE artifact_metadata SET value = 'stale' WHERE key = 'fts_rule'",
                [],
            )
            .unwrap();
        search_symbols_scoped(&stale, "needle", None, None, false, 20).unwrap();
        assert_eq!(LOOKUP_TRIGRAM_EXECUTIONS.with(std::cell::Cell::get), 0);
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
            search_symbols_sql(false, false, 20, false)
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
    fn lookup_trigram_statement_uses_the_virtual_index_without_scanning_symbols() {
        let conn = search_fixture(&code_row("a", "src/lib.rs", "rust", "needle", ""));
        let phrase = lookup_trigram_phrase("needle", None).unwrap();
        let sql = format!(
            "EXPLAIN QUERY PLAN {}",
            search_symbols_sql(false, false, 20, true)
        );
        let plan: Vec<String> = conn
            .prepare(&sql)
            .unwrap()
            .query_map(
                rusqlite::named_params! {
                    ":query": "needle",
                    ":pattern": "%needle%",
                    ":prefix": "needle%",
                    ":kind": None::<&str>,
                    ":path": None::<&str>,
                    ":path_like": None::<&str>,
                    ":trigram": phrase,
                },
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter().any(|step| step.contains("VIRTUAL TABLE INDEX")),
            "{plan:?}"
        );
        assert!(
            plan.iter()
                .any(|step| step.contains("SEARCH s USING INTEGER PRIMARY KEY")),
            "{plan:?}"
        );
        assert!(
            !plan
                .iter()
                .any(|step| step == "SCAN s" || step.starts_with("SCAN s ")),
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
    fn related_tests_match_underscores_literally_and_skip_dunder_names() {
        let conn = search_fixture(
            &[
                code_row("c1", "src/app.py", "python", "wsgi_app", ""),
                code_row("c2", "src/app.py", "python", "__init__", ""),
                "('t1', 'f_t1', 'tests/test_app.py', 'python', 'test_wsgi_app', 'function', 'def test_wsgi_app()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_t1', NULL, 1, 0, 'code')".to_string(),
                "('t2', 'f_t2', 'tests/test_app.py', 'python', 'test_wsgiXapp', 'function', 'def test_wsgiXapp()', NULL, NULL, NULL, 6, 0, 9, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_t2', NULL, 1, 0, 'code')".to_string(),
                "('t3', 'f_t3', 'tests/test_views.py', 'python', 'test_init_once', 'function', 'def test_init_once()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_t3', NULL, 1, 0, 'code')".to_string(),
            ]
            .join(","),
        );
        let names = |name: &str| -> Vec<String> {
            let target = get_symbol_by_name(&conn, name, None).unwrap().unwrap();
            find_related_tests(&conn, &target, 5)
                .unwrap()
                .into_iter()
                .map(|t| t.name)
                .collect()
        };

        assert_eq!(names("wsgi_app"), vec!["test_wsgi_app"]);
        assert!(names("__init__").is_empty());
    }

    #[test]
    fn related_tests_admit_unflagged_test_names_in_test_paths_without_helpers_or_low_signal_rows() {
        let conn = search_fixture(
            "('target', 'f_target', 'src/core.rs', 'rust', 'calculate', 'function', 'fn calculate()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_target', NULL, 0, 0, 'code'),
              ('direct', 'f_direct', 'tests/direct.rs', 'rust', 'test_direct', 'function', 'fn test_direct()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_direct', NULL, 0, 0, 'code'),
              ('pending', 'f_pending', 'autotests/tst_pending.rs', 'rust', 'test_pending', 'function', 'fn test_pending()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_pending', NULL, 0, 0, 'code'),
              ('qml_pending', 'f_qml_pending', 'autotests/tst_qml_pending.qml', 'qml', 'test_qml_pending', 'function', 'function test_qml_pending() {}', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_qml_pending', NULL, 0, 0, 'code'),
              ('name', 'f_name', 'tests/name.rs', 'rust', 'test_calculate_named', 'function', 'fn test_calculate_named()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_name', NULL, 0, 0, 'code'),
              ('fts', 'f_fts', 'tests/fts.rs', 'rust', 'test_fts', 'function', 'fn test_fts()', 'calculate behavior', NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_fts', NULL, 0, 0, 'code'),
              ('local', 'f_local', 'tests/name.rs', 'rust', 'calculate_local', 'variable', 'let calculate_local = 1;', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_local', NULL, 0, 0, 'code'),
              ('import', 'f_import', 'tests/name.rs', 'rust', 'calculate_import', 'import', 'use calculate_import;', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_import', NULL, 0, 0, 'code'),
              ('production', 'f_production', 'src/testing.rs', 'rust', 'calculate_production', 'function', 'fn calculate_production()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_production', NULL, 0, 0, 'code'),
              ('helper', 'f_helper', 'tests/apps/factory.rs', 'rust', 'build_calculate_app', 'function', 'fn build_calculate_app()', NULL, NULL, NULL, 1, 0, 5, 1, 0, 50, NULL, NULL, NULL, NULL, NULL, NULL, 'h_helper', NULL, 0, 0, 'code')",
        );
        conn.execute_batch(
            "CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT, path TEXT, start_line INTEGER, start_column INTEGER);
             CREATE TABLE pending_relationships (from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT, path TEXT, start_line INTEGER, start_column INTEGER, target_receiver TEXT, target_namespace_json TEXT, target_display_name TEXT);
             CREATE TABLE type_facts (type_fact_id TEXT, symbol_id TEXT, language TEXT, resolved_type TEXT, generic_params_json TEXT);
             INSERT INTO relationships VALUES ('direct', 'target', 'calls', 'tests/direct.rs', 1, 0);
             INSERT INTO pending_relationships VALUES ('pending', 'calculate', 'calls', 'autotests/tst_pending.rs', 1, 0, NULL, '[]', 'calculate');
             INSERT INTO pending_relationships VALUES ('qml_pending', 'calculate', 'calls', 'autotests/tst_qml_pending.qml', 1, 0, NULL, '[]', 'calculate');",
        )
        .unwrap();
        let target = get_symbol_by_name(&conn, "calculate", None)
            .unwrap()
            .unwrap();

        let names: Vec<String> = find_related_tests(&conn, &target, 5)
            .unwrap()
            .into_iter()
            .map(|test| test.name)
            .collect();

        assert_eq!(
            names,
            vec![
                "test_direct",
                "test_pending",
                "test_calculate_named",
                "test_fts"
            ]
        );
        assert!(!names.iter().any(|name| name == "test_qml_pending"));
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
    fn a_full_and_word_branch_does_not_run_the_or_query() {
        let mut rows: Vec<String> = (1..=40)
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
        WORD_QUERY_EXECUTIONS.with(|count| count.set(0));

        let candidates =
            collect_search_candidates(&conn, "restore offline", None, None, false, 10).unwrap();

        assert_eq!(WORD_QUERY_EXECUTIONS.with(|count| count.get()), 1);
        let word_rows: Vec<&Candidate> = candidates.iter().filter(|c| c.word_match).collect();
        assert_eq!(word_rows.len(), 40);
        assert!(
            word_rows
                .iter()
                .all(|candidate| candidate.result.symbol.name.starts_with("both_"))
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
            vec!["parse", "sha256", "sha", "sidecar"]
        );
        assert_eq!(trigram_name_terms("isReady"), vec!["isready", "ready"]);
        assert_eq!(trigram_name_terms("the before"), vec!["the", "before"]);
        assert_eq!(trigram_name_terms("8080 404"), vec!["8080", "404"]);
        assert!(trigram_name_terms("ab").is_empty());
    }

    #[test]
    fn the_or_query_leaves_out_digit_parts_only_beside_a_word() {
        assert_eq!(
            sanitize_fts5_query("zzq1_thing").1,
            "\"zzq\"* OR \"thing\"* OR \"zzq1_thing\"*"
        );
        assert_eq!(
            sanitize_fts5_query("retry 3 times").1,
            "\"retry\"* OR \"times\"* OR \"retry3times\"*"
        );
        assert_eq!(
            sanitize_fts5_query("retry 3 times").0,
            "(\"retry\"* AND \"3\" AND \"times\"*) OR \"retry3times\"*"
        );
        assert_eq!(
            sanitize_fts5_query("8080 1").1,
            "\"8080\"* OR \"1\" OR \"80801\"*"
        );
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
            owner_match: false,
            name_terms: Vec::new(),
            documentation: false,
            nested: false,
            owner: None,
            owner_context: None,
            file_module: None,
            module_owner: None,
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
    fn credited_fields_require_whole_name_multi_field_evidence_and_query_coverage() {
        let terms = vec![
            ("wsgi".into(), "name".into(), 3.0),
            ("response".into(), "signature".into(), TEXT_CREDIT),
            ("request".into(), "doc".into(), TEXT_CREDIT),
            ("error".into(), "doc".into(), TEXT_CREDIT),
        ];
        assert_eq!(credited_field_bonus(&terms), FIELD_DIVERSITY_BONUS);
        assert_eq!(credited_field_bonus(&terms[..2]), 0.0);
        assert_eq!(
            credited_field_bonus(&[
                ("wsgi".into(), "name".into(), 2.0),
                ("response".into(), "signature".into(), TEXT_CREDIT),
                ("request".into(), "doc".into(), TEXT_CREDIT),
                ("error".into(), "doc".into(), TEXT_CREDIT),
            ]),
            0.0
        );
        let sparse_terms = [
            ("terminal".into(), "name".into(), 3.0),
            ("docker".into(), "signature".into(), TEXT_CREDIT),
            ("local".into(), "doc".into(), TEXT_CREDIT),
            ("sandbox".into(), "doc".into(), TEXT_CREDIT),
            ("seven".into(), "none".into(), 0.0),
            ("ssh".into(), "none".into(), 0.0),
            ("singularity".into(), "none".into(), 0.0),
            ("modal".into(), "none".into(), 0.0),
            ("daytona".into(), "none".into(), 0.0),
            ("vercel".into(), "none".into(), 0.0),
        ];
        assert_eq!(credited_field_bonus(&sparse_terms), 0.0);
        assert_eq!(
            credited_field_bonus(&[
                ("response".into(), "name".into(), 3.0),
                ("request".into(), "name".into(), 3.0),
            ]),
            0.0
        );
    }

    #[test]
    fn credited_field_bonus_requires_two_distinct_document_terms() {
        let terms = [
            ("strip".into(), "name".into(), 3.0),
            ("path".into(), "signature".into(), TEXT_CREDIT),
            ("windows".into(), "doc".into(), TEXT_CREDIT),
        ];
        assert_eq!(credited_field_bonus(&terms), 0.0);
    }

    #[test]
    fn name_coverage_accepts_token_runs_substrings_and_stems() {
        let strengths = |name: &str, query: &str| {
            let stemmer = Stemmer::create(Algorithm::English);
            let words = stemmed_query_words(query, &stemmer);
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
        row.result.symbol.doc_comment = Some(format!("{}settings", "é".repeat(321)));
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
        assert_eq!(only_launcher[0].0.symbol.path, "scripts/launcher.ts");
        assert!(only_launcher.iter().all(|(_, e)| e.path_role == 0.0));

        let other_file = rows("archive verify checksum");
        assert_eq!(other_file[0].0.symbol.path, "src/archive.rs");
        assert_eq!(other_file[1].1.path_role, W_PATH_ROLE);

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

    fn credit_of<'a>(explain: &'a SearchExplain, word: &str) -> (&'a str, f64) {
        explain
            .terms
            .iter()
            .find(|(term, _, _)| term == word)
            .map(|(_, field, credit)| (field.as_str(), *credit))
            .unwrap()
    }

    #[test]
    fn a_word_that_names_the_module_credits_less_than_a_signature_match() {
        let mut signed = plain_candidate("render", "function", "src/view.rs");
        signed.result.symbol.signature = Some("fn render(blog: &Blog)".into());
        let rows = ranked(
            vec![
                plain_candidate("render", "function", "src/blogs/mod.rs"),
                plain_candidate("render", "function", "src/blog.rs"),
                plain_candidate("render", "function", "src/blog_post.rs"),
                plain_candidate("render", "function", "src/blog/render.rs"),
                signed,
            ],
            "render blog",
        );
        let row = |path: &str| {
            &rows
                .iter()
                .find(|(result, _)| result.symbol.path == path)
                .unwrap()
                .1
        };

        assert_eq!(rows[0].0.symbol.path, "src/view.rs");
        assert_eq!(credit_of(&rows[0].1, "blog"), ("signature", TEXT_CREDIT));
        for path in ["src/blogs/mod.rs", "src/blog.rs"] {
            assert_eq!(credit_of(row(path), "blog"), ("path", PATH_CREDIT));
            assert_eq!(covered_query_term_count(&row(path).terms), 1);
        }
        for path in ["src/blog_post.rs", "src/blog/render.rs"] {
            assert_eq!(credit_of(row(path), "blog"), ("none", 0.0));
        }

        let spelled = ranked(
            vec![plain_candidate("render", "function", "src/blog_post.rs")],
            "render blog post",
        );
        assert_eq!(credit_of(&spelled[0].1, "blog"), ("path", PATH_CREDIT));
        assert_eq!(credit_of(&spelled[0].1, "post"), ("path", PATH_CREDIT));
    }

    #[test]
    fn a_manifest_path_is_read_relative_to_its_folder_without_dot_segments() {
        assert_eq!(
            joined_path("pkg", "./src/../src/root.zig").as_deref(),
            Some("pkg/src/root.zig")
        );
        assert_eq!(
            joined_path("", "src/main.zig").as_deref(),
            Some("src/main.zig")
        );
        assert_eq!(joined_path("", "../outside.zig"), None);
    }

    #[test]
    fn each_language_names_the_module_of_its_entry_files() {
        let names = |path: &str, language: &str| module_names(path, language, None, None);
        let with = |path: &str, language: &str, file: Option<&str>, owner: Option<&str>| {
            module_names(path, language, file, owner)
        };

        assert_eq!(names("lua/blog/init.lua", "lua"), ["blog"]);
        assert_eq!(names("app/init.py", "python"), ["init"]);
        assert_eq!(names("py/blog/__main__.py", "python"), ["blog"]);
        assert_eq!(names("py/blog/__init__.pyi", "python"), ["blog"]);
        assert_eq!(names("web/blog/index.tsx", "tsx"), ["blog"]);
        assert_eq!(names("deno/blog/mod.ts", "typescript"), ["blog"]);
        assert_eq!(names("src/blog/mod.rs", "rust"), ["blog"]);
        let crate_root = "crates/my-crate/src/lib.rs";
        let cargo_name = Some("name = \"blog-kit\"");
        assert_eq!(with(crate_root, "rust", cargo_name, None), ["blog-kit"]);
        assert_eq!(names(crate_root, "rust"), ["my-crate"]);
        assert_eq!(
            names("crates\\my-crate\\src\\main.rs", "rust"),
            ["my-crate"]
        );
        assert!(names("src/lib.rs", "rust").is_empty());
        assert_eq!(names("src/bin/tool/main.rs", "rust"), ["tool"]);
        assert_eq!(names("src/bin/tool.rs", "rust"), ["tool"]);
        let util = with("src/blog/util.rs", "rust", None, Some("inner"));
        assert_eq!(util, ["util", "inner"]);
        let server = with("pkg/v2/server.go", "go", Some("blogsvc"), None);
        assert_eq!(server, ["blogsvc", "server"]);
        let command = with("pkg/cmd/tool/main.go", "go", Some("main"), None);
        assert_eq!(command, ["tool", "main"]);
        assert_eq!(names("Sources/BlogKit/main.swift", "swift"), ["BlogKit"]);
        assert_eq!(names("Sources/BlogKit/Post.swift", "swift"), ["Post"]);
        let scala = with("blog/package.scala", "scala", None, Some("blog"));
        assert_eq!(scala, ["blog"]);
        assert_eq!(names("zig/src/root.zig", "zig"), ["zig"]);
        assert_eq!(names("src/blog.zig", "zig"), ["blog"]);
        let elixir = with(
            "lib/my_app/blog.ex",
            "elixir",
            None,
            Some("MyApp.Blog.Post"),
        );
        assert_eq!(elixir, ["blog", "Post"]);
        let fsharp = with("src/Posts.fs", "fsharp", None, Some("Blog.Posts"));
        assert_eq!(fsharp, ["Posts"]);
        let qmldir = with("qml/Blog/qmldir", "qmldir", Some("Org.Blog"), None);
        assert_eq!(qmldir, ["Blog"]);
        assert_eq!(names("lib/blog.dart", "dart"), ["blog"]);
        let library = with("lib/src/posts_impl.dart", "dart", Some("blog.posts"), None);
        assert_eq!(library, ["posts_impl", "posts"]);
        let target = with("Sources/BlogKit/Post.swift", "swift", Some("BlogKit"), None);
        assert_eq!(target, ["BlogKit", "Post"]);
        let executable = with("Sources/Tool/main.swift", "swift", Some("Tool"), None);
        assert_eq!(executable, ["Tool"]);
        assert_eq!(
            with("pkg/src/root.zig", "zig", Some("blogz"), None),
            ["blogz"]
        );
        let module_root = with("pkg/src/lib.zig", "zig", Some("blogz"), None);
        assert_eq!(module_root, ["blogz", "lib"]);
        assert_eq!(names("src/Blog/Post.php", "php"), ["Post"]);
        assert!(with("tests/blog/init.lua", "lua", None, Some("Blog")).is_empty());
    }

    #[test]
    fn a_path_word_lifts_a_role_folder_but_never_credits_a_test_or_vendored_path() {
        let rows = ranked(
            vec![
                plain_candidate("update", "function", "examples/tutorial/flaskr/blog.py"),
                plain_candidate("update", "function", "tests/blog.rs"),
                plain_candidate("update", "function", "vendor/blog/lib.rs"),
                plain_candidate("update", "function", "src/fixtures/blog.rs"),
            ],
            "update blog",
        );
        let row = |path: &str| {
            &rows
                .iter()
                .find(|(result, _)| result.symbol.path == path)
                .unwrap()
                .1
        };

        let example = row("examples/tutorial/flaskr/blog.py");
        assert_eq!(credit_of(example, "blog"), ("path", PATH_CREDIT));
        assert_eq!(example.path_role, 0.0);
        assert_eq!(credit_of(row("tests/blog.rs"), "blog"), ("none", 0.0));
        for copied in ["vendor/blog/lib.rs", "src/fixtures/blog.rs"] {
            assert_eq!(credit_of(row(copied), "blog"), ("none", 0.0));
            assert_eq!(row(copied).path_role, W_PATH_ROLE);
        }

        let unnamed = ranked(
            vec![plain_candidate(
                "update",
                "function",
                "examples\\tutorial\\flaskr\\blog.py",
            )],
            "update post",
        );
        assert_eq!(unnamed[0].1.path_role, W_PATH_ROLE);
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
    fn a_helper_nested_in_a_function_ranks_below_an_equal_module_function() {
        let mut helper = plain_candidate("run_test", "function", "tests/test_basic.py");
        helper.nested = true;
        let module_function = plain_candidate("run_dotenv", "function", "tests/test_basic.py");

        let rows = rerank_with(vec![helper, module_function], "run", true, None);

        assert_eq!(rows[0].0.symbol.name, "run_dotenv");
        assert_eq!(rows[1].1.nested, W_NESTED);
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
                ('sf_route', 'f2', 'src/routes/api.rs', 'rust', 'axum.route.v1', 'get_users', 'function', NULL, 10, 20, 1.0, '{\"verb\":\"GET\",\"route_template\":\"/api/v1/users/{id}\",\"normalized_route_template\":\"/api/v1/users/:id\"}'),
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
        assert_eq!(
            facts_route[0].key.as_deref(),
            Some("GET /api/v1/users/{id}")
        );
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

    #[test]
    fn a_return_type_prints_only_when_the_signature_does_not_spell_it_outside_its_parameters() {
        let ty = || Some("Builder".to_string());
        assert_eq!(
            unspelled_return_type("method", Some("fn new() -> Self"), ty()).as_deref(),
            Some("Builder")
        );
        assert_eq!(
            unspelled_return_type("method", Some("fn from(b: Builder) -> Self"), ty()).as_deref(),
            Some("Builder")
        );
        assert_eq!(
            unspelled_return_type("function", Some("fn make() -> Builder"), ty()),
            None
        );
        assert_eq!(
            unspelled_return_type("method", Some("public Builder build(int size)"), ty()),
            None
        );
        assert_eq!(
            unspelled_return_type("function", Some("fn make() -> BuilderSet"), ty()).as_deref(),
            Some("Builder")
        );
        assert_eq!(
            unspelled_return_type("variable", Some("let b = make()"), ty()),
            None
        );
        assert_eq!(
            unspelled_return_type("function", Some("fn f()"), None),
            None
        );
    }
}
