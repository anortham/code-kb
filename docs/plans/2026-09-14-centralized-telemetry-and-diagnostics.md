# Centralized Global Telemetry and Diagnostic Bug Reporting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use razorback:subagent-driven-development whenever delegation is available and permitted, including for one task; serialize dependent tasks. Use razorback:executing-plans only when delegation is unavailable or the user/session explicitly selected single-agent execution.

**Goal:** Centralize `code-kb` tool telemetry into `~/.code-kb/telemetry.db` with workspace attribution and 365-day retention, expose `telemetry_summary` via MCP and CLI `stats` with time-window filtering and token-savings tracking, and provide a `bug-report` diagnostic command that generates sanitized GitHub issues.

**Architecture:** Telemetry collection is decoupled from per-workspace AST index caches (`artifact.db`) and stored in a shared SQLite WAL database at `~/.code-kb/telemetry.db` (overrideable via `CODE_KB_TELEMETRY_DIR`). Queries aggregate metrics directly in SQL without in-memory event loops, preserving the < 15 MB heap constraint. MCP tools enforce zero workspace parameters, and early routing prevents unindexed workspaces from triggering scans during stats calls.

**Tech Stack:** Rust 1.95 (Edition 2024), SQLite 3 (rusqlite with WAL mode and bundled limits), clap (derive), serde/serde_json, dunce, url.

