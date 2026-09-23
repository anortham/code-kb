use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::queries::QueryError;
use crate::workspace::{normalize_path, to_forward_slash};

#[derive(Debug, Clone)]
pub struct ToolInvocation<'a> {
    pub tool: &'a str,
    pub duration_ms: u64,
    pub outcome: &'a str, // "ok", "empty", "error"
    pub error_message: Option<&'a str>,
    pub logical_result_count: Option<usize>,
    pub bytes_returned: usize,
    pub est_tokens: usize,
    pub est_tokens_saved: usize,
    /// True when the answer had files to measure against, so `est_tokens_saved` is a real figure.
    pub est_tokens_saved_known: bool,
    pub reconcile_ms: Option<u64>,
    pub query_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TimeWindow {
    Today,
    Last7Days,
    Last30Days,
    ThisMonth,
    LastYear,
    #[default]
    AllTime,
}

impl TimeWindow {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "today" => Some(Self::Today),
            "7d" | "week" => Some(Self::Last7Days),
            "30d" => Some(Self::Last30Days),
            "month" | "this-month" => Some(Self::ThisMonth),
            "year" | "last-year" => Some(Self::LastYear),
            "all" | "all-time" => Some(Self::AllTime),
            _ => None,
        }
    }

    pub fn to_sqlite_condition(&self) -> Option<&'static str> {
        match self {
            Self::Today => Some("timestamp >= datetime('now', 'localtime', 'start of day')"),
            Self::Last7Days => Some("timestamp >= datetime('now', '-7 days')"),
            Self::Last30Days => Some("timestamp >= datetime('now', '-30 days')"),
            Self::ThisMonth => Some("timestamp >= datetime('now', 'localtime', 'start of month')"),
            Self::LastYear => Some("timestamp >= datetime('now', '-365 days')"),
            Self::AllTime => None,
        }
    }
}

