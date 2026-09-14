# Design: Centralized Global Telemetry and Diagnostic Bug Reporting

## Architecture Quality

**Affected modules:**
- `crates/code-kb-core/src/telemetry.rs`
- `crates/code-kb-cli/src/mcp/server.rs`
- `crates/code-kb-cli/src/main.rs`
- `crates/code-kb-cli/tests/mcp_test.rs`
- `crates/code-kb-cli/tests/cli_test.rs`
- `skills/code-kb/SKILL.md` & `.claude-plugin/skills/code-kb/SKILL.md`
- `AGENTS.md` / `CLAUDE.md`

**Caller-facing interface:**
- **MCP Tool:** `telemetry_summary` (alias: `code_kb_stats` as unadvertised match-arm alias, bringing advertised tools in `tools/list` to 11). Parameters:
  - `time_window` (optional string: `"today"`, `"7d"`, `"30d"`, `"month"`, `"year"`, `"all"`, default `"all"`).
  - `workspace_only` (optional boolean, default `false`).
  - **Zero workspace parameters** in tool schema (strictly enforcing Invariant 1).
- **CLI Commands:**
  - `code-kb stats [--since <window>] [--workspace] [--json]` (alias: `code-kb telemetry`).
  - `code-kb bug-report [--title <text>] [--json]`.
- **Core API:**
  - `open_global_telemetry_db() -> Result<Connection, QueryError>`
  - `get_telemetry_summary(conn: &Connection, filter: &TelemetryFilter) -> Result<TelemetrySummary, QueryError>`
  - `generate_bug_report(conn: &Connection, workspace_root: Option<&Path>, issue_title: Option<&str>) -> Result<BugReportBundle, QueryError>`

**Depth/locality check:**
Telemetry storage moves from per-workspace `<workspace>/.code-kb/telemetry.db` to the global user data directory `~/.code-kb/telemetry.db` (overrideable via `CODE_KB_TELEMETRY_DIR`). The AST syntax index (`artifact.db`) remains completely local to each workspace to preserve self-cleaning AST caches. Telemetry ingestion remains decoupled, non-blocking, and zero-panic.

**Test surface:**
Unit tests in `telemetry.rs` (isolated via `CODE_KB_TELEMETRY_DIR`), integration tests in `crates/code-kb-cli/tests/` (verifying CLI output, zero workspace schema violation in `mcp_test.rs`, tool count bump to 11, skills sync contract, and multi-process SQLite WAL concurrency).

**Seams/adapters:**
Uses `rusqlite` in WAL mode with `busy_timeout = 5000` for concurrent multi-process safety. Cross-platform home directory resolution uses `CODE_KB_TELEMETRY_DIR`, `HOME`, and `USERPROFILE`. Path normalization uses `code_kb_core::normalize_path` and forward slashes.

**Rejected shortcuts:**
- *Rejected flat JSONL logging:* Parsing JSON event logs into memory violates Invariant 2 (< 15 MB heap constraint).
- *Rejected periodic workspace sync:* Deleting an ephemeral worktree or clone before a sync occurs leads to unrecoverable data loss.
- *Rejected external `directories` crate:* Clean stdlib resolution using `HOME`/`USERPROFILE` and `CODE_KB_TELEMETRY_DIR` preserves zero-extra-dependency design.

**Architecture risk:** low

---

## 1. Problem & Motivation

Currently, `code-kb` records tool telemetry into `<workspace_root>/.code-kb/telemetry.db`. While this keeps the workspace directory sandboxed, it presents three major shortcomings:
1. **Data Loss on Workspace Teardown:** Whenever a repository directory or transient git worktree is deleted (`rm -rf` or `git worktree remove`), all tool metrics, token savings history, and error logs are permanently lost.
2. **Inability to Query Cross-Project Efficiency:** Users and AI agents have no way to answer high-level questions like *"how many tokens did code-kb save me this month across all my projects?"*
3. **Friction in Submitting Bug Reports:** When a tool call fails or produces an anomaly, users currently have no streamlined diagnostic command to collect relevant system metadata, version numbers, and recent error telemetry into an actionable GitHub issue.

---

## 2. Core Invariants & Boundaries