**Spec:** [`docs/plans/2026-09-14-centralized-telemetry-and-diagnostics-design.md`](file:///home/murphy/source/code-kb/docs/plans/2026-09-14-centralized-telemetry-and-diagnostics-design.md)

**Architecture Quality:** Low risk. Centralizes telemetry persistence while preserving workspace-local self-cleaning AST caches (`artifact.db`). Enforces strict Invariant 1 (zero workspace parameters in tool schemas) and Invariant 2 (< 15 MB retained heap).

## Global Constraints

- Never expose `workspace`, `workspace_id`, `repo_path`, or `root_dir` in any MCP tool schema (Invariant 1).
- `artifact.db` remains strictly in `<workspace>/.code-kb/artifact.db`; `telemetry.db` lives centrally in `~/.code-kb/telemetry.db` (Invariant 6).
- All aggregations must execute in SQLite (`COUNT`, `SUM`, `AVG`, `GROUP BY`) to maintain retained memory < 15 MB (Invariant 2).
- Zero panics or failures during tool execution if telemetry writes fail (best-effort non-blocking writes).
- Windows paths must be canonicalized and formatted with forward slashes via `code_kb_core::normalize_path` and `dunce::simplified`.
- `skills/code-kb/SKILL.md` and `.claude-plugin/skills/code-kb/SKILL.md` must stay byte-for-byte identical (`test_skills_md_sync_contract`).
- `AGENTS.md` and `CLAUDE.md` must stay byte-for-byte identical.

---

## Verification Strategy

**Project source of truth:** [`AGENTS.md`](file:///home/murphy/source/code-kb/AGENTS.md) and [`Cargo.toml`](file:///home/murphy/source/code-kb/Cargo.toml).

**Worker red/green scope:**
- Task 1: `cargo test -p code-kb-core telemetry`
- Task 2: `cargo test -p code-kb-cli --test mcp_test`
- Task 3: `cargo test -p code-kb-cli --test cli_test`
- Task 4: `cargo test -p code-kb-cli test_skills_md_sync_contract`

**Worker ceiling:** `cargo test --workspace`

**Worker gate invariant:** All unit and integration tests compile with zero warnings and pass cleanly without touching user's actual `~/.code-kb` directory (using `CODE_KB_TELEMETRY_DIR`).

**Lead affected-change scope:** `cargo test --workspace`

**Branch gate:** `cargo test --workspace && cargo clippy --workspace -- -D warnings`

**Security scope:** `none declared`

**Replay/metric evidence:** Retained memory remains < 15 MB; SQLite WAL query latency < 5 ms for summary aggregations.

**Escalation triggers:** Any failure in `mcp_test.rs` validating zero workspace schema parameters.

**Assigned verification failure:** Workers stop and report when assigned verification fails, unless this plan explicitly says to update that gate.

**Verification ledger:** Record invariant, command, scope label, commit SHA, result, and timestamp.

---

## Parallel Execution Contract

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| Task 1: Core Telemetry Engine | Batch A | `crates/code-kb-core/src/telemetry.rs`, `crates/code-kb-core/src/lib.rs` | Yes | Foundational database schema and query engine required by CLI and MCP. |
| Task 2: MCP Server Routing & Tool | Batch B | `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/tests/mcp_test.rs`, `crates/code-kb-cli/tests/adversarial_m2_server_test.rs`, `crates/code-kb-cli/tests/adversarial_m3_server_test.rs` | Yes | Depends on Task 1 types and core methods. |
| Task 3: CLI Commands & Bug Report | Batch C | `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/tests/cli_test.rs` | Yes | Depends on Task 1 core functions and CLI args. |
| Task 4: Skill & Guideline Sync | Batch D | `skills/code-kb/SKILL.md`, `.claude-plugin/skills/code-kb/SKILL.md`, `AGENTS.md`, `CLAUDE.md` | Yes | Documents tools and contracts completed in Tasks 1-3. |

---

### Task 1: Core Telemetry Engine Updates

**Files:**
- Modify: `crates/code-kb-core/src/telemetry.rs:1-375`
- Modify: `crates/code-kb-core/src/lib.rs:40-50`

**Interfaces:**
- Consumes: `rusqlite`, `dunce`, `code_kb_core::normalize_path`, `std::env`.
- Produces: `open_global_telemetry_db`, `TimeWindow`, `TelemetryFilter`, `TelemetrySummary`, `BugReportBundle`, `get_telemetry_summary`, `generate_bug_report`, `migrate_legacy_workspace_telemetry`.

**Contract inputs:**
- Global directory resolved from `CODE_KB_TELEMETRY_DIR`, falling back to `HOME` (Unix) or `USERPROFILE` (Windows) joined with `.code-kb`.
- Schema includes `workspace_root`, `workspace_name`, `est_tokens_saved`, and 365-day pruning.

**File ownership:** `crates/code-kb-core/src/telemetry.rs`, `crates/code-kb-core/src/lib.rs`

**Serialization required:** Yes

**Dependency reason:** Initial foundational task establishing the database schema and public query API.

**Step 1: Write failing unit tests**
In `crates/code-kb-core/src/telemetry.rs`, add tests:
- `test_global_telemetry_dir_isolation()` verifying `CODE_KB_TELEMETRY_DIR` controls database location.
- `test_telemetry_filter_time_windows()` verifying `Today`, `Last7Days`, `Last30Days`, `ThisMonth`, `LastYear`, `AllTime` filtering.
- `test_telemetry_workspace_scoping()` verifying scoping to specific workspace vs global.
- `test_est_tokens_saved_aggregation()` verifying `SUM(est_tokens_saved)` in SQL summary.
- `test_legacy_migration()` verifying migration from mock `.code-kb/telemetry.db` and old file cleanup.
- `test_bug_report_bundle_generation()` verifying markdown and GitHub issue URL generation.

**Step 2: Run tests to verify they fail**
Run: `cargo test -p code-kb-core telemetry`
Expected: Compilation failure (missing types/fields) or test failures.

**Step 3: Implement core telemetry engine**
- Implement `get_global_telemetry_dir() -> PathBuf` and `open_global_telemetry_db() -> Result<Connection, QueryError>`.
- Update `open_telemetry_db` to delegate to `open_global_telemetry_db()`.
- Add table columns `workspace_root`, `workspace_name`, `est_tokens_saved` and indexes `idx_tool_telemetry_ws_ts`, `idx_tool_telemetry_tool`, `idx_tool_telemetry_ts`.
- Update pruning to 365 days.
- Implement `TimeWindow` enum with `Default` (`AllTime`), `parse`, and `to_sqlite_condition`.
- Implement `TelemetryFilter` struct.
- Implement `record_tool_call_conn` and `record_tool_call` with normalized forward-slash workspace paths.
- Implement `get_telemetry_summary(conn, filter)` using SQL aggregations (`COUNT`, `SUM`, `ROUND(AVG(duration_ms)) as avg_duration_ms`) without holding in-memory duration vectors.
- Implement `generate_bug_report(conn, workspace_root, title)` compiling OS, arch, versions, recent error logs, and URL-encoded GitHub issue link.
- Implement `migrate_legacy_workspace_telemetry(global_conn, workspace_root)`.
- Re-export new public types in `crates/code-kb-core/src/lib.rs`.

**Step 4: Run tests to verify they pass**
Run: `cargo test -p code-kb-core telemetry`
Expected: PASS (all tests green).

**Step 5: Apply commit mode**
Apply `serial-worker-commit`: commit owned files with message `feat(core): centralize telemetry in global database with workspace attribution and bug report generation`.

**Acceptance criteria:**
- [x] Telemetry is stored centrally in `$CODE_KB_TELEMETRY_DIR/telemetry.db` or `~/.code-kb/telemetry.db`.
- [x] Retention prunes records older than 365 days.
- [x] `TimeWindow` filters records accurately across day/week/month/year/all.
- [x] `est_tokens_saved` is tracked and aggregated in O(1) memory.
- [x] Legacy workspace databases are migrated and cleaned up.
- [x] Worker-scope verification passes (`cargo test -p code-kb-core telemetry`).

---

### Task 2: MCP Server Routing, Tool Schema & Parity

**Files:**
- Modify: `crates/code-kb-cli/src/mcp/server.rs:20-450`
- Modify: `crates/code-kb-cli/tests/mcp_test.rs:170-190`
- Modify: `crates/code-kb-cli/tests/adversarial_m2_server_test.rs:810-820`
- Modify: `crates/code-kb-cli/tests/adversarial_m3_server_test.rs:665-675`

**Interfaces:**
- Consumes: Task 1 `open_global_telemetry_db`, `get_telemetry_summary`, `TelemetryFilter`, `TimeWindow`.
- Produces: MCP tool `telemetry_summary` (alias `code_kb_stats`), updated `tools/list` (length 11).

**Contract inputs:**
- Invariant 1: Strictly zero workspace parameters in schema.
- Early routing: `telemetry_summary` / `code_kb_stats` must execute before `!self.db_path.exists()` check and auto-scan logic.

**File ownership:** `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/tests/mcp_test.rs`, `crates/code-kb-cli/tests/adversarial_m2_server_test.rs`, `crates/code-kb-cli/tests/adversarial_m3_server_test.rs`

**Serialization required:** Yes

**Dependency reason:** Depends on Task 1 types and core telemetry engine.

**Step 1: Write failing integration test**
In `crates/code-kb-cli/tests/mcp_test.rs`:
- Add test calling `telemetry_summary` over MCP protocol with `time_window: "month"`, asserting successful summary response.
- Add test verifying `telemetry_summary` succeeds on an unindexed repository without generating `artifact.db`.
- Verify `mcp_test.rs` asserts `tools.len() == 11`.

**Step 2: Run test to verify it fails**
Run: `cargo test -p code-kb-cli --test mcp_test`
Expected: FAIL (tool count mismatch or unknown tool `telemetry_summary`).

**Step 3: Implement MCP server changes**
- Initialize `telemetry_conn` on `McpServer` with `open_global_telemetry_db().ok()`.
- In `bind_workspace`: call `migrate_legacy_workspace_telemetry` for the new workspace root; do not replace `telemetry_conn` with a local per-workspace database.
- Register `telemetry_summary` in `list_tools`:
  - `time_window`: optional string (`"today"`, `"7d"`, `"30d"`, `"month"`, `"year"`, `"all"`).
  - `workspace_only`: optional boolean.
  - Zero workspace path parameter in schema.
- In `handle_call_tool_inner`:
  - Route `telemetry_summary` and alias `code_kb_stats` immediately before the `!self.db_path.exists()` check.
  - Implement `handle_telemetry_summary`: build `TelemetryFilter`, call `get_telemetry_summary`, format into markdown or JSON string.
- Update tool length assertions (`11`) in `mcp_test.rs`, `adversarial_m2_server_test.rs`, and `adversarial_m3_server_test.rs`.

**Step 4: Run test to verify it passes**
Run: `cargo test -p code-kb-cli --test mcp_test`
Expected: PASS (all MCP tests pass including Invariant 1 schema verification).

**Step 5: Apply commit mode**
Apply `serial-worker-commit`: commit owned files with message `feat(mcp): add telemetry_summary tool with early routing and global storage`.

**Acceptance criteria:**
- [x] `telemetry_summary` is exposed in `tools/list` with zero workspace parameters.
- [x] `code_kb_stats` acts as an unadvertised alias in `handle_call_tool_inner`.
- [x] Calling `telemetry_summary` on an unindexed directory does not trigger auto-scan.
- [x] Total advertised tools count in `tools/list` is 11.
- [x] Worker-scope verification passes (`cargo test -p code-kb-cli --test mcp_test`).

---

### Task 3: CLI Commands & Bug Report Subcommand

**Files:**
- Modify: `crates/code-kb-cli/src/main.rs:70-340`
- Modify: `crates/code-kb-cli/tests/cli_test.rs:300-350`

**Interfaces:**
- Consumes: Task 1 `get_telemetry_summary`, `generate_bug_report`, `TelemetryFilter`, `TimeWindow`.
- Produces: CLI commands `code-kb stats [--since <w>] [--workspace] [--json]` and `code-kb bug-report [--title <t>] [--json]`.

**Contract inputs:**
- Default `code-kb stats` aggregates globally across all workspaces unless `--workspace` is passed.
- `code-kb bug-report` prints sanitized diagnostic markdown and clickable GitHub issue URL.

**File ownership:** `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/tests/cli_test.rs`

**Serialization required:** Yes

**Dependency reason:** Depends on Task 1 and integrates CLI arg parsing and commands.

**Step 1: Write failing CLI tests**
In `crates/code-kb-cli/tests/cli_test.rs`:
- Add test for `code-kb stats --since 30d`.
- Add test for `code-kb stats --workspace --json`.
- Add test for `code-kb bug-report --title "test bug"`.
- Add test for `code-kb bug-report --json`.

**Step 2: Run test to verify it fails**
Run: `cargo test -p code-kb-cli --test cli_test test_cli_stats`
Expected: FAIL (missing arguments/subcommand).

**Step 3: Implement CLI updates**
- Update `StatsArgs`:
  - `#[arg(short = 's', long, default_value = "all")] pub since: String`
  - `#[arg(short = 'w', long)] pub workspace: bool`
  - `#[arg(long)] pub json: bool`
- In `main.rs`: update `Command::Stats` / `Command::Telemetry` handler to parse `TimeWindow` and build `TelemetryFilter` with `workspace_root` set only when `--workspace` is present.
- Add `BugReportArgs`:
  - `#[arg(short = 't', long)] pub title: Option<String>`
  - `#[arg(long)] pub json: bool`
- Add `Command::BugReport(BugReportArgs)` to `Commands` enum.
- In `main.rs`: implement `Command::BugReport` handler:
  - Open global telemetry DB.
  - Call `generate_bug_report`.
  - If `--json`, print JSON bundle; otherwise, print formatted terminal markdown and clickable URL.

**Step 4: Run test to verify it passes**
Run: `cargo test -p code-kb-cli --test cli_test`
Expected: PASS (all CLI tests pass).

**Step 5: Apply commit mode**
Apply `serial-worker-commit`: commit owned files with message `feat(cli): add time-windowed stats flags and bug-report diagnostic command`.

**Acceptance criteria:**
- [x] `code-kb stats` defaults to global aggregation and supports `--since` and `--workspace`.
- [x] `code-kb bug-report` outputs system metadata, recent errors, and pre-filled GitHub URL.
- [x] `--json` flag supported on both commands.
- [x] Worker-scope verification passes (`cargo test -p code-kb-cli --test cli_test`).

---

### Task 4: Skill & Guideline Sync Contract

**Files:**
- Modify: `skills/code-kb/SKILL.md`
- Modify: `.claude-plugin/skills/code-kb/SKILL.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`

**Interfaces:**
- Consumes: Completed CLI commands and MCP tool contracts from Tasks 1-3.
- Produces: Byte-for-byte synced skill markdown and project guidelines.

**Contract inputs:**
- `skills/code-kb/SKILL.md` and `.claude-plugin/skills/code-kb/SKILL.md` must be identical.
- `AGENTS.md` and `CLAUDE.md` must be identical.

**File ownership:** `skills/code-kb/SKILL.md`, `.claude-plugin/skills/code-kb/SKILL.md`, `AGENTS.md`, `CLAUDE.md`

**Serialization required:** Yes

**Dependency reason:** Final task documenting the completed system across all agent surfaces.

**Step 1: Write failing test / check sync**
Verify `test_skills_md_sync_contract` in `cli_test.rs`.

**Step 2: Update documentation files**
- In `skills/code-kb/SKILL.md`:
  - Document `telemetry_summary` tool (and CLI `code-kb stats`, `code-kb bug-report`).
  - Explain how agents answer questions like *"how many tokens did code-kb save me this month?"* and generate bug reports.
- Copy `skills/code-kb/SKILL.md` to `.claude-plugin/skills/code-kb/SKILL.md`.
- In `AGENTS.md` and `CLAUDE.md`:
  - Clarify Invariant 6: `artifact.db` is local and self-cleaning per workspace; `telemetry.db` is centralized in `~/.code-kb/telemetry.db`.

**Step 3: Run verification to verify all pass**
Run: `cargo test -p code-kb-cli test_skills_md_sync_contract`
Run: `cargo test --workspace`
Expected: PASS across entire workspace.

**Step 4: Apply commit mode**
Apply `serial-worker-commit`: commit owned files with message `docs: document telemetry_summary tool, bug reporting, and update sync contracts`.

**Acceptance criteria:**
- [x] `skills/code-kb/SKILL.md` and `.claude-plugin/skills/code-kb/SKILL.md` are byte-for-byte identical.
- [x] `AGENTS.md` and `CLAUDE.md` are byte-for-byte identical.
- [x] Full branch gate verification passes (`cargo test --workspace && cargo clippy --workspace -- -D warnings`).
