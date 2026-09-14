# Autonomous Execution Report - Centralized Global Telemetry and Diagnostic Bug Reporting

**Status:** Awaiting publication approval
**Plan:** docs/plans/2026-09-14-centralized-telemetry-and-diagnostics.md
**Branch:** main
**PR:** pending — filled in after PR creation
**Publication authority:** local commit=authorized (user implementation request); push=missing; PR=missing
**Duration:** 1 hour 45 minutes
**Phases:** 1/1 complete
**Tasks:** 4/4 complete
**External-model policy:** policy honored (codex, xai)

## What shipped
- Centralized global SQLite WAL telemetry engine at `~/.code-kb/telemetry.db` (or `$CODE_KB_TELEMETRY_DIR`) with workspace attribution, 365-day retention pruning, and O(1) heap aggregation in `crates/code-kb-core/src/telemetry.rs`.
- MCP server routing and tool registration for `telemetry_summary` with alias `code_kb_stats` in `crates/code-kb-cli/src/mcp/server.rs`, strictly enforcing Invariant 1 (zero workspace parameters) and early routing on unindexed repositories without triggering auto-scans (11 tools total).
- CLI subcommands `code-kb stats [--since <WINDOW>] [--workspace] [--json]` and `code-kb bug-report [--title <TITLE>] [--json]` in `crates/code-kb-cli/src/main.rs`.
- Diagnostic bug report generator in `code-kb-core` compiling sanitized system metadata, recent error logs, formatted markdown tables, and clickable pre-filled GitHub issue submission URLs.
- Documentation and byte-for-byte sync contracts updated across `skills/code-kb/SKILL.md`, `.claude-plugin/skills/code-kb/SKILL.md`, `AGENTS.md`, and `CLAUDE.md`, clarifying Invariant 6 (self-cleaning workspaces for `artifact.db` and centralized persistence for `telemetry.db`).
- Remediated all 6 findings from Codex reviews (migration transaction safety, same-file self-deletion prevention, schema upgrade ordering, path sanitization, avoiding workspace binary execution, MCP recent-error isolation).
- Remediated all 6 findings from Grok 4.6 review (test isolation via `CODE_KB_TELEMETRY_DIR`, CLI stats error scoping, `telemetry_summary` no-rebind early routing, time window validation and display, CLI legacy migration, and localtime SQLite date calculations).

## Judgment calls (non-blocking decisions made)
- `crates/code-kb-core/src/telemetry.rs:125-145` — Extracted `resolve_telemetry_dir(env_dir, home, userprofile)` as a helper function alongside `get_global_telemetry_dir()`. In Rust 2024 (1.95+), `std::env::set_var` is marked unsafe, while the workspace explicitly enforces `#![forbid(unsafe_code)]`. Extracting `resolve_telemetry_dir` allows testing resolution precedence with pure deterministic paths without needing `unsafe` blocks in tests.
- `crates/code-kb-core/src/telemetry.rs:161-165` — Added internal test-scoped override `set_telemetry_dir_override` to allow integration unit tests to isolate database creation cleanly without mutating the global user environment or violating `forbid(unsafe_code)`.
- `crates/code-kb-core/src/telemetry.rs:570-658` — In `migrate_legacy_workspace_telemetry`, chose to collect legacy rows into an intermediate vector and drop `legacy_conn` before deleting `<workspace>/.code-kb/telemetry.db`. This strictly guarantees that file handles are completely closed prior to deletion, satisfying Invariant 5 for Windows file locking semantics.
- `crates/code-kb-cli/src/mcp/server.rs:428` — Parsed optional parameter aliases `"since"` and `"window"` for `time_window` in `handle_telemetry_summary` per Core Invariant 6 ("Zero-Friction Tool Ergonomics: Tool handlers accept intuitive parameter aliases").
- `crates/code-kb-cli/src/mcp/server.rs:452` — Supported optional `"json": true` argument in `handle_telemetry_summary` allowing callers flexibility between human-readable markdown and structured JSON.
- `crates/code-kb-cli/src/mcp/server.rs:538-552` — Moved `telemetry_summary` / `code_kb_stats` early route to the very top of `handle_call_tool_inner` before any workspace path extraction or rebinding, guaranteeing that telemetry lookups remain completely side-effect free and never switch the active workspace session.
- `crates/code-kb-cli/src/main.rs:340` — Executed `Command::BugReport` early alongside `Command::Stats` and `Command::Logs`, before `workspace.locate_db()` and `artifact.db` validation, because bug reporting and telemetry do not require an active indexed `artifact.db`.
- `skills/code-kb/SKILL.md:43-61` — Placed `## Telemetry & Diagnostics` after Phase 4 rather than inside the progressive disclosure workflow phases, because progressive disclosure describes code navigation/editing workflows, while telemetry and diagnostics represent operational introspection and troubleshooting.

