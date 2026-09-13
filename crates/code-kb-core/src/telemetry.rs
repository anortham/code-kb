use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::queries::QueryError;

#[derive(Debug, Clone)]
pub struct ToolInvocation<'a> {
    pub tool: &'a str,
    pub duration_ms: u64,
    pub outcome: &'a str, // "ok", "empty", "error"
    pub error_message: Option<&'a str>,
    pub result_count: usize,
    pub bytes_returned: usize,
    pub est_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySummary {
    pub total_calls: usize,
    pub ok_calls: usize,
    pub empty_calls: usize,
    pub error_calls: usize,
    pub total_tokens_returned: usize,
    pub tool_stats: Vec<ToolStat>,
    pub recent_errors: Vec<TelemetryErrorRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStat {
    pub tool: String,
    pub count: usize,
    pub ok_count: usize,
    pub error_count: usize,
    pub median_duration_ms: u64,
    pub tokens_returned: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryErrorRecord {
    pub timestamp: String,
    pub tool: String,
    pub error_message: String,
}

/// Open or initialize the telemetry database in WAL mode.
fn open_telemetry_db(workspace_root: &Path) -> Result<Connection, QueryError> {
    let db_dir = workspace_root.join(".code-kb");
    if !db_dir.exists() {
        let _ = std::fs::create_dir_all(&db_dir);
    }
    let db_path = db_dir.join("telemetry.db");
    let conn = Connection::open(&db_path)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 2000;
         CREATE TABLE IF NOT EXISTS tool_telemetry (
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
         CREATE INDEX IF NOT EXISTS idx_tool_telemetry_tool ON tool_telemetry(tool, timestamp DESC);
         CREATE INDEX IF NOT EXISTS idx_tool_telemetry_ts ON tool_telemetry(timestamp DESC);",
    )?;
    Ok(conn)
}

/// Record a tool call to telemetry.db.
/// This function is best-effort and will never panic or return an error to callers.
pub fn record_tool_call(workspace_root: &Path, invocation: &ToolInvocation) {
    if let Ok(conn) = open_telemetry_db(workspace_root) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let ts = format!("{:?}", SystemTime::now());
        let id_source = format!("{}:{}:{}", invocation.tool, ts, now.as_nanos());
        let id = blake3::hash(id_source.as_bytes()).to_hex().to_string();
        let version = env!("CARGO_PKG_VERSION");

        let _ = conn.execute(
            "INSERT INTO tool_telemetry (
                id, timestamp, tool, duration_ms, outcome, error_message,
                result_count, bytes_returned, est_tokens, code_kb_version
            ) VALUES (?1, datetime('now'), ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                invocation.tool,
                invocation.duration_ms as i64,
                invocation.outcome,
                invocation.error_message,
                invocation.result_count as i64,
                invocation.bytes_returned as i64,
                invocation.est_tokens as i64,
                version
            ],
        );
    }
}

