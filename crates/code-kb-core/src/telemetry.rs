use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::SystemTime;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::queries::QueryError;
use crate::workspace::{normalize_path, paths_equal, to_forward_slash};

#[derive(Debug, Clone)]
pub struct ToolInvocation<'a> {
    pub tool: &'a str,
    pub duration_ms: u64,
    pub outcome: &'a str, // "ok", "empty", "error"
    pub error_message: Option<&'a str>,
    pub result_count: usize,
    pub bytes_returned: usize,
    pub est_tokens: usize,
    pub est_tokens_saved: usize,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySummary {
    pub total_calls: usize,
    pub ok_calls: usize,
    pub empty_calls: usize,
    pub error_calls: usize,
    pub total_tokens_returned: usize,
    pub est_tokens_saved: usize,
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
    pub error_count: usize,
    pub avg_duration_ms: u64,
    pub tokens_returned: usize,
    pub tokens_saved: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryErrorRecord {
    pub timestamp: String,
    pub tool: String,
    pub error_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BugReportBundle {
    pub os_info: String,
    pub arch_info: String,
    pub code_kb_version: String,
    pub julie_extract_version: String,
    pub active_workspace_name: Option<String>,
    pub recent_errors: Vec<TelemetryErrorRecord>,
    pub markdown_body: String,
    pub github_issue_url: String,
}

static TELEMETRY_DIR_OVERRIDE: RwLock<Option<PathBuf>> = RwLock::new(None);

pub fn resolve_telemetry_dir(
    env_dir: Option<String>,
    home: Option<String>,
    userprofile: Option<String>,
) -> PathBuf {
    if let Some(dir) = env_dir
        && !dir.trim().is_empty()
    {
        return PathBuf::from(dir);
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

pub fn get_global_telemetry_dir() -> PathBuf {
    if let Ok(guard) = TELEMETRY_DIR_OVERRIDE.read()
        && let Some(ref path) = *guard
    {
        return path.clone();
    }
    resolve_telemetry_dir(
        std::env::var("CODE_KB_TELEMETRY_DIR").ok(),
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
             bytes_returned INTEGER NOT NULL DEFAULT 0,
             est_tokens INTEGER NOT NULL DEFAULT 0,
             est_tokens_saved INTEGER NOT NULL DEFAULT 0,
             code_kb_version TEXT NOT NULL
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
            duration_ms, outcome, error_message, result_count,
            bytes_returned, est_tokens, est_tokens_saved, code_kb_version
        ) VALUES (?1, datetime('now'), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            id,
            norm_ws,
            ws_name,
            invocation.tool,
            invocation.duration_ms as i64,
            invocation.outcome,
            invocation.error_message,
            invocation.result_count as i64,
            invocation.bytes_returned as i64,
            invocation.est_tokens as i64,
            invocation.est_tokens_saved as i64,
            version
        ],
    );
}

/// Record a tool call to the global telemetry database.
/// Best-effort and non-panicking.
pub fn record_tool_call(workspace_root: &Path, invocation: &ToolInvocation) {
    if let Ok(conn) = open_global_telemetry_db() {
        record_tool_call_conn(&conn, workspace_root, invocation);
    }
}

pub fn get_telemetry_summary(
    conn: &Connection,
    filter: &TelemetryFilter,
) -> Result<TelemetrySummary, QueryError> {
    let (where_clause, ws_param, alt_ws_param) = match (
        &filter.time_window.to_sqlite_condition(),
        &filter.workspace_root,
    ) {
        (Some(time_cond), Some(ws)) => {
            let (canon, alt) = workspace_root_match_candidates(ws);
            if alt.is_some() {
                (
                    format!(
                        "WHERE {} AND (workspace_root = ?1 OR workspace_root = ?2)",
                        time_cond
                    ),
                    Some(canon),
                    alt,
                )
            } else {
                (
                    format!("WHERE {} AND workspace_root = ?1", time_cond),
                    Some(canon),
                    None,
                )
            }
        }
        (Some(time_cond), None) => (format!("WHERE {}", time_cond), None, None),
        (None, Some(ws)) => {
            let (canon, alt) = workspace_root_match_candidates(ws);
            if alt.is_some() {
                (
                    "WHERE (workspace_root = ?1 OR workspace_root = ?2)".to_string(),
                    Some(canon),
                    alt,
                )
            } else {
                ("WHERE workspace_root = ?1".to_string(), Some(canon), None)
            }
        }
        (None, None) => ("".to_string(), None, None),
    };

    let scope_description = match &filter.workspace_root {
        Some(ws) => format!("Workspace: {}", to_forward_slash(&normalize_path(ws))),
        None => "Global (all workspaces)".to_string(),
    };

    // 1. Overall aggregations
    let totals_sql = format!(
        "SELECT COUNT(*),
                SUM(CASE WHEN outcome = 'ok' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'empty' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END),
                SUM(est_tokens),
                SUM(est_tokens_saved)
         FROM tool_telemetry
         {}",
        where_clause
    );

    let (total_calls, ok_calls, empty_calls, error_calls, total_tokens_returned, est_tokens_saved) = {
        let mut stmt = conn.prepare(&totals_sql)?;
        let row_mapper = |row: &rusqlite::Row| {
            let total: i64 = row.get(0)?;
            let ok: Option<i64> = row.get(1)?;
            let empty: Option<i64> = row.get(2)?;
            let error: Option<i64> = row.get(3)?;
            let tokens: Option<i64> = row.get(4)?;
            let tokens_saved: Option<i64> = row.get(5)?;
            Ok((
                total as usize,
                ok.unwrap_or(0) as usize,
                empty.unwrap_or(0) as usize,
                error.unwrap_or(0) as usize,
                tokens.unwrap_or(0) as usize,
                tokens_saved.unwrap_or(0) as usize,
            ))
        };
        match (&ws_param, &alt_ws_param) {
            (Some(ws), Some(alt)) => stmt.query_row(params![ws, alt], row_mapper)?,
            (Some(ws), None) => stmt.query_row(params![ws], row_mapper)?,
            _ => stmt.query_row([], row_mapper)?,
        }
    };

    // 2. Per-tool statistics
    let tool_stats_sql = format!(
        "SELECT tool,
                COUNT(*),
                SUM(CASE WHEN outcome = 'ok' THEN 1 ELSE 0 END),
                SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END),
                ROUND(AVG(duration_ms)),
                SUM(est_tokens),
                SUM(est_tokens_saved)
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
            let error_count: Option<i64> = row.get(3)?;
            let avg_duration: Option<f64> = row.get(4)?;
            let tokens: Option<i64> = row.get(5)?;
            let tokens_saved: Option<i64> = row.get(6)?;

            Ok(ToolStat {
                tool,
                count: count as usize,
                ok_count: ok_count.unwrap_or(0) as usize,
                error_count: error_count.unwrap_or(0) as usize,
                avg_duration_ms: avg_duration.unwrap_or(0.0).round() as u64,
                tokens_returned: tokens.unwrap_or(0) as usize,
                tokens_saved: tokens_saved.unwrap_or(0) as usize,
            })
        };

        match (&ws_param, &alt_ws_param) {
            (Some(ws), Some(alt)) => {
                let rows = stmt.query_map(params![ws, alt], row_mapper)?;
                for stat in rows.flatten() {
                    tool_stats.push(stat);
                }
            }
            (Some(ws), None) => {
                let rows = stmt.query_map(params![ws], row_mapper)?;
                for stat in rows.flatten() {
                    tool_stats.push(stat);
                }
            }
            _ => {
                let rows = stmt.query_map([], row_mapper)?;
                for stat in rows.flatten() {
                    tool_stats.push(stat);
                }
            }
        }
    }

    // 3. Recent errors
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

    Ok(TelemetrySummary {
        total_calls,
        ok_calls,
        empty_calls,
        error_calls,
        total_tokens_returned,
        est_tokens_saved,
        time_window: filter.time_window,
        scope_description,
        tool_stats,
        recent_errors,
    })
}

fn is_same_file(p1: &Path, p2: &Path) -> bool {
    if paths_equal(p1, p2) {
        return true;
    }
    match (dunce::canonicalize(p1), dunce::canonicalize(p2)) {
        (Ok(c1), Ok(c2)) => paths_equal(&c1, &c2),
        _ => false,
    }
}

fn sanitize_error_message(msg: &str) -> String {
    let mut home_candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME")
        && !home.trim().is_empty()
        && home != "/"
    {
        let simplified = dunce::simplified(Path::new(&home))
            .to_string_lossy()
            .to_string();
        if simplified != home {
            home_candidates.push(simplified);
        }
        home_candidates.push(home);
    }
    if let Ok(profile) = std::env::var("USERPROFILE")
        && !profile.trim().is_empty()
        && profile != "/"
    {
        let simplified = dunce::simplified(Path::new(&profile))
            .to_string_lossy()
            .to_string();
        if simplified != profile {
            home_candidates.push(simplified);
        }
        home_candidates.push(profile);
    }

    sanitize_error_message_with_homes(msg, &home_candidates)
}

fn sanitize_error_message_with_homes(msg: &str, home_candidates: &[String]) -> String {
    let mut sanitized = msg.to_string();

    for home in home_candidates {
        let norm_home = to_forward_slash(&normalize_path(Path::new(home)));
        sanitized = sanitized.replace(home.as_str(), "~");
        if norm_home != *home {
            sanitized = sanitized.replace(&norm_home, "~");
        }
        let backslash_home = home.replace('/', "\\");
        if backslash_home != *home {
            sanitized = sanitized.replace(&backslash_home, "~");
        }
    }

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

pub fn generate_bug_report(
    conn: &Connection,
    workspace_root: Option<&Path>,
    issue_title: Option<&str>,
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
    markdown
        .push_str("\n### Description\n<!-- Please describe the bug or unexpected behavior -->\n\n");

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

    // Generate GitHub issue URL
    let title_str = issue_title
        .map(sanitize_error_message)
        .unwrap_or_else(|| "Bug report".to_string());
    let mut issue_url = url::Url::parse("https://github.com/anortham/code-kb/issues/new")
        .map_err(|e| QueryError::Sqlite(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
    issue_url
        .query_pairs_mut()
        .append_pair("title", &title_str)
        .append_pair("body", &markdown);

    Ok(BugReportBundle {
        os_info,
        arch_info,
        code_kb_version,
        julie_extract_version,
        active_workspace_name,
        recent_errors,
        markdown_body: markdown,
        github_issue_url: issue_url.to_string(),
    })
}

/// Migrates legacy telemetry from `<workspace_root>/.code-kb/telemetry.db` into the global telemetry database.
/// Cleans up the legacy file upon successful migration.
pub fn migrate_legacy_workspace_telemetry(
    global_conn: &Connection,
    workspace_root: &Path,
) -> Result<usize, QueryError> {
    let legacy_db_path = workspace_root.join(".code-kb").join("telemetry.db");
    if !legacy_db_path.exists() {
        return Ok(0);
    }

    // Skip migration if legacy database and destination database identify the same physical file
    let global_db_path = get_global_telemetry_dir().join("telemetry.db");
    if is_same_file(&legacy_db_path, &global_db_path) {
        return Ok(0);
    }
    if let Ok(dest_db_str) =
        global_conn.query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
        && !dest_db_str.is_empty()
        && is_same_file(&legacy_db_path, Path::new(&dest_db_str))
    {
        return Ok(0);
    }

    let norm_ws = to_forward_slash(&normalize_path(workspace_root));
    let ws_name = Path::new(&norm_ws)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".to_string());

    let mut migrated_count = 0;
    let rows_to_insert = {
        let legacy_conn = Connection::open_with_flags(
            &legacy_db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        let has_table = {
            let mut check_stmt = legacy_conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='tool_telemetry'",
            )?;
            check_stmt.exists([])?
        };
        if !has_table {
            return Ok(0);
        }

        let mut stmt = legacy_conn.prepare(
            "SELECT id, timestamp, tool, duration_ms, outcome, error_message,
                    result_count, bytes_returned, est_tokens, code_kb_version
             FROM tool_telemetry",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;

        let mut collected = Vec::new();
        for r in rows {
            collected.push(r?);
        }
        collected
    };

    let tx = global_conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT OR IGNORE INTO tool_telemetry (
                id, timestamp, workspace_root, workspace_name, tool,
                duration_ms, outcome, error_message, result_count,
                bytes_returned, est_tokens, est_tokens_saved, code_kb_version
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
        )?;

        for (
            id,
            ts,
            tool,
            duration_ms,
            outcome,
            error_msg,
            result_count,
            bytes_returned,
            est_tokens,
            code_kb_version,
        ) in rows_to_insert
        {
            let inserted = stmt.execute(params![
                id,
                ts,
                norm_ws,
                ws_name,
                tool,
                duration_ms,
                outcome,
                error_msg,
                result_count,
                bytes_returned,
                est_tokens,
                code_kb_version,
            ])?;
            migrated_count += inserted;
        }
    }
    tx.commit()?;

    // Clean up legacy database and any temporary WAL/SHM files only after commit succeeds
    let _ = std::fs::remove_file(&legacy_db_path);
    let _ = std::fs::remove_file(legacy_db_path.with_file_name("telemetry.db-wal"));
    let _ = std::fs::remove_file(legacy_db_path.with_file_name("telemetry.db-shm"));

    Ok(migrated_count)
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
        (summary.ok_calls as f64 / summary.total_calls as f64) * 100.0
    } else {
        0.0
    };

    out.push_str(&format!(
        "Scope: {} | Window: {} | Total Tool Calls: {} | Success Rate: {:.1}% | Tokens Served: ~{} | Tokens Saved: ~{}\n\n",
        summary.scope_description, summary.time_window, summary.total_calls, success_rate, summary.total_tokens_returned, summary.est_tokens_saved
    ));

    out.push_str("### Tool Invocations & Performance\n");
    out.push_str("| Tool | Calls | Avg Latency | Tokens Served | Tokens Saved | Success Rate |\n");
    out.push_str("|---|---:|---:|---:|---:|---:|\n");

    for stat in &summary.tool_stats {
        let rate = if stat.count > 0 {
            (stat.ok_count as f64 / stat.count as f64) * 100.0
        } else {
            0.0
        };
        out.push_str(&format!(
            "| `{}` | {} | {} ms | ~{} | ~{} | {:.1}% |\n",
            stat.tool,
            stat.count,
            stat.avg_duration_ms,
            stat.tokens_returned,
            stat.tokens_saved,
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
        let home_dir = PathBuf::from("/home/user");
        let profile_dir = PathBuf::from("C:\\Users\\user");

        assert_eq!(
            resolve_telemetry_dir(
                Some(custom_dir.to_string_lossy().to_string()),
                Some(home_dir.to_string_lossy().to_string()),
                Some(profile_dir.to_string_lossy().to_string()),
            ),
            custom_dir
        );

        assert_eq!(
            resolve_telemetry_dir(
                None,
                Some(home_dir.to_string_lossy().to_string()),
                Some(profile_dir.to_string_lossy().to_string()),
            ),
            home_dir.join(".code-kb")
        );

        assert_eq!(
            resolve_telemetry_dir(None, None, Some(profile_dir.to_string_lossy().to_string()),),
            profile_dir.join(".code-kb")
        );

        assert_eq!(
            resolve_telemetry_dir(None, None, None),
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
            },
        )
        .unwrap();
        assert_eq!(s_today.total_calls, 1);

        let s_7d = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::Last7Days,
                workspace_root: None,
            },
        )
        .unwrap();
        assert_eq!(s_7d.total_calls, 2);

        let s_30d = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::Last30Days,
                workspace_root: None,
            },
        )
        .unwrap();
        assert_eq!(s_30d.total_calls, 3);

        let s_year = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::LastYear,
                workspace_root: None,
            },
        )
        .unwrap();
        assert_eq!(s_year.total_calls, 4);

        let s_all = get_telemetry_summary(
            &conn,
            &TelemetryFilter {
                time_window: TimeWindow::AllTime,
                workspace_root: None,
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
            result_count: 1,
            bytes_returned: 100,
            est_tokens: 25,
            est_tokens_saved: 100,
        };
        record_tool_call_conn(&conn, ws_a, &inv_a);
        record_tool_call_conn(&conn, ws_a, &inv_a);

        let inv_b = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 8,
            outcome: "ok",
            error_message: None,
            result_count: 1,
            bytes_returned: 200,
            est_tokens: 50,
            est_tokens_saved: 200,
        };
        record_tool_call_conn(&conn, ws_b, &inv_b);

        let filter_a = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(ws_a.to_path_buf()),
        };
        let sum_a = get_telemetry_summary(&conn, &filter_a).unwrap();
        assert_eq!(sum_a.total_calls, 2);
        assert_eq!(sum_a.tool_stats.len(), 1);
        assert_eq!(sum_a.tool_stats[0].tool, "find_symbol");
        assert!(sum_a.scope_description.contains("alpha"));

        let filter_b = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(ws_b.to_path_buf()),
        };
        let sum_b = get_telemetry_summary(&conn, &filter_b).unwrap();
        assert_eq!(sum_b.total_calls, 1);
        assert_eq!(sum_b.tool_stats.len(), 1);
        assert_eq!(sum_b.tool_stats[0].tool, "file_skeleton");
        assert!(sum_b.scope_description.contains("beta"));

        let filter_global = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: None,
        };
        let sum_global = get_telemetry_summary(&conn, &filter_global).unwrap();
        assert_eq!(sum_global.total_calls, 3);
        assert_eq!(sum_global.scope_description, "Global (all workspaces)");
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
            result_count: 5,
            bytes_returned: 1000,
            est_tokens: 250,
            est_tokens_saved: 750,
        };
        let inv2 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 20,
            outcome: "ok",
            error_message: None,
            result_count: 3,
            bytes_returned: 600,
            est_tokens: 150,
            est_tokens_saved: 450,
        };
        let inv3 = ToolInvocation {
            tool: "get_symbol_body",
            duration_ms: 30,
            outcome: "ok",
            error_message: None,
            result_count: 1,
            bytes_returned: 200,
            est_tokens: 50,
            est_tokens_saved: 500,
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
    fn test_legacy_migration() {
        let ws_temp = crate::safe_tempdir();
        let ws_root = ws_temp.path();
        let legacy_dir = ws_root.join(".code-kb");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_db_path = legacy_dir.join("telemetry.db");

        let legacy_conn = Connection::open(&legacy_db_path).unwrap();
        legacy_conn.execute_batch(
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
            INSERT INTO tool_telemetry VALUES ('leg1', '2026-09-01 12:00:00', 'find_symbol', 10, 'ok', NULL, 1, 50, 12, '0.6.0');
            INSERT INTO tool_telemetry VALUES ('leg2', '2026-09-01 12:01:00', 'replace_symbol_body', 20, 'error', 'disk error', 0, 0, 0, '0.6.0');"
        ).unwrap();
        drop(legacy_conn);

        assert!(legacy_db_path.exists());

        let global_temp = crate::safe_tempdir();
        let global_conn = open_telemetry_db_at(global_temp.path()).unwrap();

        let migrated = migrate_legacy_workspace_telemetry(&global_conn, ws_root).unwrap();
        assert_eq!(migrated, 2);

        assert!(!legacy_db_path.exists());

        let summary = get_telemetry_summary(&global_conn, &TelemetryFilter::default()).unwrap();
        assert_eq!(summary.total_calls, 2);
        assert_eq!(summary.ok_calls, 1);
        assert_eq!(summary.error_calls, 1);

        let migrated_second = migrate_legacy_workspace_telemetry(&global_conn, ws_root).unwrap();
        assert_eq!(migrated_second, 0);
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
            result_count: 0,
            bytes_returned: 0,
            est_tokens: 0,
            est_tokens_saved: 0,
        };
        record_tool_call_conn(&conn, ws, &inv_err);

        let bundle = generate_bug_report(&conn, Some(ws), Some("Parser failure")).unwrap();
        assert_eq!(bundle.code_kb_version, env!("CARGO_PKG_VERSION"));
        assert!(!bundle.os_info.is_empty());
        assert!(!bundle.arch_info.is_empty());
        assert!(bundle.julie_extract_version.contains(crate::sync::PINNED_JULIE_VERSION));
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
    fn test_telemetry_recording_and_summary() {
        let temp = crate::safe_tempdir();
        let root = temp.path();
        let conn = open_telemetry_db_at(temp.path()).expect("open db");

        let inv1 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 6,
            outcome: "ok",
            error_message: None,
            result_count: 5,
            bytes_returned: 1200,
            est_tokens: 300,
            est_tokens_saved: 900,
        };
        record_tool_call_conn(&conn, root, &inv1);

        let inv2 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 4,
            outcome: "ok",
            error_message: None,
            result_count: 3,
            bytes_returned: 800,
            est_tokens: 200,
            est_tokens_saved: 600,
        };
        record_tool_call_conn(&conn, root, &inv2);

        let inv3 = ToolInvocation {
            tool: "replace_symbol_body",
            duration_ms: 12,
            outcome: "error",
            error_message: Some("Syntax error in Rust function"),
            result_count: 0,
            bytes_returned: 50,
            est_tokens: 12,
            est_tokens_saved: 0,
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

        // Verify we can insert a new record with workspace_root and query via index
        let inv = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 5,
            outcome: "ok",
            error_message: None,
            result_count: 1,
            bytes_returned: 100,
            est_tokens: 25,
            est_tokens_saved: 75,
        };
        record_tool_call_conn(&conn, Path::new("/workspace/project"), &inv);

        let ws_filter = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(PathBuf::from("/workspace/project")),
        };
        let ws_summary = get_telemetry_summary(&conn, &ws_filter).unwrap();
        assert_eq!(ws_summary.total_calls, 1);
    }

    #[test]
    fn test_legacy_migration_same_path_no_delete() {
        let ws_temp = crate::safe_tempdir();
        let ws_root = ws_temp.path();
        let legacy_dir = ws_root.join(".code-kb");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_db_path = legacy_dir.join("telemetry.db");

        let legacy_conn = Connection::open(&legacy_db_path).unwrap();
        init_telemetry_db(&legacy_conn).unwrap();
        legacy_conn
            .execute(
                "INSERT INTO tool_telemetry VALUES (
                    'same1', '2026-09-01 12:00:00', '/ws', 'repo', 'find_symbol',
                    10, 'ok', NULL, 1, 50, 12, 0, '0.7.0'
                )",
                [],
            )
            .unwrap();

        // Test 1: destination connection opened on the exact same database file
        let migrated = migrate_legacy_workspace_telemetry(&legacy_conn, ws_root).unwrap();
        assert_eq!(
            migrated, 0,
            "must skip migration when destination is the same database"
        );
        assert!(legacy_db_path.exists(), "must not unlink the database file");

        // Test 2: global telemetry dir override points to the same directory
        set_telemetry_dir_override(Some(legacy_dir.clone()));
        let migrated_ovr = migrate_legacy_workspace_telemetry(&legacy_conn, ws_root).unwrap();
        assert_eq!(
            migrated_ovr, 0,
            "must skip migration when global dir matches legacy dir"
        );
        assert!(
            legacy_db_path.exists(),
            "must not unlink when global dir matches legacy dir"
        );
        set_telemetry_dir_override(None);
    }

    #[test]
    fn test_legacy_migration_rollback_on_failure() {
        let ws_temp = crate::safe_tempdir();
        let ws_root = ws_temp.path();
        let legacy_dir = ws_root.join(".code-kb");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        let legacy_db_path = legacy_dir.join("telemetry.db");

        let legacy_conn = Connection::open(&legacy_db_path).unwrap();
        legacy_conn
            .execute_batch(
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
                    'leg1', '2026-09-01 12:00:00', 'find_symbol', 10, 'ok', NULL, 1, 50, 12, '0.6.0'
                );",
            )
            .unwrap();
        drop(legacy_conn);

        let global_temp = crate::safe_tempdir();
        let global_conn = open_telemetry_db_at(global_temp.path()).unwrap();

        // Install a trigger that forces insertion to fail
        global_conn
            .execute(
                "CREATE TRIGGER fail_telemetry_insert BEFORE INSERT ON tool_telemetry
                 BEGIN
                     SELECT RAISE(ABORT, 'simulated disk write error');
                 END;",
                [],
            )
            .unwrap();

        let res = migrate_legacy_workspace_telemetry(&global_conn, ws_root);
        assert!(
            res.is_err(),
            "migration must return error when insert fails"
        );
        assert!(
            legacy_db_path.exists(),
            "legacy database must NOT be deleted after failed migration"
        );

        // Drop trigger and verify migration now succeeds
        global_conn
            .execute("DROP TRIGGER fail_telemetry_insert", [])
            .unwrap();
        let res2 = migrate_legacy_workspace_telemetry(&global_conn, ws_root);
        assert_eq!(res2.unwrap(), 1);
        assert!(
            !legacy_db_path.exists(),
            "legacy database should be deleted only after successful commit"
        );
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
            result_count: 0,
            bytes_returned: 0,
            est_tokens: 0,
            est_tokens_saved: 0,
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

        let bundle =
            generate_bug_report(&conn, Some(ws_temp.path()), Some("Issue with | pipes")).unwrap();

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
                12, 'error', 'Failed to find symbol Foo', 0, 100, 25, 0, '0.9.0'
            )",
            params![raw_var_path],
        )
        .unwrap();

        // Query using canonical macOS /private/var/folders path
        let filter = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(PathBuf::from("/private/var/folders/zz/12345678/T/my_repo")),
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
                12, 'error', 'Reverse matching error', 0, 100, 25, 0, '0.9.0'
            )",
            params!["/private/var/folders/zz/99999999/T/other_repo"],
        )
        .unwrap();

        let filter_rev = TelemetryFilter {
            time_window: TimeWindow::AllTime,
            workspace_root: Some(PathBuf::from("/var/folders/zz/99999999/T/other_repo")),
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
        )
        .unwrap();
        assert_eq!(bug_report.recent_errors.len(), 1);
        assert!(
            bug_report.recent_errors[0]
                .error_message
                .contains("Failed to find symbol Foo")
        );
    }
}