## External review (codex & grok, adversarial)
- **Reviewers:** OpenAI Codex (Passes: general 1 / security 1) & xAI Grok 4.6 (Code review pass 1)
- **Findings total:** 12
- **Verified real, fixed:** 12 (commits: `24c973e`, `f934555`, `1fbb4af`, `81baf84`)
  - [codex: general+security] Migration can unlink the active global database when workspace is HOME or points to `.code-kb` (`is_same_file` / canonical identity check added before opening or deleting).
  - [codex: security] Destination inserts could fail during legacy migration, discarding errors and deleting legacy DB (wrapped migration in transaction, propagate failure, delete legacy DB only upon successful commit).
  - [codex: general] Schema upgrade fails before adding required columns when compound index is created before ALTER TABLE statements (`init_telemetry_schema` column migrations executed before dependent index creation).
  - [codex: security] Stored error messages copied verbatim into bug report and GitHub issue URL export unsanitized user paths (`generate_bug_report` redacts user home directory to `~`, escapes markdown table pipes, truncates error messages).
  - [codex: security] Generating diagnostics could discover and execute repository-supplied `.tools/julie-extract` binary (`generate_bug_report` uses pinned version directly, only probes adjacent to `current_exe()`).
  - [codex: security] Global MCP summaries expose raw error messages and paths from other repositories (`handle_telemetry_summary` scopes `recent_errors` to current active workspace).
  - [grok: high] Integration tests in `mcp_test.rs`, `cli_test.rs`, and `adversarial_m3_server_test.rs` spawned without `CODE_KB_TELEMETRY_DIR`, mutating user's durable home database during `cargo test` (isolated all subprocess spawns with test-local `CODE_KB_TELEMETRY_DIR`).
  - [grok: high] CLI `code-kb stats` without `--workspace` dumped cross-project error messages and paths from unrelated repos (scoped `recent_errors` on global queries to active workspace and sanitized paths).
  - [grok: high] `telemetry_summary` extracted path parameters and rebound session workspace before early routing (moved early routing to the top of `handle_call_tool_inner` so telemetry queries never rebind the active session).
  - [grok: medium] Unknown time window strings silently defaulted to `AllTime` with no feedback (added strict validation in CLI and MCP returning descriptive errors, added `time_window` field to `TelemetrySummary`, and printed window in summary headers).
  - [grok: medium] CLI `stats` and `bug-report` commands did not trigger legacy workspace telemetry migration (added `migrate_legacy_workspace_telemetry` calls in both command handlers).
  - [grok: low] `today` and `month` SQLite date calculations evaluated in UTC rather than localtime (added `'localtime'` modifier to `datetime` calls in `TimeWindow::to_sqlite_condition`).
- **Dismissed:** 0
- **Flagged for your review:** 0
- **Cost:** codex (subscription) + grok ($0.15 API / token usage)

## Review campaign
- **State:** clean
- **Evidence:** cross-model-reviewed
- **Round:** 3
- **External invocations:** 3 (Codex general adversarial, Codex security, Grok 4.6 code review)
- **Open critical/high:** 0
- **Open medium/low:** 0
- **Open at/above floor:** 0

## Tests
- 189 passed tests across `code-kb-core` and `code-kb-cli` unit, adversarial, cli, mcp, blast-radius, edit, freshness, stress, watcher, and worktree suites (0 failing).
- `cargo clippy --workspace -- -D warnings`: PASS (0 warnings, 0 errors).
- `cmp AGENTS.md CLAUDE.md`: PASS (byte-for-byte identical).
- `cmp skills/code-kb/SKILL.md .claude-plugin/skills/code-kb/SKILL.md`: PASS (byte-for-byte identical).

## Blockers hit
- None

## Files changed
- `.claude-plugin/skills/code-kb/SKILL.md` | 22 +-
- `AGENTS.md` | 3 +-
- `CLAUDE.md` | 3 +-
- `crates/code-kb-cli/src/main.rs` | 79 +-
- `crates/code-kb-cli/src/mcp/server.rs` | 191 ++-
- `crates/code-kb-cli/tests/adversarial_m2_server_test.rs` | 4 +-
- `crates/code-kb-cli/tests/adversarial_m3_server_test.rs` | 9 +-
- `crates/code-kb-cli/tests/cli_test.rs` | 202 +++
- `crates/code-kb-cli/tests/mcp_test.rs` | 531 +++++++-
- `crates/code-kb-core/src/lib.rs` | 6 +-
- `crates/code-kb-core/src/telemetry.rs` | 1349 ++++++++++++++++++--
- `docs/plans/2026-09-14-centralized-telemetry-and-diagnostics-design.md` | 299 +++++
- `docs/plans/2026-09-14-centralized-telemetry-and-diagnostics.md` | 296 +++++
- `skills/code-kb/SKILL.md` | 22 +-

## Source control
- **Outstanding:** None — all commits ride on `main`.
- **Worktrees left in place:** None

## Next steps
- Review PR: pending — filled in after PR creation
- Awaiting publication approval from user to push branch `main` to `origin/main` and/or open a PR.