/// Query telemetry statistics and error records from telemetry.db.
pub fn get_telemetry_summary(workspace_root: &Path) -> Result<TelemetrySummary, QueryError> {
    let db_path = workspace_root.join(".code-kb").join("telemetry.db");
    if !db_path.exists() {
        return Ok(TelemetrySummary {
            total_calls: 0,
            ok_calls: 0,
            empty_calls: 0,
            error_calls: 0,
            total_tokens_returned: 0,
            tool_stats: Vec::new(),
            recent_errors: Vec::new(),
        });
    }

    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let mut stmt = conn.prepare(
        "SELECT tool, duration_ms, outcome, est_tokens FROM tool_telemetry ORDER BY tool, duration_ms ASC",
    )?;

    let mut total_calls = 0;
    let mut ok_calls = 0;
    let mut empty_calls = 0;
    let mut error_calls = 0;
    let mut total_tokens = 0;

    struct ToolAccumulator {
        durations: Vec<u64>,
        ok_count: usize,
        error_count: usize,
        tokens: usize,
    }

    let mut tool_map: HashMap<String, ToolAccumulator> = HashMap::new();

    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)? as u64,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)? as usize,
        ))
    })?;

    for r in rows {
        let (tool, duration, outcome, tokens) = r?;
        total_calls += 1;
        total_tokens += tokens;

        let is_ok = outcome == "ok";
        let is_error = outcome == "error";

        if is_ok {
            ok_calls += 1;
        } else if outcome == "empty" {
            empty_calls += 1;
        } else if is_error {
            error_calls += 1;
        }

        let entry = tool_map.entry(tool).or_insert_with(|| ToolAccumulator {
            durations: Vec::new(),
            ok_count: 0,
            error_count: 0,
            tokens: 0,
        });

        entry.durations.push(duration);
        if is_ok {
            entry.ok_count += 1;
        }
        if is_error {
            entry.error_count += 1;
        }
        entry.tokens += tokens;
    }

    let mut tool_stats = Vec::new();
    for (tool, acc) in tool_map {
        let count = acc.durations.len();
        let median = if count == 0 {
            0
        } else {
            acc.durations[count / 2]
        };

        tool_stats.push(ToolStat {
            tool,
            count,
            ok_count: acc.ok_count,
            error_count: acc.error_count,
            median_duration_ms: median,
            tokens_returned: acc.tokens,
        });
    }

    tool_stats.sort_by_key(|b| std::cmp::Reverse(b.count));

    let mut error_stmt = conn.prepare(
        "SELECT timestamp, tool, error_message
         FROM tool_telemetry
         WHERE outcome = 'error' AND error_message IS NOT NULL
         ORDER BY timestamp DESC
         LIMIT 10",
    )?;

    let error_rows = error_stmt.query_map([], |row| {
        Ok(TelemetryErrorRecord {
            timestamp: row.get(0)?,
            tool: row.get(1)?,
            error_message: row.get(2)?,
        })
    })?;

    let mut recent_errors = Vec::new();
    for err in error_rows.flatten() {
        recent_errors.push(err);
    }

    Ok(TelemetrySummary {
        total_calls,
        ok_calls,
        empty_calls,
        error_calls,
        total_tokens_returned: total_tokens,
        tool_stats,
        recent_errors,
    })
}

/// Format telemetry summary into human-readable terminal output.
pub fn format_telemetry_summary(summary: &TelemetrySummary) -> String {
    let mut out = String::new();
    out.push_str("=================================================================\n");
    out.push_str("                    code-kb Telemetry Summary                    \n");
    out.push_str("=================================================================\n");

    if summary.total_calls == 0 {
        out.push_str("No tool calls recorded in this workspace yet.\n");
        return out;
    }

    let success_rate = if summary.total_calls > 0 {
        (summary.ok_calls as f64 / summary.total_calls as f64) * 100.0
    } else {
        0.0
    };

    out.push_str(&format!(
        "Total Tool Calls: {} | Success Rate: {:.1}% | Tokens Served: ~{}\n\n",
        summary.total_calls, success_rate, summary.total_tokens_returned
    ));

    out.push_str("### Tool Invocations & Performance\n");
    out.push_str("| Tool | Calls | Median Latency | Tokens Served | Success Rate |\n");
    out.push_str("|---|---:|---:|---:|---:|\n");

    for stat in &summary.tool_stats {
        let rate = if stat.count > 0 {
            (stat.ok_count as f64 / stat.count as f64) * 100.0
        } else {
            0.0
        };
        out.push_str(&format!(
            "| `{}` | {} | {} ms | ~{} | {:.1}% |\n",
            stat.tool, stat.count, stat.median_duration_ms, stat.tokens_returned, rate
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
    fn test_telemetry_recording_and_summary() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        let inv1 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 6,
            outcome: "ok",
            error_message: None,
            result_count: 5,
            bytes_returned: 1200,
            est_tokens: 300,
        };
        record_tool_call(root, &inv1);

        let inv2 = ToolInvocation {
            tool: "file_skeleton",
            duration_ms: 4,
            outcome: "ok",
            error_message: None,
            result_count: 3,
            bytes_returned: 800,
            est_tokens: 200,
        };
        record_tool_call(root, &inv2);

        let inv3 = ToolInvocation {
            tool: "replace_symbol_body",
            duration_ms: 12,
            outcome: "error",
            error_message: Some("Syntax error in Rust function"),
            result_count: 0,
            bytes_returned: 50,
            est_tokens: 12,
        };
        record_tool_call(root, &inv3);

        let summary = get_telemetry_summary(root).unwrap();
        assert_eq!(summary.total_calls, 3);
        assert_eq!(summary.ok_calls, 2);
        assert_eq!(summary.error_calls, 1);
        assert_eq!(summary.total_tokens_returned, 512);

        assert_eq!(summary.tool_stats.len(), 2);
        let skel_stat = summary
            .tool_stats
            .iter()
            .find(|s| s.tool == "file_skeleton")
            .unwrap();
        assert_eq!(skel_stat.count, 2);
        assert_eq!(skel_stat.ok_count, 2);

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
}