impl std::fmt::Display for TimeWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Today => write!(f, "today"),
            Self::Last7Days => write!(f, "7d"),
            Self::Last30Days => write!(f, "30d"),
            Self::ThisMonth => write!(f, "month"),
            Self::LastYear => write!(f, "year"),
            Self::AllTime => write!(f, "all"),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelemetryFilter {
    pub time_window: TimeWindow,
    pub workspace_root: Option<PathBuf>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySummary {
    pub total_calls: usize,
    pub ok_calls: usize,
    pub empty_calls: usize,
    pub error_calls: usize,
    pub total_tokens_returned: usize,
    pub est_tokens_saved: usize,
    pub saved_known_calls: usize,
    pub time_window: TimeWindow,
    pub scope_description: String,
    pub tool_stats: Vec<ToolStat>,
    pub recent_errors: Vec<TelemetryErrorRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStat {
    pub tool: String,
    pub count: usize,
    pub ok_count: usize,
    pub empty_count: usize,
    pub error_count: usize,
    pub avg_duration_ms: u64,
    pub p50_ms: Option<u64>,
    pub p95_ms: Option<u64>,
    pub avg_reconcile_ms: Option<u64>,
    pub avg_query_ms: Option<u64>,
    pub tokens_returned: usize,
    pub tokens_saved: usize,
    pub saved_known_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryErrorRecord {
    pub timestamp: String,
    pub tool: String,
    pub error_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexFacts {
    pub extractor_version: Option<String>,
    pub schema_version: Option<String>,
    pub index_level: Option<String>,
    pub updated_at: Option<String>,
    pub file_count: i64,
    pub symbol_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BugReportBundle {
    pub os_info: String,
    pub arch_info: String,
    pub code_kb_version: String,
    pub julie_extract_version: String,
    pub active_workspace_name: Option<String>,
    pub index: Option<IndexFacts>,
    pub recent_errors: Vec<TelemetryErrorRecord>,
    pub log_tail: Vec<String>,
    pub markdown_body: String,
    pub github_issue_url: String,
}

static TELEMETRY_DIR_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn resolve_telemetry_dir(
    env_dir: Option<String>,
    cargo_target_tmp: Option<String>,
    home: Option<String>,
    userprofile: Option<String>,
) -> PathBuf {
    if let Some(dir) = env_dir
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir);
    }
    if let Some(target_tmp) = cargo_target_tmp
        && !target_tmp.trim().is_empty()
    {
        return PathBuf::from(target_tmp).join("test-telemetry");
    }
    if let Some(home) = home
        && !home.trim().is_empty()
    {
        return PathBuf::from(home).join(".code-kb");
    }
    if let Some(profile) = userprofile
        && !profile.trim().is_empty()
    {
        return PathBuf::from(profile).join(".code-kb");
    }
    PathBuf::from(".code-kb")
}

pub fn is_telemetry_disabled_with(no_telem: Option<&str>, disable_telem: Option<&str>) -> bool {
    no_telem
        .or(disable_telem)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

pub fn is_telemetry_disabled() -> bool {
    let no_telem = std::env::var("CODE_KB_NO_TELEMETRY").ok();
    let disable_telem = std::env::var("CODE_KB_DISABLE_TELEMETRY").ok();
    is_telemetry_disabled_with(no_telem.as_deref(), disable_telem.as_deref())
}

pub fn get_global_telemetry_dir() -> PathBuf {
    if let Ok(guard) = TELEMETRY_DIR_OVERRIDE.read()
        && let Some(ref path) = *guard
    {
        return path.clone();
    }
    resolve_telemetry_dir(
        std::env::var("CODE_KB_TELEMETRY_DIR").ok(),
        std::env::var("CARGO_TARGET_TMPDIR").ok(),
        std::env::var("HOME").ok(),
        std::env::var("USERPROFILE").ok(),
    )
}

#[cfg(test)]
pub(crate) fn set_telemetry_dir_override(path: Option<PathBuf>) {
    if let Ok(mut guard) = TELEMETRY_DIR_OVERRIDE.write() {
        *guard = path;
    }
}

pub fn open_global_telemetry_db() -> Result<Connection, QueryError> {
    open_telemetry_db_at(&get_global_telemetry_dir())
}

pub fn open_telemetry_db_at(dir: &Path) -> Result<Connection, QueryError> {
    if !dir.exists() {
        let _ = std::fs::create_dir_all(dir);
    }
    let db_path = dir.join("telemetry.db");
    let conn = Connection::open(&db_path)?;
    init_telemetry_db(&conn)?;
    Ok(conn)
}

fn init_telemetry_db(conn: &Connection) -> Result<(), QueryError> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         CREATE TABLE IF NOT EXISTS tool_telemetry (
             id TEXT PRIMARY KEY,
             timestamp TEXT NOT NULL,
             workspace_root TEXT NOT NULL,
             workspace_name TEXT NOT NULL,
             tool TEXT NOT NULL,
             duration_ms INTEGER NOT NULL,
             outcome TEXT NOT NULL,
             error_message TEXT,
             result_count INTEGER NOT NULL DEFAULT 0,
             result_count_known INTEGER NOT NULL DEFAULT 0,
             bytes_returned INTEGER NOT NULL DEFAULT 0,
             est_tokens INTEGER NOT NULL DEFAULT 0,
             est_tokens_saved INTEGER NOT NULL DEFAULT 0,
             est_tokens_saved_known INTEGER NOT NULL DEFAULT 0,
             code_kb_version TEXT NOT NULL,
             reconcile_ms INTEGER DEFAULT NULL,
             query_ms INTEGER DEFAULT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_tool_telemetry_tool ON tool_telemetry(tool, timestamp DESC);
         CREATE INDEX IF NOT EXISTS idx_tool_telemetry_ts ON tool_telemetry(timestamp DESC);
         DELETE FROM tool_telemetry WHERE timestamp < datetime('now', '-365 days');",
    )?;

    // Handle column migrations if table previously existed without workspace columns
    let mut stmt = conn.prepare("PRAGMA table_info(tool_telemetry)")?;
    let cols = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut col_names = HashSet::new();
    for col in cols.flatten() {
        col_names.insert(col);
    }
    if !col_names.contains("workspace_root") {
        conn.execute(
            "ALTER TABLE tool_telemetry ADD COLUMN workspace_root TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if !col_names.contains("workspace_name") {
        conn.execute(
            "ALTER TABLE tool_telemetry ADD COLUMN workspace_name TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    if !col_names.contains("est_tokens_saved") {
        conn.execute(
            "ALTER TABLE tool_telemetry ADD COLUMN est_tokens_saved INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !col_names.contains("est_tokens_saved_known") {
        conn.execute(
            "ALTER TABLE tool_telemetry ADD COLUMN est_tokens_saved_known INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !col_names.contains("result_count_known") {
        conn.execute(
            "ALTER TABLE tool_telemetry ADD COLUMN result_count_known INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !col_names.contains("reconcile_ms") {
        conn.execute(
            "ALTER TABLE tool_telemetry ADD COLUMN reconcile_ms INTEGER DEFAULT NULL",
            [],
        )?;
    }
    if !col_names.contains("query_ms") {
        conn.execute(
            "ALTER TABLE tool_telemetry ADD COLUMN query_ms INTEGER DEFAULT NULL",
            [],
        )?;
    }
    if col_names.contains("version") && !col_names.contains("code_kb_version") {
        conn.execute(
            "ALTER TABLE tool_telemetry RENAME COLUMN version TO code_kb_version",
            [],
        )?;
    } else if !col_names.contains("code_kb_version") {
        conn.execute(
            "ALTER TABLE tool_telemetry ADD COLUMN code_kb_version TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }

    // Dependent indexes must be created AFTER columns are verified to exist
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tool_telemetry_ws_ts ON tool_telemetry(workspace_root, timestamp DESC)",
        [],
    )?;

    Ok(())
}

/// Open the global telemetry database, delegating to `open_global_telemetry_db()`.
pub fn open_telemetry_db(_workspace_root: &Path) -> Result<Connection, QueryError> {
    open_global_telemetry_db()
}

/// Returns the normalized workspace root and an optional alternate representation
/// (e.g. resolving canonical path, or mapping macOS `/private/var`, `/private/tmp`, `/private/etc` symmetry).
pub(crate) fn workspace_root_match_candidates(ws: &Path) -> (String, Option<String>) {
    let norm = to_forward_slash(&normalize_path(ws));
    let canonical = dunce::canonicalize(ws)
        .map(|p| to_forward_slash(&normalize_path(&p)))
        .unwrap_or_else(|_| norm.clone());

    if canonical != norm {
        return (canonical, Some(norm));
    }

    // Handle macOS `/private/var`, `/private/tmp`, `/private/etc` symmetry
    if let Some(rest) = norm.strip_prefix("/private/") {
        if rest.starts_with("var/") || rest.starts_with("tmp/") || rest.starts_with("etc/") {
            return (norm.clone(), Some(format!("/{}", rest)));
        }
    } else if norm.starts_with("/var/") || norm.starts_with("/tmp/") || norm.starts_with("/etc/") {
        return (norm.clone(), Some(format!("/private{}", norm)));
    }

    (norm, None)
}

/// Fast record of a tool invocation with normalized workspace path attribution.
pub fn record_tool_call_conn(
    conn: &Connection,
    workspace_root: &Path,
    invocation: &ToolInvocation,
) {
    if is_telemetry_disabled() {
        return;
    }
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let ts = format!("{:?}", SystemTime::now());
    let id_source = format!("{}:{}:{}", invocation.tool, ts, now.as_nanos());
    let id = blake3::hash(id_source.as_bytes()).to_hex().to_string();
    let version = env!("CARGO_PKG_VERSION");

    let canonical =
        dunce::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let norm_ws = to_forward_slash(&normalize_path(&canonical));
    let ws_name = Path::new(&norm_ws)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());

    let _ = conn.execute(
        "INSERT INTO tool_telemetry (
            id, timestamp, workspace_root, workspace_name, tool,
            duration_ms, outcome, error_message, result_count, result_count_known,
            bytes_returned, est_tokens, est_tokens_saved, est_tokens_saved_known,
            code_kb_version, reconcile_ms, query_ms
        ) VALUES (?1, datetime('now'), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            id,
            norm_ws,
            ws_name,
            invocation.tool,
            invocation.duration_ms as i64,
            invocation.outcome,
            invocation.error_message,
            invocation.logical_result_count.unwrap_or_default() as i64,
            invocation.logical_result_count.is_some() as i64,
            invocation.bytes_returned as i64,
            invocation.est_tokens as i64,
            invocation.est_tokens_saved as i64,
            invocation.est_tokens_saved_known as i64,
            version,
            invocation.reconcile_ms.map(|v| v as i64),
            invocation.query_ms.map(|v| v as i64),
        ],
    );
}

/// Record a tool call to the global telemetry database.
/// Best-effort and non-panicking.
pub fn record_tool_call(workspace_root: &Path, invocation: &ToolInvocation) {
    if is_telemetry_disabled() {
        return;
    }
    if let Ok(conn) = open_global_telemetry_db() {
        record_tool_call_conn(&conn, workspace_root, invocation);
    }
}

fn calculate_percentile(sorted: &[u64], pct: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    Some(sorted[idx])
}

pub fn get_telemetry_summary(
    conn: &Connection,
    filter: &TelemetryFilter,
) -> Result<TelemetrySummary, QueryError> {
    let mut conditions = Vec::new();
    let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(time_cond) = filter.time_window.to_sqlite_condition() {
        conditions.push(time_cond.to_string());
    }

    if let Some(ref ws) = filter.workspace_root {
        let (canon, alt) = workspace_root_match_candidates(ws);
        if let Some(alt_val) = alt {
            let idx1 = params_vec.len() + 1;
            let idx2 = params_vec.len() + 2;
            conditions.push(format!(
                "(workspace_root = ?{} OR workspace_root = ?{})",
                idx1, idx2
            ));
            params_vec.push(Box::new(canon));
            params_vec.push(Box::new(alt_val));
        } else {
            let idx1 = params_vec.len() + 1;
            conditions.push(format!("workspace_root = ?{}", idx1));
            params_vec.push(Box::new(canon));
        }
    }

    if let Some(ref v) = filter.version {
        let idx = params_vec.len() + 1;
        conditions.push(format!("code_kb_version = ?{}", idx));
        params_vec.push(Box::new(v.clone()));
    }

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    let ws_desc = match &filter.workspace_root {
        Some(ws) => format!("Workspace: {}", to_forward_slash(&normalize_path(ws))),
        None => "Global (all workspaces)".to_string(),
    };
    let ver_desc = match &filter.version {
        Some(v) => format!("Version: {}", v),
        None => "Version: all".to_string(),
    };
    let scope_description = format!("{} | {}", ws_desc, ver_desc);

    let params_slice: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();

    // 1. Overall aggregations
    let totals_sql = format!(
        "SELECT COUNT(*),
                SUM(CASE WHEN outcome = 'ok' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'empty' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END),
                SUM(est_tokens),
                SUM(est_tokens_saved),
                SUM(CASE WHEN est_tokens_saved_known = 1 THEN 1 ELSE 0 END)
         FROM tool_telemetry
         {}",
        where_clause
    );

    let (
        total_calls,
        ok_calls,
        empty_calls,
        error_calls,
        total_tokens_returned,
        est_tokens_saved,
        saved_known_calls,
    ) = {
        let mut stmt = conn.prepare(&totals_sql)?;
        let row_mapper = |row: &rusqlite::Row| {
            let total: i64 = row.get(0)?;
            let ok: Option<i64> = row.get(1)?;
            let empty: Option<i64> = row.get(2)?;
            let error: Option<i64> = row.get(3)?;
            let tokens: Option<i64> = row.get(4)?;
            let tokens_saved: Option<i64> = row.get(5)?;
            let saved_known: Option<i64> = row.get(6)?;
            Ok((
                total as usize,
                ok.unwrap_or(0) as usize,
                empty.unwrap_or(0) as usize,
                error.unwrap_or(0) as usize,
                tokens.unwrap_or(0) as usize,
                tokens_saved.unwrap_or(0) as usize,
                saved_known.unwrap_or(0) as usize,
            ))
        };
        stmt.query_row(
            rusqlite::params_from_iter(params_slice.iter().copied()),
            row_mapper,
        )?
    };

    // 2. Collect durations for percentiles
    let mut durations_by_tool: HashMap<String, Vec<u64>> = HashMap::new();
    {
        let durations_sql = format!(
            "SELECT tool, duration_ms
             FROM tool_telemetry
             {}
             ORDER BY tool, duration_ms ASC",
            where_clause
        );
        let mut d_stmt = conn.prepare(&durations_sql)?;
        let d_rows = d_stmt.query_map(
            rusqlite::params_from_iter(params_slice.iter().copied()),
            |row| {
                let tool: String = row.get(0)?;
                let dur: i64 = row.get(1)?;
                Ok((tool, dur.max(0) as u64))
            },
        )?;
        for item in d_rows.flatten() {
            durations_by_tool.entry(item.0).or_default().push(item.1);
        }
    }

    // 3. Per-tool statistics
    let tool_stats_sql = format!(
        "SELECT tool,
                COUNT(*),
                SUM(CASE WHEN outcome = 'ok' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'empty' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END),
                ROUND(AVG(duration_ms)),
                SUM(est_tokens),
                SUM(est_tokens_saved),
                SUM(CASE WHEN est_tokens_saved_known = 1 THEN 1 ELSE 0 END),
                ROUND(AVG(reconcile_ms)),
                ROUND(AVG(query_ms))
         FROM tool_telemetry
         {}
         GROUP BY tool
         ORDER BY COUNT(*) DESC",
        where_clause
    );

    let mut tool_stats = Vec::new();
    {
        let mut stmt = conn.prepare(&tool_stats_sql)?;
        let row_mapper = |row: &rusqlite::Row| {
            let tool: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            let ok_count: Option<i64> = row.get(2)?;
            let empty_count: Option<i64> = row.get(3)?;
            let error_count: Option<i64> = row.get(4)?;
            let avg_duration: Option<f64> = row.get(5)?;
            let tokens: Option<i64> = row.get(6)?;
            let tokens_saved: Option<i64> = row.get(7)?;
            let saved_known: Option<i64> = row.get(8)?;
            let avg_rec: Option<f64> = row.get(9)?;
            let avg_q: Option<f64> = row.get(10)?;

            let (p50, p95) = if let Some(durs) = durations_by_tool.get(&tool) {
                (
                    calculate_percentile(durs, 50.0),
                    calculate_percentile(durs, 95.0),
                )
            } else {
                (None, None)
            };

            Ok(ToolStat {
                tool,
                count: count as usize,
                ok_count: ok_count.unwrap_or(0) as usize,
                empty_count: empty_count.unwrap_or(0) as usize,
                error_count: error_count.unwrap_or(0) as usize,
                avg_duration_ms: avg_duration.unwrap_or(0.0).round() as u64,
                p50_ms: p50,
                p95_ms: p95,
                avg_reconcile_ms: avg_rec.map(|v| v.round() as u64),
                avg_query_ms: avg_q.map(|v| v.round() as u64),
                tokens_returned: tokens.unwrap_or(0) as usize,
                tokens_saved: tokens_saved.unwrap_or(0) as usize,
                saved_known_count: saved_known.unwrap_or(0) as usize,
            })
        };

        let rows = stmt.query_map(
            rusqlite::params_from_iter(params_slice.iter().copied()),
            row_mapper,
        )?;
        for stat in rows.flatten() {
            tool_stats.push(stat);
        }
    }

    // 4. Recent errors
    let error_where_clause = if where_clause.is_empty() {
        "WHERE outcome = 'error' AND error_message IS NOT NULL".to_string()
    } else {
        format!(
            "{} AND outcome = 'error' AND error_message IS NOT NULL",
            where_clause
        )
    };

    let errors_sql = format!(
        "SELECT timestamp, tool, error_message
         FROM tool_telemetry
         {}
         ORDER BY timestamp DESC
         LIMIT 10",
        error_where_clause
    );

    let mut recent_errors = Vec::new();
    {
        let mut stmt = conn.prepare(&errors_sql)?;
        let row_mapper = |row: &rusqlite::Row| {
            let raw_msg: String = row.get(2)?;
            Ok(TelemetryErrorRecord {
                timestamp: row.get(0)?,
                tool: row.get(1)?,
                error_message: sanitize_error_message(&raw_msg),
            })
        };

        let rows = stmt.query_map(
            rusqlite::params_from_iter(params_slice.iter().copied()),
            row_mapper,
        )?;
        for err in rows.flatten() {
            recent_errors.push(err);
        }
    }

    Ok(TelemetrySummary {
        total_calls,
        ok_calls,
        empty_calls,
        error_calls,
        total_tokens_returned,
        est_tokens_saved,
        saved_known_calls,
        time_window: filter.time_window,
        scope_description,
        tool_stats,
        recent_errors,
    })
}

fn sanitize_error_message(msg: &str) -> String {
    let home_candidates = home_candidates();
    sanitize_error_message_with_homes(msg, &home_candidates)
}

/// Replaces the user's home directory with `~` and keeps everything else, including newlines.
fn mask_home_paths(text: &str) -> String {
    mask_home_paths_with(text, &home_candidates())
}

fn home_candidates() -> Vec<String> {
    let mut candidates = Vec::new();
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(var)
            && !home.trim().is_empty()
            && home != "/"
        {
            let simplified = dunce::simplified(Path::new(&home))
                .to_string_lossy()
                .to_string();
            if simplified != home {
                candidates.push(simplified);
            }
            candidates.push(home);
        }
    }
    candidates
}

fn mask_home_paths_with(text: &str, home_candidates: &[String]) -> String {
    let mut masked = text.to_string();
    for home in home_candidates {
        let norm_home = to_forward_slash(&normalize_path(Path::new(home)));
        masked = masked.replace(home.as_str(), "~");
        if norm_home != *home {
            masked = masked.replace(&norm_home, "~");
        }
        let backslash_home = home.replace('/', "\\");
        if backslash_home != *home {
            masked = masked.replace(&backslash_home, "~");
        }
    }
    masked
}

fn sanitize_error_message_with_homes(msg: &str, home_candidates: &[String]) -> String {
    let mut sanitized = mask_home_paths_with(msg, home_candidates);

    // Replace newlines with spaces to avoid breaking markdown tables
    sanitized = sanitized.replace("\r\n", " ").replace(['\n', '\r'], " ");

    // Escape markdown table pipe characters
    sanitized = sanitized.replace('|', "\\|");

    // Truncate message to 500 characters
    if sanitized.chars().count() > 500 {
        let mut truncated: String = sanitized.chars().take(500).collect();
        if truncated.ends_with('\\') && !truncated.ends_with("\\\\") {
            truncated.pop();
        }
        truncated
    } else {
        sanitized
    }
}

fn index_facts(workspace_root: &Path) -> Option<IndexFacts> {
    let db_path = workspace_root.join(".code-kb").join("artifact.db");
    let conn = crate::db::open_read_only(&db_path).ok()?;
    let metadata = |key: &str| {
        conn.query_row(
            "SELECT value FROM artifact_metadata WHERE key = ?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .ok()
    };
    let count = |table: &str| {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0)
    };
    Some(IndexFacts {
        extractor_version: metadata("binary_version"),
        schema_version: metadata("schema_version"),
        index_level: metadata("index_level"),
        updated_at: metadata("updated_at"),
        file_count: count("files"),
        symbol_count: count("symbols"),
    })
}

fn log_tail(workspace_root: &Path, lines: usize) -> Vec<String> {
    let mut tail: Vec<String> = Vec::new();
    for path in crate::workspace::log_files_newest_first(&crate::workspace::log_dir(workspace_root))
    {
        if tail.len() >= lines {
            break;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        let wanted = lines - tail.len();
        let file_lines: Vec<&str> = content.lines().collect();
        let mut older: Vec<String> = file_lines[file_lines.len().saturating_sub(wanted)..]
            .iter()
            .map(|line| mask_home_paths(line))
            .collect();
        older.append(&mut tail);
        tail = older;
    }
    tail
}

/// Browsers and GitHub truncate query strings past a few kilobytes.
const MAX_ISSUE_URL_LEN: usize = 8000;

fn build_issue_url(title: &str, body: &str) -> Result<url::Url, QueryError> {
    let mut issue_url = url::Url::parse("https://github.com/anortham/code-kb/issues/new")
        .map_err(|e| QueryError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
    issue_url
        .query_pairs_mut()
        .append_pair("title", title)
        .append_pair("body", body);
    Ok(issue_url)
}

/// Builds the diagnostic bundle for a GitHub issue. `markdown_body` is complete. The
/// pre-filled issue URL omits the log tail, and falls back to the environment and index
/// sections alone when the rest would push it past `MAX_ISSUE_URL_LEN`.
pub fn generate_bug_report(
    conn: &Connection,
    workspace_root: Option<&Path>,
    issue_title: Option<&str>,
    description: Option<&str>,
    log_lines: usize,
) -> Result<BugReportBundle, QueryError> {
    let os_info = std::env::consts::OS.to_string();
    let arch_info = std::env::consts::ARCH.to_string();
    let code_kb_version = env!("CARGO_PKG_VERSION").to_string();

    let exe_name = if cfg!(windows) {
        "julie-extract.exe"
    } else {
        "julie-extract"
    };

    let sibling_binary = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(exe_name)))
        .filter(|p| p.is_file());

    let julie_extract_version = if let Some(bin) = sibling_binary {
        if let Ok(output) = std::process::Command::new(&bin).arg("--version").output() {
            let ver = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !ver.is_empty() {
                ver
            } else {
                crate::sync::PINNED_JULIE_VERSION.to_string()
            }
        } else {
            crate::sync::PINNED_JULIE_VERSION.to_string()
        }
    } else {
        crate::sync::PINNED_JULIE_VERSION.to_string()
    };

    let active_workspace_name = workspace_root.map(|ws| {
        let norm = to_forward_slash(&normalize_path(ws));
        Path::new(&norm)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "repo".to_string())
    });

    // Query recent errors
    let mut recent_errors = Vec::new();
    let (error_sql, ws_param, alt_ws_param) = if let Some(ws) = workspace_root {
        let (canon, alt) = workspace_root_match_candidates(ws);
        if alt.is_some() {
            (
                "SELECT timestamp, tool, error_message
                 FROM tool_telemetry
                 WHERE outcome = 'error' AND error_message IS NOT NULL AND (workspace_root = ?1 OR workspace_root = ?2)
                 ORDER BY timestamp DESC
                 LIMIT 10",
                Some(canon),
                alt,
            )
        } else {
            (
                "SELECT timestamp, tool, error_message
                 FROM tool_telemetry
                 WHERE outcome = 'error' AND error_message IS NOT NULL AND workspace_root = ?1
                 ORDER BY timestamp DESC
                 LIMIT 10",
                Some(canon),
                None,
            )
        }
    } else {
        (
            "SELECT timestamp, tool, error_message
             FROM tool_telemetry
             WHERE outcome = 'error' AND error_message IS NOT NULL
             ORDER BY timestamp DESC
             LIMIT 10",
            None,
            None,
        )
    };

    {
        let mut stmt = conn.prepare(error_sql)?;
        let row_mapper = |row: &rusqlite::Row| {
            let raw_msg: String = row.get(2)?;
            Ok(TelemetryErrorRecord {
                timestamp: row.get(0)?,
                tool: row.get(1)?,
                error_message: sanitize_error_message(&raw_msg),
            })
        };
        match (&ws_param, &alt_ws_param) {
            (Some(ws), Some(alt)) => {
                let rows = stmt.query_map(params![ws, alt], row_mapper)?;
                for err in rows.flatten() {
                    recent_errors.push(err);
                }
            }
            (Some(ws), None) => {
                let rows = stmt.query_map(params![ws], row_mapper)?;
                for err in rows.flatten() {
                    recent_errors.push(err);
                }
            }
            _ => {
                let rows = stmt.query_map([], row_mapper)?;
                for err in rows.flatten() {
                    recent_errors.push(err);
                }
            }
        }
    }

    // Build markdown body
    let mut markdown = String::new();
    markdown.push_str("### Environment\n");
    markdown.push_str(&format!("- **OS:** {}\n", os_info));
    markdown.push_str(&format!("- **Architecture:** {}\n", arch_info));
    markdown.push_str(&format!("- **code-kb Version:** {}\n", code_kb_version));
    markdown.push_str(&format!(
        "- **julie-extract Version:** {}\n",
        julie_extract_version
    ));
    if let Some(ref ws) = active_workspace_name {
        markdown.push_str(&format!("- **Active Workspace:** {}\n", ws));
    }

    let index = workspace_root.and_then(index_facts);
    markdown.push_str("\n### Index\n");
    match &index {
        Some(facts) => {
            let field = |value: &Option<String>| value.clone().unwrap_or_else(|| "unknown".into());
            markdown.push_str(&format!(
                "- **Extractor:** {} (schema {}, level {})\n- **Updated:** {}\n- **Files / Symbols:** {} / {}\n",
                field(&facts.extractor_version),
                field(&facts.schema_version),
                field(&facts.index_level),
                field(&facts.updated_at),
                facts.file_count,
                facts.symbol_count
            ));
        }
        None => markdown.push_str("- No `.code-kb/artifact.db` in the active workspace.\n"),
    }

    let short_body = markdown.clone();
    markdown.push_str("\n### Description\n");
    match description.map(str::trim).filter(|text| !text.is_empty()) {
        Some(text) => markdown.push_str(&format!("{}\n\n", mask_home_paths(text))),
        None => markdown.push_str("<!-- Please describe the bug or unexpected behavior -->\n\n"),
    }

    if !recent_errors.is_empty() {
        markdown.push_str("### Recent Telemetry Errors\n");
        markdown.push_str("| Timestamp | Tool | Error Message |\n");
        markdown.push_str("|---|---|---|\n");
        for err in &recent_errors {
            markdown.push_str(&format!(
                "| {} | `{}` | {} |\n",
                err.timestamp, err.tool, err.error_message
            ));
        }
    }

    let title_str = issue_title
        .map(sanitize_error_message)
        .unwrap_or_else(|| "Bug report".to_string());
    let mut issue_url = build_issue_url(&title_str, &markdown)?;
    if issue_url.as_str().len() > MAX_ISSUE_URL_LEN {
        let short_body = format!(
            "{short_body}\n<!-- The full report was too long for a URL. Paste the output of `code-kb bug-report` here. -->\n"
        );
        issue_url = build_issue_url(&title_str, &short_body)?;
    }

    let log_tail = workspace_root
        .map(|root| log_tail(root, log_lines))
        .unwrap_or_default();
    if !log_tail.is_empty() {
        markdown.push_str(&format!(
            "\n### Recent Log Lines\n```text\n{}\n```\n",
            log_tail.join("\n")
        ));
    }

    Ok(BugReportBundle {
        os_info,
        arch_info,
        code_kb_version,
        julie_extract_version,
        active_workspace_name,
        index,
        recent_errors,
        log_tail,
        markdown_body: markdown,
        github_issue_url: issue_url.to_string(),
    })
}

pub fn format_telemetry_summary(summary: &TelemetrySummary) -> String {
    let mut out = String::new();
    out.push_str("=================================================================\n");
    out.push_str("                    code-kb Telemetry Summary                    \n");
    out.push_str("=================================================================\n");

    if summary.total_calls == 0 {
        out.push_str(&format!(
            "Scope: {} | Window: {}\nNo tool calls recorded for this scope yet.\n",
            summary.scope_description, summary.time_window
        ));
        return out;
    }

    let success_rate = if summary.total_calls > 0 {
        ((summary.ok_calls + summary.empty_calls) as f64 / summary.total_calls as f64) * 100.0
    } else {
        0.0
    };

    out.push_str(&format!(
        "Scope: {} | Window: {} | Total Tool Calls: {} | Success Rate: {:.1}% | Empty Results: {} | Tokens Served: ~{} | Est. Tokens Saved: ~{} (baseline known for {} of {} calls)\n\n",
        summary.scope_description,
        summary.time_window,
        summary.total_calls,
        success_rate,
        summary.empty_calls,
        summary.total_tokens_returned,
        summary.est_tokens_saved,
        summary.saved_known_calls,
        summary.total_calls
    ));

    out.push_str("### Tool Invocations & Performance\n");
    out.push_str(
        "| Tool | Calls | Empty | Latency (p50 / p95 / avg) | Query / Reconcile | Tokens Served | Est. Tokens Saved | Success Rate |\n",
    );
    out.push_str("|---|---:|---:|---:|---:|---:|---:|---:|\n");

    for stat in &summary.tool_stats {
        let rate = if stat.count > 0 {
            ((stat.ok_count + stat.empty_count) as f64 / stat.count as f64) * 100.0
        } else {
            0.0
        };
        let p50_str = stat
            .p50_ms
            .map(|v| format!("{}ms", v))
            .unwrap_or_else(|| "-".to_string());
        let p95_str = stat
            .p95_ms
            .map(|v| format!("{}ms", v))
            .unwrap_or_else(|| "-".to_string());
        let latency_str = format!("{} / {} / {}ms", p50_str, p95_str, stat.avg_duration_ms);

        let query_str = stat
            .avg_query_ms
            .map(|v| format!("{}ms", v))
            .unwrap_or_else(|| "-".to_string());
        let rec_str = stat
            .avg_reconcile_ms
            .map(|v| format!("{}ms", v))
            .unwrap_or_else(|| "-".to_string());
        let phase_str = format!("{} / {}", query_str, rec_str);

        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | ~{} | ~{} ({}/{}) | {:.1}% |\n",
            stat.tool,
            stat.count,
            stat.empty_count,
            latency_str,
            phase_str,
            stat.tokens_returned,
            stat.tokens_saved,
            stat.saved_known_count,
            stat.count,
            rate
        ));
    }

    if !summary.recent_errors.is_empty() {
        out.push_str("\n### Recent Errors\n");
        for err in &summary.recent_errors {
            out.push_str(&format!(
                "- {} [`{}`]: {}\n",
                err.timestamp, err.tool, err.error_message
            ));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_telemetry_dir_isolation() {
        let custom_dir = PathBuf::from("/custom/telemetry/path");
        let target_tmp = PathBuf::from("/workspace/target/tmp");
        let home_dir = PathBuf::from("/home/user");
        let profile_dir = PathBuf::from("C:\\Users\\user");

        // 1. Explicit CODE_KB_TELEMETRY_DIR takes highest precedence
        assert_eq!(
            resolve_telemetry_dir(
                Some(custom_dir.to_string_lossy().to_string()),
                Some(target_tmp.to_string_lossy().to_string()),
                Some(home_dir.to_string_lossy().to_string()),
                Some(profile_dir.to_string_lossy().to_string()),
            ),
            custom_dir
        );

        // 2. CARGO_TARGET_TMPDIR isolates tests when explicit dir is absent
        assert_eq!(
            resolve_telemetry_dir(
                None,
                Some(target_tmp.to_string_lossy().to_string()),
                Some(home_dir.to_string_lossy().to_string()),
                Some(profile_dir.to_string_lossy().to_string()),
            ),
            target_tmp.join("test-telemetry")
        );

        // 3. HOME directory fallback
        assert_eq!(
            resolve_telemetry_dir(
                None,
                None,
                Some(home_dir.to_string_lossy().to_string()),
                Some(profile_dir.to_string_lossy().to_string()),
            ),
            home_dir.join(".code-kb")
        );

        // 4. USERPROFILE directory fallback
        assert_eq!(
            resolve_telemetry_dir(
                None,
                None,
                None,
                Some(profile_dir.to_string_lossy().to_string()),
            ),
            profile_dir.join(".code-kb")
        );

        // 5. Default current working dir
        assert_eq!(
            resolve_telemetry_dir(None, None, None, None),
            PathBuf::from(".code-kb")
        );

        let temp = crate::safe_tempdir();
        set_telemetry_dir_override(Some(temp.path().to_path_buf()));
        let dir = get_global_telemetry_dir();
        assert_eq!(dir, temp.path());

        let conn = open_global_telemetry_db().expect("open_global_telemetry_db should succeed");
        assert!(temp.path().join("telemetry.db").exists());
        drop(conn);
        set_telemetry_dir_override(None);
    }

    #[test]
    fn test_telemetry_disabled_flags() {
        assert!(is_telemetry_disabled_with(Some("1"), None));
        assert!(is_telemetry_disabled_with(Some("true"), None));
        assert!(is_telemetry_disabled_with(Some("TRUE"), None));
        assert!(is_telemetry_disabled_with(None, Some("1")));
        assert!(is_telemetry_disabled_with(None, Some("true")));
        assert!(is_telemetry_disabled_with(None, Some("TRUE")));

        assert!(!is_telemetry_disabled_with(Some("0"), None));
        assert!(!is_telemetry_disabled_with(Some("false"), None));
        assert!(!is_telemetry_disabled_with(None, Some("0")));
        assert!(!is_telemetry_disabled_with(None, None));
    }

    #[test]
    fn test_telemetry_filter_time_windows() {
        assert_eq!(TimeWindow::parse("today"), Some(TimeWindow::Today));
        assert_eq!(TimeWindow::parse("7d"), Some(TimeWindow::Last7Days));
        assert_eq!(TimeWindow::parse("week"), Some(TimeWindow::Last7Days));
        assert_eq!(TimeWindow::parse("30d"), Some(TimeWindow::Last30Days));
        assert_eq!(TimeWindow::parse("month"), Some(TimeWindow::ThisMonth));
        assert_eq!(TimeWindow::parse("this-month"), Some(TimeWindow::ThisMonth));
        assert_eq!(TimeWindow::parse("year"), Some(TimeWindow::LastYear));
        assert_eq!(TimeWindow::parse("last-year"), Some(TimeWindow::LastYear));
        assert_eq!(TimeWindow::parse("all"), Some(TimeWindow::AllTime));
        assert_eq!(TimeWindow::parse("all-time"), Some(TimeWindow::AllTime));
        assert_eq!(TimeWindow::parse("invalid"), None);

        let temp = crate::safe_tempdir();
        let conn = open_telemetry_db_at(temp.path()).expect("open db");

        let ws_root = Path::new("/workspace/test");
        let norm_ws =
            crate::workspace::to_forward_slash(&crate::workspace::normalize_path(ws_root));

        conn.execute(
            "INSERT INTO tool_telemetry (id, timestamp, workspace_root, workspace_name, tool, duration_ms, outcome, est_tokens, est_tokens_saved, code_kb_version)
             VALUES ('id1', datetime('now'), ?1, 'test', 'find_symbol', 10, 'ok', 100, 200, '0.7.0')",
            params![norm_ws],
        ).unwrap();

        conn.execute(
            "INSERT INTO tool_telemetry (id, timestamp, workspace_root, workspace_name, tool, duration_ms, outcome, est_tokens, est_tokens_saved, code_kb_version)
             VALUES ('id2', datetime('now', '-2 days'), ?1, 'test', 'find_symbol', 10, 'ok', 100, 200, '0.7.0')",
            params![norm_ws],
        ).unwrap();

        conn.execute(
            "INSERT INTO tool_telemetry (id, timestamp, workspace_root, workspace_name, tool, duration_ms, outcome, est_tokens, est_tokens_saved, code_kb_version)
             VALUES ('id3', datetime('now', '-15 days'), ?1, 'test', 'find_symbol', 10, 'ok', 100, 200, '0.7.0')",
            params![norm_ws],
        ).unwrap();

        conn.execute(
            "INSERT INTO tool_telemetry (id, timestamp, workspace_root, workspace_name, tool, duration_ms, outcome, est_tokens, est_tokens_saved, code_kb_version)
             VALUES ('id4', datetime('now', '-60 days'), ?1, 'test', 'find_symbol', 10, 'ok', 100, 200, '0.7.0')",
            params![norm_ws],
        ).unwrap();

        let s_today = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::Today,
                workspace_root: None,
                version: None,
            },
        )
        .unwrap();
        assert_eq!(s_today.total_calls, 1);

        let s_7d = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::Last7Days,
                workspace_root: None,
                version: None,
            },
        )
        .unwrap();
        assert_eq!(s_7d.total_calls, 2);

        let s_30d = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::Last30Days,
                workspace_root: None,
                version: None,
            },
        )
        .unwrap();
        assert_eq!(s_30d.total_calls, 3);

        let s_year = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::LastYear,
                workspace_root: None,
                version: None,
            },
        )
        .unwrap();
        assert_eq!(s_year.total_calls, 4);

        let s_all = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::AllTime,
                workspace_root: None,
                version: None,
            },
        )
        .unwrap();
        assert_eq!(s_all.total_calls, 4);
    }

    #[test]
    fn test_telemetry_workspace_scoping() {
        let temp = crate::safe_tempdir();
        let conn = open_telemetry_db_at(temp.path()).expect("open db");

        let ws_a = Path::new("/projects/alpha");
        let ws_b = Path::new("/projects/beta");

        let inv_a = ToolInvocation {
            tool: "find_symbol",
            duration_ms: 15,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(1),
            bytes_returned: 100,
            est_tokens: 25,
            est_tokens_saved: 100,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        record_tool_call_conn(&conn, ws_a, &inv_a);
        record_tool_call_conn(&conn, ws_a, &inv_a);

        let inv_b = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 8,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(1),
            bytes_returned: 200,
            est_tokens: 50,
            est_tokens_saved: 200,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        record_tool_call_conn(&conn, ws_b, &inv_b);

        let filter_a = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(ws_a.to_path_buf()),
            version: None,
        };
        let sum_a = get_telemetry_summary(&conn, &filter_a).unwrap();
        assert_eq!(sum_a.total_calls, 2);
        assert_eq!(sum_a.tool_stats.len(), 1);
        assert_eq!(sum_a.tool_stats[0].tool, "find_symbol");
        assert!(sum_a.scope_description.contains("alpha"));

        let filter_b = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(ws_b.to_path_buf()),
            version: None,
        };
        let sum_b = get_telemetry_summary(&conn, &filter_b).unwrap();
        assert_eq!(sum_b.total_calls, 1);
        assert_eq!(sum_b.tool_stats.len(), 1);
        assert_eq!(sum_b.tool_stats[0].tool, "file_skeleton");
        assert!(sum_b.scope_description.contains("beta"));

        let filter_global = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: None,
            version: None,
        };
        let sum_global = get_telemetry_summary(&conn, &filter_global).unwrap();
        assert_eq!(sum_global.total_calls, 3);
        assert!(
            sum_global
                .scope_description
                .contains("Global (all workspaces)")
        );
    }

    #[test]
    fn test_est_tokens_saved_aggregation() {
        let temp = crate::safe_tempdir();
        let conn = open_telemetry_db_at(temp.path()).expect("open db");
        let ws = Path::new("/projects/token_test");

        let inv1 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 10,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(5),
            bytes_returned: 1000,
            est_tokens: 250,
            est_tokens_saved: 750,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        let inv2 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 20,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(3),
            bytes_returned: 600,
            est_tokens: 150,
            est_tokens_saved: 450,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        let inv3 = ToolInvocation {
            tool: "get_symbol_body",
            duration_ms: 30,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(1),
            bytes_returned: 200,
            est_tokens: 50,
            est_tokens_saved: 500,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };

        record_tool_call_conn(&conn, ws, &inv1);
        record_tool_call_conn(&conn, ws, &inv2);
        record_tool_call_conn(&conn, ws, &inv3);

        let filter = TelemetryFilter::default();
        let summary = get_telemetry_summary(&conn, &filter).unwrap();

        assert_eq!(summary.total_calls, 3);
        assert_eq!(summary.total_tokens_returned, 450);
        assert_eq!(summary.est_tokens_saved, 1700);

        let skel_stat = summary
            .tool_stats
            .iter()
            .find(|s| s.tool == "file_skeleton")
            .unwrap();
        assert_eq!(skel_stat.count, 2);
        assert_eq!(skel_stat.tokens_returned, 400);
        assert_eq!(skel_stat.tokens_saved, 1200);
        assert_eq!(skel_stat.avg_duration_ms, 15);

        let sym_stat = summary
            .tool_stats
            .iter()
            .find(|s| s.tool == "get_symbol_body")
            .unwrap();
        assert_eq!(sym_stat.count, 1);
        assert_eq!(sym_stat.tokens_returned, 50);
        assert_eq!(sym_stat.tokens_saved, 500);
        assert_eq!(sym_stat.avg_duration_ms, 30);
    }

    #[test]
    fn test_saved_baseline_coverage_counts_known_and_unknown_apart() {
        let temp = crate::safe_tempdir();
        let conn = open_telemetry_db_at(temp.path()).expect("open db");
        let ws = Path::new("/projects/coverage_test");

        let known = ToolInvocation {
            tool: "find_references",
            duration_ms: 10,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(4),
            bytes_returned: 400,
            est_tokens: 100,
            est_tokens_saved: 900,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        let unknown = ToolInvocation {
            tool: "find_references",
            duration_ms: 12,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(0),
            bytes_returned: 40,
            est_tokens: 10,
            est_tokens_saved: 0,
            est_tokens_saved_known: false,
            reconcile_ms: None,
            query_ms: None,
        };
        let outline = ToolInvocation {
            tool: "codebase_outline",
            duration_ms: 8,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(7),
            bytes_returned: 200,
            est_tokens: 50,
            est_tokens_saved: 0,
            est_tokens_saved_known: false,
            reconcile_ms: None,
            query_ms: None,
        };

        record_tool_call_conn(&conn, ws, &known);
        record_tool_call_conn(&conn, ws, &unknown);
        record_tool_call_conn(&conn, ws, &outline);

        let summary = get_telemetry_summary(&conn, &TelemetryFilter::default()).unwrap();
        assert_eq!(summary.total_calls, 3);
        assert_eq!(summary.est_tokens_saved, 900);
        assert_eq!(summary.saved_known_calls, 1);

        let refs_stat = summary
            .tool_stats
            .iter()
            .find(|s| s.tool == "find_references")
            .unwrap();
        assert_eq!(refs_stat.count, 2);
        assert_eq!(refs_stat.saved_known_count, 1);

        let outline_stat = summary
            .tool_stats
            .iter()
            .find(|s| s.tool == "codebase_outline")
            .unwrap();
        assert_eq!(outline_stat.saved_known_count, 0);

        let formatted = format_telemetry_summary(&summary);
        assert!(
            formatted.contains("Est. Tokens Saved: ~900 (baseline known for 1 of 3 calls)"),
            "{formatted}"
        );
        assert!(formatted.contains("~900 (1/2)"), "{formatted}");
        assert!(formatted.contains("~0 (0/1)"), "{formatted}");
    }

    #[test]
    fn test_bug_report_bundle_generation() {
        let temp = crate::safe_tempdir();
        let conn = open_telemetry_db_at(temp.path()).unwrap();
        let ws = Path::new("/home/user/src/code-kb");

        let inv_err = ToolInvocation {
            tool: "replace_symbol_body",
            duration_ms: 50,
            outcome: "error",
            error_message: Some("Tree-sitter parse failure on invalid syntax"),
            logical_result_count: None,
            bytes_returned: 0,
            est_tokens: 0,
            est_tokens_saved: 0,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        record_tool_call_conn(&conn, ws, &inv_err);

        let bundle = generate_bug_report(&conn, Some(ws), Some("Parser failure"), None, 0).unwrap();
        assert_eq!(bundle.code_kb_version, env!("CARGO_PKG_VERSION"));
        assert!(!bundle.os_info.is_empty());
        assert!(!bundle.arch_info.is_empty());
        assert!(
            bundle
                .julie_extract_version
                .contains(crate::sync::PINNED_JULIE_VERSION)
        );
        assert_eq!(bundle.active_workspace_name, Some("code-kb".to_string()));
        assert_eq!(bundle.recent_errors.len(), 1);
        assert!(
            bundle.recent_errors[0]
                .error_message
                .contains("Tree-sitter parse failure")
        );

        assert!(bundle.markdown_body.contains("code-kb"));
        assert!(bundle.markdown_body.contains(&bundle.os_info));
        assert!(bundle.markdown_body.contains("Tree-sitter parse failure"));

        assert!(
            bundle
                .github_issue_url
                .starts_with("https://github.com/anortham/code-kb/issues/new?")
        );
        assert!(bundle.github_issue_url.contains("title=Parser"));

        let parsed_url = url::Url::parse(&bundle.github_issue_url).unwrap();
        assert_eq!(parsed_url.host_str(), Some("github.com"));
    }

    #[test]
    fn test_logical_result_count_persists_known_empty_and_nonempty_results() {
        let temp = crate::safe_tempdir();
        let conn = open_telemetry_db_at(temp.path()).expect("open db");
        let root = temp.path();

        for (tool, outcome, logical_result_count) in [
            ("search_symbols", "empty", Some(0)),
            ("search_symbols", "ok", Some(3)),
        ] {
            record_tool_call_conn(
                &conn,
                root,
                &ToolInvocation {
                    tool,
                    duration_ms: 1,
                    outcome,
                    error_message: None,
                    logical_result_count,
                    bytes_returned: 10,
                    est_tokens: 2,
                    est_tokens_saved: 0,
                    est_tokens_saved_known: true,
                    reconcile_ms: None,
                    query_ms: None,
                },
            );
        }

        let counts = conn
            .prepare("SELECT result_count, result_count_known FROM tool_telemetry ORDER BY rowid")
            .unwrap()
            .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(counts, vec![(0, 1), (3, 1)]);

        let summary = get_telemetry_summary(&conn, &TelemetryFilter::default()).unwrap();
        assert_eq!(summary.empty_calls, 1);
        assert_eq!(summary.ok_calls, 1);
        assert_eq!(summary.tool_stats[0].empty_count, 1);
        let formatted = format_telemetry_summary(&summary);
        assert!(formatted.contains("Success Rate: 100.0%"));
        assert!(formatted.contains("Empty Results: 1"));
        assert!(formatted.contains("| `search_symbols` | 2 | 1 |"));
    }

    #[test]
    fn test_telemetry_recording_and_summary() {
        let temp = crate::safe_tempdir();
        let root = temp.path();
        let conn = open_telemetry_db_at(temp.path()).expect("open db");

        let inv1 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 6,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(5),
            bytes_returned: 1200,
            est_tokens: 300,
            est_tokens_saved: 900,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        record_tool_call_conn(&conn, root, &inv1);

        let inv2 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 4,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(3),
            bytes_returned: 800,
            est_tokens: 200,
            est_tokens_saved: 600,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        record_tool_call_conn(&conn, root, &inv2);

        let inv3 = ToolInvocation {
            tool: "replace_symbol_body",
            duration_ms: 12,
            outcome: "error",
            error_message: Some("Syntax error in Rust function"),
            logical_result_count: None,
            bytes_returned: 50,
            est_tokens: 12,
            est_tokens_saved: 0,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        record_tool_call_conn(&conn, root, &inv3);

        let summary = get_telemetry_summary(&conn, &TelemetryFilter::default()).unwrap();
        assert_eq!(summary.total_calls, 3);
        assert_eq!(summary.ok_calls, 2);
        assert_eq!(summary.error_calls, 1);
        assert_eq!(summary.total_tokens_returned, 512);
        assert_eq!(summary.est_tokens_saved, 1500);

        assert_eq!(summary.tool_stats.len(), 2);
        let skel_stat = summary
            .tool_stats
            .iter()
            .find(|s| s.tool == "file_skeleton")
            .unwrap();
        assert_eq!(skel_stat.count, 2);
        assert_eq!(skel_stat.ok_count, 2);
        assert_eq!(skel_stat.avg_duration_ms, 5);

        assert_eq!(summary.recent_errors.len(), 1);
        assert_eq!(summary.recent_errors[0].tool, "replace_symbol_body");
        assert!(
            summary.recent_errors[0]
                .error_message
                .contains("Syntax error")
        );

        let formatted = format_telemetry_summary(&summary);
        assert!(formatted.contains("Total Tool Calls: 3"));
        assert!(formatted.contains("| `file_skeleton` | 2 |"));
        assert!(formatted.contains("Syntax error in Rust function"));
    }

    #[test]
    fn test_old_schema_upgrade() {
        let temp = crate::safe_tempdir();
        let db_path = temp.path().join("telemetry.db");
        let conn = Connection::open(&db_path).unwrap();

        // Create old schema v1 without workspace_root, workspace_name, or est_tokens_saved
        conn.execute_batch(
            "CREATE TABLE tool_telemetry (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                tool TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                outcome TEXT NOT NULL,
                error_message TEXT,
                result_count INTEGER NOT NULL DEFAULT 0,
                bytes_returned INTEGER NOT NULL DEFAULT 0,
                est_tokens INTEGER NOT NULL DEFAULT 0,
                code_kb_version TEXT NOT NULL
            );
            INSERT INTO tool_telemetry VALUES (
                'old1', '2026-09-01 12:00:00', 'find_symbol', 10, 'ok', NULL, 1, 50, 12, '0.6.0'
            );",
        )
        .unwrap();

        // Running init_telemetry_db must migrate columns before creating index
        init_telemetry_db(&conn).expect("schema upgrade should succeed on legacy DB");

        // Verify that workspace_root was added and idx_tool_telemetry_ws_ts was created
        let summary = get_telemetry_summary(&conn, &TelemetryFilter::default()).unwrap();
        assert_eq!(summary.total_calls, 1);
        assert_eq!(summary.ok_calls, 1);
        assert_eq!(
            conn.query_row(
                "SELECT result_count_known FROM tool_telemetry WHERE id = 'old1'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT est_tokens_saved_known FROM tool_telemetry WHERE id = 'old1'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
        assert_eq!(
            get_telemetry_summary(&conn, &TelemetryFilter::default())
                .unwrap()
                .saved_known_calls,
            0
        );

        // Verify we can insert a new record with workspace_root and query via index
        let inv = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 5,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(1),
            bytes_returned: 100,
            est_tokens: 25,
            est_tokens_saved: 75,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        record_tool_call_conn(&conn, Path::new("/workspace/project"), &inv);

        let ws_filter = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(PathBuf::from("/workspace/project")),
            version: None,
        };
        let ws_summary = get_telemetry_summary(&conn, &ws_filter).unwrap();
        assert_eq!(ws_summary.total_calls, 1);
    }

    #[test]
    fn test_bug_report_includes_index_facts_description_and_masked_log_tail() {
        let telemetry_dir = crate::safe_tempdir();
        let conn = open_telemetry_db_at(telemetry_dir.path()).unwrap();
        let workspace = crate::safe_tempdir();
        let root = workspace.path();
        let kb_dir = root.join(".code-kb");
        std::fs::create_dir_all(kb_dir.join("logs")).unwrap();
        let index = Connection::open(kb_dir.join("artifact.db")).unwrap();
        index
            .execute_batch(
                "CREATE TABLE artifact_metadata (key TEXT PRIMARY KEY, value TEXT);
                 INSERT INTO artifact_metadata VALUES ('binary_version', '3.1.1'), ('schema_version', '7'), ('index_level', 'facts');
                 CREATE TABLE files (file_id TEXT); INSERT INTO files VALUES ('a'), ('b');
                 CREATE TABLE symbols (symbol_id TEXT); INSERT INTO symbols VALUES ('s');",
            )
            .unwrap();
        drop(index);
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/default/home".to_string());
        let logs = kb_dir.join("logs");
        std::fs::write(
            logs.join("code-kb.log.2026-09-19"),
            "older one\nolder two\n",
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            logs.join("code-kb.log.2026-09-20"),
            format!("first\nsecond {home}/repo/src/lib.rs\nthird\n"),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(logs.join("notes.txt"), "not a log\n").unwrap();

        let bundle =
            generate_bug_report(&conn, Some(root), Some("Crash"), Some("  It crashed.  "), 2)
                .unwrap();

        let facts = bundle.index.as_ref().unwrap();
        assert_eq!(facts.extractor_version.as_deref(), Some("3.1.1"));
        assert_eq!(facts.schema_version.as_deref(), Some("7"));
        assert_eq!(facts.index_level.as_deref(), Some("facts"));
        assert_eq!(facts.updated_at, None);
        assert_eq!((facts.file_count, facts.symbol_count), (2, 1));
        assert_eq!(bundle.log_tail, vec!["second ~/repo/src/lib.rs", "third"]);
        assert!(
            bundle
                .markdown_body
                .contains("- **Extractor:** 3.1.1 (schema 7, level facts)")
        );
        assert!(
            bundle
                .markdown_body
                .contains("- **Files / Symbols:** 2 / 1")
        );
        assert!(
            bundle
                .markdown_body
                .contains("### Description\nIt crashed.\n")
        );
        assert!(
            bundle
                .markdown_body
                .contains("### Recent Log Lines\n```text\nsecond ~/repo/src/lib.rs\nthird\n```")
        );
        assert!(!bundle.markdown_body.contains(&home));
        assert!(!bundle.github_issue_url.contains("Recent+Log+Lines"));
        assert!(bundle.github_issue_url.contains("It+crashed."));

        let across_rollover = generate_bug_report(
            &conn,
            Some(root),
            None,
            Some(&format!("{home}/x\nline two")),
            4,
        )
        .unwrap();
        assert_eq!(
            across_rollover.log_tail,
            vec!["older two", "first", "second ~/repo/src/lib.rs", "third"]
        );
        assert!(
            across_rollover
                .markdown_body
                .contains("### Description\n~/x\nline two\n")
        );
        assert!(!across_rollover.markdown_body.contains(&home));
        assert!(!across_rollover.github_issue_url.contains("notes"));

        let oversized =
            generate_bug_report(&conn, Some(root), None, Some(&"y".repeat(9000)), 0).unwrap();
        assert!(oversized.markdown_body.contains(&"y".repeat(9000)));
        assert!(oversized.github_issue_url.len() <= MAX_ISSUE_URL_LEN);
        assert!(oversized.github_issue_url.contains("too+long+for+a+URL"));
        assert!(oversized.github_issue_url.contains("Files+%2F+Symbols"));

        let without_index =
            generate_bug_report(&conn, Some(Path::new("/nonexistent/ws")), None, None, 0).unwrap();
        assert!(without_index.index.is_none());
        assert!(without_index.log_tail.is_empty());
        assert!(
            without_index
                .markdown_body
                .contains("No `.code-kb/artifact.db`")
        );
        assert!(without_index.markdown_body.contains("<!-- Please describe"));
    }

    #[test]
    fn test_bug_report_sanitization_and_no_external_exec() {
        let temp = crate::safe_tempdir();
        let conn = open_telemetry_db_at(temp.path()).unwrap();

        let current_home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/default/home".to_string());

        let sensitive_error = format!(
            "{}/workspace/secret-repo/src/lib.rs: syntax error | unexpected token | extra line\nsecond line of error | {}",
            current_home,
            "x".repeat(600), // > 500 chars to test truncation
        );

        let inv = ToolInvocation {
            tool: "replace_symbol_body",
            duration_ms: 10,
            outcome: "error",
            error_message: Some(&sensitive_error),
            logical_result_count: None,
            bytes_returned: 0,
            est_tokens: 0,
            est_tokens_saved: 0,
            est_tokens_saved_known: true,
            reconcile_ms: None,
            query_ms: None,
        };
        // Create a fake malicious julie-extract binary in a .tools directory in workspace
        let ws_temp = crate::safe_tempdir();
        record_tool_call_conn(&conn, ws_temp.path(), &inv);
        let malicious_tools_dir = ws_temp.path().join(".tools");
        std::fs::create_dir_all(&malicious_tools_dir).unwrap();
        let fake_bin = if cfg!(windows) {
            malicious_tools_dir.join("julie-extract.exe")
        } else {
            malicious_tools_dir.join("julie-extract")
        };
        std::fs::write(&fake_bin, b"#!/bin/sh\necho malicious 9.9.9\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let bundle = generate_bug_report(
            &conn,
            Some(ws_temp.path()),
            Some("Issue with | pipes"),
            None,
            0,
        )
        .unwrap();

        // 1. Path sanitization verification
        if !current_home.is_empty() && current_home != "/" {
            assert!(
                !bundle.markdown_body.contains(&current_home),
                "Home directory must be sanitized to ~"
            );
            assert!(
                bundle.markdown_body.contains("~/workspace/secret-repo"),
                "Home directory should be replaced with ~"
            );
            assert!(
                !bundle.github_issue_url.contains(&current_home),
                "GitHub URL must not leak home directory"
            );
        }

        // Direct test of custom home path sanitization
        let custom_sanitized = sanitize_error_message_with_homes(
            "/custom/secret/path/main.rs: err | note\nsecond line",
            &["/custom/secret/path".to_string()],
        );
        assert_eq!(custom_sanitized, "~/main.rs: err \\| note second line");

        // 2. Pipe and newline escaping
        assert!(
            !bundle.markdown_body.contains(" | unexpected token"),
            "Pipe characters must be escaped"
        );
        assert!(
            bundle.markdown_body.contains(r" \| unexpected token"),
            "Pipe characters must be escaped as \\|"
        );
        assert!(
            !bundle.markdown_body.contains("extra line\nsecond line"),
            "Newlines must be sanitized"
        );

        // 3. Length truncation (max 500 chars)
        assert!(
            bundle.recent_errors[0].error_message.chars().count() <= 500,
            "Error message must be truncated to 500 chars"
        );

        // 4. No external binary execution verification
        assert_ne!(
            bundle.julie_extract_version, "malicious 9.9.9",
            "Must not execute .tools/julie-extract from workspace"
        );
        assert_eq!(
            bundle.julie_extract_version,
            crate::sync::PINNED_JULIE_VERSION,
            "Must report pinned version"
        );
    }

    #[test]
    fn test_telemetry_summary_symlink_and_macos_private_var_matching() {
        let telem_dir = crate::safe_tempdir();
        let conn = Connection::open(telem_dir.path().join("telemetry.db")).unwrap();
        init_telemetry_db(&conn).unwrap();

        // Insert a record using macOS /var/folders path
        let raw_var_path = "/var/folders/zz/12345678/T/my_repo";
        conn.execute(
            "INSERT INTO tool_telemetry VALUES (
                't-1', datetime('now'), ?1, 'my_repo', 'lookup_symbol',
                12, 'error', 'Failed to find symbol Foo', 0, 0, 100, 25, 0, 0, '0.9.0',
                NULL, NULL
            )",
            params![raw_var_path],
        )
        .unwrap();

        // Query using canonical macOS /private/var/folders path
        let filter = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(PathBuf::from("/private/var/folders/zz/12345678/T/my_repo")),
            version: None,
        };
        let summary = get_telemetry_summary(&conn, &filter).unwrap();
        assert_eq!(
            summary.total_calls, 1,
            "Must match record across /private/var and /var"
        );
        assert_eq!(summary.recent_errors.len(), 1);
        assert!(
            summary.recent_errors[0]
                .error_message
                .contains("Failed to find symbol Foo")
        );

        // Reverse: insert with /private/var, query with /var
        conn.execute(
            "INSERT INTO tool_telemetry VALUES (
                't-2', datetime('now'), ?1, 'other_repo', 'lookup_symbol',
                12, 'error', 'Reverse matching error', 0, 0, 100, 25, 0, 0, '0.9.0',
                NULL, NULL
            )",
            params!["/private/var/folders/zz/99999999/T/other_repo"],
        )
        .unwrap();

        let filter_rev = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(PathBuf::from("/var/folders/zz/99999999/T/other_repo")),
            version: None,
        };
        let summary_rev = get_telemetry_summary(&conn, &filter_rev).unwrap();
        assert_eq!(summary_rev.total_calls, 1);
        assert_eq!(summary_rev.recent_errors.len(), 1);
        assert!(
            summary_rev.recent_errors[0]
                .error_message
                .contains("Reverse matching error")
        );

        // Bug report must also match
        let bug_report = generate_bug_report(
            &conn,
            Some(Path::new("/private/var/folders/zz/12345678/T/my_repo")),
            Some("test issue"),
            None,
            0,
        )
        .unwrap();
        assert_eq!(bug_report.recent_errors.len(), 1);
        assert!(
            bug_report.recent_errors[0]
                .error_message
                .contains("Failed to find symbol Foo")
        );
    }

    #[test]
    fn test_phase_metrics_and_version_filtering() {
        let temp = crate::safe_tempdir();
        let db_path = temp.path().join("telemetry.db");
        let conn = Connection::open(&db_path).unwrap();

        // 1. Verify schema upgrade adds reconcile_ms and query_ms
        conn.execute_batch(
            "CREATE TABLE tool_telemetry (
                id TEXT PRIMARY KEY,
                timestamp TEXT NOT NULL,
                workspace_root TEXT NOT NULL,
                workspace_name TEXT NOT NULL,
                tool TEXT NOT NULL,
                duration_ms INTEGER NOT NULL,
                outcome TEXT NOT NULL,
                error_message TEXT,
                result_count INTEGER NOT NULL DEFAULT 0,
                result_count_known INTEGER NOT NULL DEFAULT 0,
                bytes_returned INTEGER NOT NULL DEFAULT 0,
                est_tokens INTEGER NOT NULL DEFAULT 0,
                est_tokens_saved INTEGER NOT NULL DEFAULT 0,
                code_kb_version TEXT NOT NULL
            );
            INSERT INTO tool_telemetry VALUES (
                'legacy1', '2026-09-01 12:00:00', '/ws', 'ws', 'lookup_symbol', 10, 'ok', NULL, 1, 1, 50, 12, 0, '1.1.0'
            );",
        )
        .unwrap();

        init_telemetry_db(&conn).expect("schema upgrade should succeed on legacy DB");

        // Verify legacy row has NULL reconcile_ms and query_ms
        let (rec, q): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT reconcile_ms, query_ms FROM tool_telemetry WHERE id = 'legacy1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(rec, None);
        assert_eq!(q, None);

        // 2. Insert records with phase metrics and different versions
        let inv1 = ToolInvocation {
            tool: "lookup_symbol",
            duration_ms: 120,
            outcome: "ok",
            error_message: None,
            logical_result_count: Some(1),
            bytes_returned: 100,
            est_tokens: 25,
            est_tokens_saved: 50,
            est_tokens_saved_known: true,
            reconcile_ms: Some(100),
            query_ms: Some(20),
        };
        // Record with custom version manually for test partitioning
        conn.execute(
            "INSERT INTO tool_telemetry (
                id, timestamp, workspace_root, workspace_name, tool,
                duration_ms, outcome, error_message, result_count, result_count_known,
                bytes_returned, est_tokens, est_tokens_saved, code_kb_version,
                reconcile_ms, query_ms
            ) VALUES ('c1', datetime('now'), '/ws', 'ws', ?1, ?2, ?3, NULL, 1, 1, ?4, ?5, ?6, '1.1.2', ?7, ?8)",
            params![
                inv1.tool,
                inv1.duration_ms as i64,
                inv1.outcome,
                inv1.bytes_returned as i64,
                inv1.est_tokens as i64,
                inv1.est_tokens_saved as i64,
                inv1.reconcile_ms.map(|v| v as i64),
                inv1.query_ms.map(|v| v as i64),
            ],
        ).unwrap();

        for (id, dur, q_ms) in [("c2", 5, 5), ("c3", 15, 15), ("c4", 25, 25)] {
            conn.execute(
                "INSERT INTO tool_telemetry (
                    id, timestamp, workspace_root, workspace_name, tool,
                    duration_ms, outcome, error_message, result_count, result_count_known,
                    bytes_returned, est_tokens, est_tokens_saved, code_kb_version,
                    reconcile_ms, query_ms
                ) VALUES (?1, datetime('now'), '/ws', 'ws', 'lookup_symbol', ?2, 'ok', NULL, 1, 1, 100, 25, 50, '1.1.3', 0, ?3)",
                params![id, dur as i64, q_ms as i64],
            ).unwrap();
        }

        // 3. Test version filtering: "1.1.3"
        let filter_v113 = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: None,
            version: Some("1.1.3".to_string()),
        };
        let summary_v113 = get_telemetry_summary(&conn, &filter_v113).unwrap();
        assert_eq!(summary_v113.total_calls, 3);
        assert_eq!(summary_v113.tool_stats.len(), 1);
        let stat = &summary_v113.tool_stats[0];
        assert_eq!(stat.tool, "lookup_symbol");
        assert_eq!(stat.count, 3);
        assert_eq!(stat.p50_ms, Some(15));
        assert_eq!(stat.p95_ms, Some(25));
        assert_eq!(stat.avg_reconcile_ms, Some(0));
        assert_eq!(stat.avg_query_ms, Some(15));

        // 4. Test version filtering: None (all versions including legacy)
        let filter_all = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: None,
            version: None,
        };
        let summary_all = get_telemetry_summary(&conn, &filter_all).unwrap();
        assert_eq!(summary_all.total_calls, 5); // legacy1 + c1 + c2 + c3 + c4

        // 5. Test record_tool_call_conn persists phase fields
        record_tool_call_conn(&conn, Path::new("/ws"), &inv1);
        let (last_rec, last_q): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT reconcile_ms, query_ms FROM tool_telemetry ORDER BY rowid DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(last_rec, Some(100));
        assert_eq!(last_q, Some(20));
    }
}