1. **Centralized Telemetry vs. Self-Cleaning AST Caches:**
   - [`artifact.db`](../../AGENTS.md#L83-L86) remains strictly inside `<workspace>/.code-kb/artifact.db`. High-volume AST caches remain self-cleaning and zero-orphan.
   - `telemetry.db` moves to `~/.code-kb/telemetry.db` (cross-platform user home directory, overrideable via `CODE_KB_TELEMETRY_DIR`).
2. **Strict Invariant 1 Compliance (Zero Workspace Parameters in Tool Schemas):**
   - The MCP tool `telemetry_summary` **never** exposes `workspace`, `repo_path`, or `workspace_id`.
   - By default, `telemetry_summary` aggregates globally across all projects. If `workspace_only: true` is passed, the server implicitly filters by its internally bound workspace root.
3. **Zero Heap Footprint (< 15 MB):**
   - All aggregations are performed via indexed SQLite SQL queries (`COUNT`, `SUM`, `GROUP BY`), streaming summary rows without loading raw event lists or duration vectors into RAM.
4. **Zero Disruption Guarantee:**
   - Telemetry collection is strictly best-effort and non-panicking. Database or locking errors are logged via `tracing::warn!` and never interrupt tool execution.

---

## 3. Database Schema & Storage

### Storage Location
- Default: `~/.code-kb/telemetry.db` (Linux/macOS `$HOME/.code-kb/telemetry.db`, Windows `%USERPROFILE%\.code-kb\telemetry.db`).
- Configurable override: `$CODE_KB_TELEMETRY_DIR/telemetry.db` (for isolated automated testing and CI).

### Normalized Workspace Paths
To guarantee exact SQL string equality on Windows and Unix alike:
`workspace_root` must always be normalized via `code_kb_core::normalize_path` and converted to forward slashes before insertion or querying.

### SQLite Pragmas & Schema
```sql
PRAGMA journal_mode = WAL;
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

CREATE INDEX IF NOT EXISTS idx_tool_telemetry_ws_ts ON tool_telemetry(workspace_root, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_tool_telemetry_tool ON tool_telemetry(tool, timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_tool_telemetry_ts ON tool_telemetry(timestamp DESC);

-- Automatic 365-day retention pruning
DELETE FROM tool_telemetry WHERE timestamp < datetime('now', '-365 days');
```

### Estimating Tokens Saved (`est_tokens_saved`)
For progressive disclosure tools (`file_skeleton`, `get_symbol_body`, `get_context_slice`, `find_symbol`, `search_symbols`):
- Full-file reading baseline: `file_size_bytes / 4` (or baseline heuristic 4x response tokens when whole file size is not tracked).
- `est_tokens_saved` = `max(0, baseline_tokens - est_tokens_returned)`.
- By storing `est_tokens_saved` per invocation row, SQLite executes `SELECT SUM(est_tokens_saved)` in constant time and minimal memory.

---

## 4. Components & Interfaces

### 4.1 Core Telemetry Engine (`crates/code-kb-core/src/telemetry.rs`)

#### Filter & Enums
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
            Self::Today => Some("timestamp >= datetime('now', 'start of day')"),
            Self::Last7Days => Some("timestamp >= datetime('now', '-7 days')"),
            Self::Last30Days => Some("timestamp >= datetime('now', '-30 days')"),
            Self::ThisMonth => Some("timestamp >= datetime('now', 'start of month')"),
            Self::LastYear => Some("timestamp >= datetime('now', '-365 days')"),
            Self::AllTime => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TelemetryFilter {
    pub time_window: TimeWindow,
    pub workspace_root: Option<PathBuf>,
}
```

#### Summary & Diagnostics Structures
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySummary {
    pub total_calls: usize,
    pub ok_calls: usize,
    pub empty_calls: usize,
    pub error_calls: usize,
    pub total_tokens_returned: usize,
    pub est_tokens_saved: usize,
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
```

#### Core Functions & O(1) Heap Aggregation
* `open_global_telemetry_db() -> Result<Connection, QueryError>`: Opens or creates `~/.code-kb/telemetry.db` with WAL mode and ensures table/indexes exist.
* `record_tool_call_conn(conn: &Connection, workspace_root: &Path, invocation: &ToolInvocation)`: Fast insertion with normalized workspace path attribution.
* `record_tool_call(invocation: &ToolInvocation)`: Opens global DB and records call. No per-workspace database files are written.
* `get_telemetry_summary(conn: &Connection, filter: &TelemetryFilter) -> Result<TelemetrySummary, QueryError>`:
  - Uses SQL aggregations:
    ```sql
    SELECT tool,
           COUNT(*),
           SUM(CASE WHEN outcome = 'ok' THEN 1 ELSE 0 END),
           SUM(CASE WHEN outcome = 'error' THEN 1 ELSE 0 END),
           ROUND(AVG(duration_ms)),
           SUM(est_tokens),
           SUM(est_tokens_saved)
    FROM tool_telemetry
    WHERE ...
    GROUP BY tool
    ```
  - Eliminates in-memory `durations: Vec<u64>` vector allocations across the 365-day dataset, preserving < 15 MB heap.
* `generate_bug_report(conn: &Connection, workspace_root: Option<&Path>, issue_title: Option<&str>) -> Result<BugReportBundle, QueryError>`: Compiles system info, versions, and recent errors into markdown and prefilled GitHub URL.
* `migrate_legacy_workspace_telemetry(global_conn: &Connection, workspace_root: &Path)`:
  - Invoked during `bind_workspace`.
  - Checks if `<workspace_root>/.code-kb/telemetry.db` exists.
  - Reads legacy rows and inserts them into global storage with `INSERT OR IGNORE`.
  - Deletes or renames the legacy local `telemetry.db` file so it is never re-read.

---

### 4.2 MCP Server (`crates/code-kb-cli/src/mcp/server.rs`)

* **Persistent Connection:** Holds a single persistent `telemetry_conn: Option<Connection>` connected to `~/.code-kb/telemetry.db`.
* **Early Routing for Telemetry Tool:**
  - `handle_call_tool_inner` intercepts `telemetry_summary` / `code_kb_stats` **before** the `!self.db_path.exists()` check and auto-scan logic. Querying stats does not trigger an auto-scan or require `artifact.db`.
* **Tool Schema (`telemetry_summary`):**
  - Inputs: `time_window: Option<String>`, `workspace_only: Option<bool>`.
  - Returns formatted markdown or JSON summary of token savings and usage patterns.
  - Strictly no workspace paths exposed.
  - Advertised tools in `tools/list` increases to 11 (`code_kb_stats` is an unadvertised match-arm alias).
* **On Tool Completion:** Invokes `record_tool_call_conn(&conn, &self.workspace.canonical_root, &invocation)` writing to global DB.

---

### 4.3 CLI Commands (`crates/code-kb-cli/src/main.rs`)

1. **`code-kb stats` / `code-kb telemetry`:**
   ```text
   Usage: code-kb stats [OPTIONS]

   Options:
     -s, --since <WINDOW>   Time window: today, 7d, 30d, month, year, all [default: all]
     -w, --workspace        Scope telemetry to current workspace [default: global aggregate]
         --json             Output JSON format
   ```
2. **`code-kb bug-report`:**
   ```text
   Usage: code-kb bug-report [OPTIONS]

   Options:
     -t, --title <TITLE>    Optional issue title
         --json             Output raw JSON diagnostic bundle
   ```
   Outputs formatted terminal markdown with system info, recent errors, and prints a clickable GitHub URL:
   `https://github.com/anortham/code-kb/issues/new?title=...&body=...`

---

### 4.4 Agent Skill & Guidelines (`skills/code-kb/SKILL.md`, `AGENTS.md`)

* Update [`skills/code-kb/SKILL.md`](../../skills/code-kb/SKILL.md) and [`.claude-plugin/skills/code-kb/SKILL.md`](../../.claude-plugin/skills/code-kb/SKILL.md) in lockstep:
  - Document `telemetry_summary` for answering agent token savings inquiries and gathering bug report diagnostics.
* Update [`AGENTS.md`](../../AGENTS.md) and [`CLAUDE.md`](../../CLAUDE.md) in lockstep:
  - Invariant 6 clarified: `artifact.db` is local and self-cleaning per workspace; `telemetry.db` is centralized in `~/.code-kb/telemetry.db`.

---

## 5. Testing & Verification

1. **Core Unit Tests (`telemetry.rs`):**
   - Isolation using `CODE_KB_TELEMETRY_DIR`.
   - Migration from legacy `.code-kb/telemetry.db` with subsequent cleanup.
   - Accurate filtering across `TimeWindow` variants.
   - Normalized workspace path equality on Windows and Unix.
   - Global vs per-workspace scoping.
   - 365-day retention pruning.
   - O(1) heap aggregation without row duration vectors.
   - Bug report generation and URL encoding validity.
2. **CLI & MCP Integration Tests:**
   - Invariant 1 schema verification in `mcp_test.rs` ensuring no workspace parameter is exposed.
   - Verify `tools.len() == 11` in `mcp_test.rs` and update adversarial tests accordingly.
   - Verify `test_skills_md_sync_contract` passes.
   - `code-kb stats` and `code-kb bug-report` CLI verification.
   - Early routing verification: `telemetry_summary` works on an unindexed directory without triggering auto-scan.
   - Concurrent writes via SQLite WAL.
3. **Cross-Platform Compatibility:**
   - Path normalization via `dunce::simplified` and forward slashes.
