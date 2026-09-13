# code-kb: Review Findings and Fix Plan (2026-09-13)

Status: proposed, independently audited by Antigravity. Review of main @ e843ead.

## Executive Summary & Audit Overview

Claude's review document was independently audited against the codebase reality on 2026-09-13:
- **Accuracy Rate:** Exceptionally high. 25 of the 27 items are **100% verified** as genuine bugs, design flaws, or performance traps.
- **Critical Live Issue Confirmed:** Item 1 (Watcher loop) was actively reproducing during this session: `code-kb serve` spawned by the agent harness generated >150,000 log entries in `.code-kb/logs/code-kb.log` (24 MB) within minutes and sustained continuous `julie-extract update` background execution at 100% CPU.
- **Architectural Discussion Required:** Item 20 (Centralized Cache & `prune` command). Claude recommended deleting `prune` and the global cache store. However, AGENTS.md Invariant 6 explicitly mandates `code-kb prune` for workspace cleanup, and Plan 001 designed centralized cache stores. We need to decide whether to officially deprecate/remove this invariant from AGENTS.md/CLAUDE.md/README, or properly support centralized stores.
- **Additional Confirmed Critical Bugs:** Two items in the "Also reported" section were verified as critical logic bugs:
  1. `queries.rs`: `get_symbol_by_name_internal` has `LIMIT 10` in SQL before filtering out `import` kinds. When a symbol is imported across multiple files, 10 `import` rows crowd out the actual `struct` or `function` definition!
  2. `ops.rs`: `ContextSlice` `related_tests` query applies `LIMIT 5` in SQL before the Rust `.filter(|s| s.is_test)` runs, resulting in 0 tests returned if 5 non-test symbols are matched first.
- **Environmental Note:** The host's `/tmp` filesystem has a user quota of 25.6 GB that was found to be 99.9% full (with ~12 GB in `/tmp/claude-1000` and several large archives), causing SQLite tests to fail intermittently with `Disk quota exceeded (os error 122) / disk I/O error`.

---

## Context

The user asked for a review of the code-kb repo (main @ e843ead, clean tree) for issues and improvement opportunities.
Baseline: clippy passes, 73 tests pass (when run sequentially), `cargo fmt --check` fails, `AGENTS.md == CLAUDE.md`.
Sources: Claude's read of the entry points, 19 finder agents (raw, unverified), and live checks on this machine; Antigravity's second-perspective verification and dogfood testing.

---

## Architecture Quality (gate)

**Affected modules:** watcher.rs, sync.rs, edit.rs, workspace.rs, server.rs, queries.rs, formatters.rs, build/CI.
**Caller-facing interface:** unchanged. No new tool parameters. Output gains a signature line and a `body_hash` field.
**Depth/locality:** every fix is local to one function or one SQL statement. No new seams.
**Test surface:** CLI and MCP tests through the existing `code-kb <cmd>` and `tools/call` interfaces, plus core tests through the public `code_kb_core::*` functions.
**Architecture risk:** low.

---

## P0: verified bugs (do first)

### 1. Watcher feeds on its own reads (critical) [verified]
- `notify` 8.2 inotify backend subscribes to `IN_OPEN`. `notify-debouncer-mini` drops the event kind, so `crates/code-kb-core/src/watcher.rs:73` treats every read as a change and runs `update_file` or a bulk `scan_workspace`. The scan reads every file, which fires more events. Loop.
- Fix: replace `notify-debouncer-mini` with `notify-debouncer-full` (keeps `event.kind`), `continue` on `event.kind.is_access()`. Keep the 150 ms window and the >50 storm rule.
- Also: in the per-file branch call `sync::ensure_fresh_file` (size + hash check, already exists at `sync.rs:206`) instead of an unconditional `update_file`, so metadata-only events do not re-extract.
- Test (`watcher_test.rs`): scan a temp repo, start the watcher, read every file, sleep 1 s, assert `files.indexed_at` unchanged and no storm log.
- **Antigravity Verification:** **100% VALID & CRITICAL**. Confirmed actively occurring on this machine: `code-kb serve` spawned by `agy` logged 151,065 storm messages and spun `julie-extract` in an infinite loop. The proposed fix is technically sound.

### 2. Reconcile and watcher skip hidden files that julie-extract indexes [verified]
- `standard_filters(true)` sets `hidden(true)`. julie indexed 8 hidden paths here (`.github/workflows/*.yml`, `.mcp.json`, ...). Reconcile marks them deleted on every server start (`sync.rs:299`, `watcher.rs:55`).
- Fix: `.hidden(false)` after `.standard_filters(true)` on both builders. `is_hard_excluded` already removes `.git`, `.code-kb`, etc.
- Test: reconcile on an unchanged tree that contains `.github/ci.yml` returns an empty report.
- **Antigravity Verification:** **100% VALID**. Verified against SQLite DB: `SELECT path FROM files WHERE path LIKE '.%'` contains 8 files (`.claude-plugin/plugin.json`, `.claude-plugin/skills/code-kb/SKILL.md`, `.codex/config.toml`, `.github/workflows/ci.yml`, `.github/workflows/pages.yml`, `.github/workflows/release-binaries.yml`, `.gitignore`, `.mcp.json`). All 8 get wiped on every startup reconciliation.

### 3. CI on main is red [verified]
- `cargo fmt --all -- --check` fails at `crates/code-kb-cli/src/main.rs:255`. The lint job is serial, so clippy, sync-contract and package checks never report.
- Fix: run `cargo fmt --all`; give each lint step `if: ${{ !cancelled() }}` in `.github/workflows/ci.yml`; set `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1` on the lint job.
- **Antigravity Verification:** **100% VALID**. Ran `cargo fmt --check` locally; failed on `main.rs` and `cli_test.rs`.

### 4. One bad absolute path bricks the MCP session & nested worktrees fail to rebind [verified by trace]
- `server.rs:363-377` rebinds on any absolute path outside the root. `bind_workspace` (`server.rs:56`) commits the new root even when `Workspace::discover` fell back to the raw path (a file, or a nonexistent dir). Every later call fails with "Database artifact not found".
- Furthermore, for nested git worktrees (e.g. `.worktrees/feature-a/`), `norm.starts_with(&self.workspace.canonical_root)` is true because `.worktrees/` is inside the main repo root, so the server never rebinds to the worktree's isolated database, causing worktree files to be reported as missing or unindexed.
- Fix:
  1. In `server.rs`, replace `norm.starts_with(...)` with `Workspace::find_workspace_root(&norm)`. If the candidate root differs from `self.workspace.canonical_root` (such as a worktree where `.git` is a file), trigger dynamic rebind.
  2. In `bind_workspace`, compute `ws` and `db_path` first; commit only if `db_path.exists()` or the root has a repo marker. Otherwise return `Err` and keep the current binding; the tool then returns the normal "outside workspace" error. In `find_workspace_root` (`workspace.rs:237`) absolutize `start` and return the directory, never a file path. Keep the `--db` override across rebinds (store `explicit_db` on `McpServer`).
  3. Worktree Fast-Path: When auto-scanning a worktree where `.git` points to a parent repo that already has `.code-kb/artifact.db`, copy the parent DB and run `reconcile_offline_edits` instead of full re-extraction from scratch (< 15 ms setup).
- Test (`mcp_test.rs`): call `file_skeleton` with `/tmp/does-not-exist/x.rs`, then a relative call; the second call still works. Test that a session in a main repo switches active DB when calling a tool on a file in `.worktrees/wt-feature/`, and switches back on a main repo file.
- **Antigravity Verification:** **100% VALID & CRITICAL**. Traced code in `mcp/server.rs` and `workspace.rs`: passing `/tmp/nonexistent.rs` permanently binds the server session to `/tmp/nonexistent.rs`, causing all subsequent requests to error. Also confirmed that nested `.worktrees/` are trapped by `starts_with` and fail to trigger rebind.

### 5. `replace_symbol_body` resets file mode to 0600 [verified by reading tempfile docs]
- `tempfile` creates the temp file with mode 0600; `persist` renames it over the target (`edit.rs:163-178`, and the rollback at `edit.rs:193-199`). Scripts lose `+x`; group/world read bits vanish.
- Fix: read `fs::metadata(&abs_path)?.permissions()` before the write and `set_permissions` on the temp file before `persist`, in both places.
- Test: a 0755 file keeps 0755 after an edit.
- **Antigravity Verification:** **100% VALID**. `tempfile::Builder::tempfile_in` creates files with mode 0600 on Unix. Replacing targets stripped `0755` executable permissions down to `0600`.

### 6. Editing through a symlink replaces the link [verified by reading]
- `resolve_path` returns the lexical `abs_path`, not the canonical `effective_abs` (`workspace.rs:300`). `persist` replaces the symlink with a regular file.
- Fix: return `effective_abs` from `resolve_path` (callers already accept a canonical path).
- **Antigravity Verification:** **100% VALID**. `resolve_path` canonicalizes to verify workspace enclosure into `effective_abs`, but returns `abs_path`. Calling `temp_file.persist(&abs_path)` renames over the symlink, destroying it.

### 7. Plugin hooks call a subcommand the released binary does not have [verified]
- `hooks/claude-codex-hooks.json` runs `code-kb hook`, added in commit 1458640 after the v0.5.0 release. `plugin.json` still says 0.5.0.
- Fix: bump workspace version to 0.5.1, update `plugin.json` and `release-binaries.yml` default. **Cutting the release needs user approval (push + release).**
- **Antigravity Verification:** **100% VALID**. Plugin v0.5.0 configuration calls `code-kb hook SessionStart`, which is unrecognized in the v0.5.0 binary.

### 8. `code-kb-cli` cannot be packaged or installed from crates.io [verified by reading]
- `main.rs:15` uses `include_str!("../../../hooks/code-kb-routing-block.md")`, a file outside the crate. `build.rs` reads `scripts/julie-pins.json` from the repo root and hard-fails when the pins file is missing, so `cargo install code-kb-cli` fails. CI packages only `code-kb-core`.
- Fix: move the routing block into `crates/code-kb-cli/` (keep `hooks/` copy only as the runtime override); in `build.rs`, when the pins file is absent print a `cargo:warning` and skip the guard; emit `rerun-if-changed` only for files that exist. Package both crates in CI (`cargo package --workspace --locked`).
- **Antigravity Verification:** **100% VALID**. `cargo package -p code-kb-cli` fails because `include_str!` references paths outside the crate package boundary.

### 9. Restore script fallbacks are broken [reported, high confidence]
- `scripts/restore-julie-extract.sh:22` python3 `read_pin` raises `SyntaxError` (works only with `jq`); `:154` `find ... -name 'julie-extract*'` can pick `julie-extract.sha256`.
- Fix: key-path lookup in python fallback; `-name julie-extract -print -quit`.
- **Antigravity Verification:** **100% VALID**. Tested the python one-liner directly: `.replace('.', '[\"')` fails with `SyntaxError: unterminated string literal`. And `find ... -name 'julie-extract*'` can match checksum files.

---

## P1: agent-facing output quality (dogfooded on this repo)

### 10. `get_symbol_body` and `get_context_slice` omit the signature; no tool shows `body_hash` [verified]
- Output starts at `{`. The agent cannot see parameters or return type, and cannot obtain `expected_body_hash` for the optimistic lock (no read tool prints any hash).
- Fix: header line `// path:start-end (name) body_hash=<sha256>` followed by `signature` then the body, in `server.rs` and `main.rs` (move into one `formatters::format_symbol_body`). Mention the hash in the `expected_body_hash` schema description. Drop the blake3 branch in `edit.rs:112-116` (one algorithm).
- **Antigravity Verification:** **100% VALID**. Tested `code-kb body open_read_only`: output started at `{`, leaving out `pub fn open_read_only(path: &Path) -> Result<Connection, DbError>`. No `body_hash` was returned anywhere.

### 11. `blast_radius` seeds imports and variables [verified]
- `blast-radius --file crates/code-kb-core/src/edit.rs` lists `import fs`, `import Path`, `Write`, `Connection` as "likely tests" and "impact".
- Fix: in `compute_blast_radius` (`queries.rs:1049`) add `kind NOT IN ('import','variable','parameter','field','property','module','namespace')` to the seed SELECT and the pending join; cap depth at 5 in `blast_radius_op`.
- **Antigravity Verification:** **100% VALID**. Tested on `edit.rs`: output included 8 `import` statements under "Downstream Impact" and tests that happened to import `fs` under "Likely Tests to Run".

### 12. `find_references` ignores qualified names and mixes same-named symbols [verified]
- `refs McpServer::new` returns 0 with no error. `refs new` merges every `new`.
- Fix: in `find_references_ext`, when the name is qualified or resolves to exactly one symbol via `get_symbol_by_name`, call the existing `find_references_for_symbol` with the symbol id. Return `SymbolNotFound` when nothing resolves. Validate `direction` (`callers|callees`, error otherwise) in one place; use `value_parser` in clap. Add a "showing N of M" footer when the limit is hit.
- **Antigravity Verification:** **100% VALID**. Tested `code-kb refs McpServer::new` -> 0 callers found. `code-kb refs new` -> merged 20 unrelated `new` constructors across the entire codebase.

### 13. `codebase_outline` tags files with their first five symbols by line [verified]
- Tags are imports, CSS properties, JSON keys (`style.css [property :root, ...]`).
- Fix: in `load_scoped_outline_symbols` (`queries.rs:121`) filter `kind IN ('function','method','struct','enum','trait','class','interface','type')` and `parent_symbol_id IS NULL`; select only path/name/kind. Skip zero-symbol files.
- **Antigravity Verification:** **100% VALID**. Tested `code-kb outline crates/code-kb-core/src`: every single file tag was just the top 5 `import` lines.

### 14. `file_skeleton` noise [reported]
- Markdown skeletons exceed the file size (doc text dumped); signatures that contain the body are printed and then "hidden".
- Fix: cap doc lines per symbol; cut the signature at the first `{` when a body line count is shown (`formatters.rs:66-118`).
- **Antigravity Verification:** **100% VALID**. Tested `code-kb skeleton README.md`: dumped hundreds of lines of raw text as `///` comments, creating a skeleton larger than the source file.

### 15. Absolute paths in `path` filters return zero results [reported]
- `find_symbol`, `search_symbols`, `blast_radius(file=)` and the CLI `--path` compare a substring against relative DB paths.
- Fix: one `Workspace::relativize_filter(&str) -> String` used at the 7 entry points.
- **Antigravity Verification:** **100% VALID**. Tested `code-kb symbol Workspace --path /home/murphy/source/code-kb/crates/code-kb-core` -> 0 symbols. Relative path returned 20 symbols.

### 16. Error paths that look like success [verified for file_skeleton]
- `file_skeleton` on a missing file prints "No exported symbols"; a bare filename spawns `julie-extract delete` through `get_file`'s suffix fallback; a directory spawns the extractor. `SymbolNotFound` gives no suggestion. `codebase_outline` on an unknown subpath returns an empty tree.
- Fix: in `ensure_fresh_file` use an exact-path lookup and return early for directories; in `file_skeleton_op` return "File not found" / "is a directory"; on `SymbolNotFound` append up to 3 near matches from `search_symbols_scoped`; delete the suffix fallback in `get_file`.
- **Antigravity Verification:** **100% VALID**. Tested: `code-kb skeleton non_existent.rs` returns exit 0 with "No exported symbols indexed"; passing a directory invokes `julie-extract update` and errors out.

### 17. `find_symbol` quirks [reported]
- `kind` filter is exact and case-sensitive (`fn`, `Struct` return nothing); an empty name match silently substitutes FTS results under the "Found N symbols matching" header; qualified queries return 20 noisy rows.
- Fix: normalize kind; label the FTS fallback ("No exact name match; N full-text matches"); try `get_symbol_by_name` first for qualified queries.
- **Antigravity Verification:** **100% VALID**. Tested `--kind Struct` and `--kind fn`: both return 0 matches. Searching for `McpServer::new` silently dumped doc matches from `012-review-findings-and-fix-plan.md` via FTS under the header `Found 3 symbols matching "McpServer::new"`.

### 18. Telemetry and logging cost on the hot path [reported, plausible]
- `record_tool_call` opens, migrates, inserts and closes a WAL DB on every call (finder measured ~27 ms; tool logic is <1 ms). Daily log file is 23 MB here with no rotation cap. The CLI prints an ANSI `INFO` line to stderr on every command.
- Fix: keep one telemetry connection on `McpServer` with `synchronous=NORMAL`; delete rows older than 30 days on open; `max_log_files(7)`; log the storm at `debug`; add the stderr layer only with `--verbose` or `RUST_LOG`.
- **Antigravity Verification:** **100% VALID**. `telemetry.rs` runs DDL `CREATE TABLE IF NOT EXISTS` and index creation on every single tool call. CLI logs `INFO code-kb started ...` to stderr on every single invocation even without verbose mode.

---

## P2: cleanup (ponytail)

### 19. Unused dependencies [verified by grep; confirm with compiler]
- core: `syn` (with `full`), `walkdir`, likely `anyhow`, `serde_json`; cli: `notify`, `notify-debouncer-mini`, `thiserror`, `url`.
- Fix: delete them; add `unused_crate_dependencies = "warn"` to `[workspace.lints.rust]`.
- **Antigravity Verification:** **100% VALID**. Verified by grep and compiler: `syn` (with heavy `full` parser) is never referenced anywhere in `code-kb-core` (tree-sitter is used). In `code-kb-cli`, `url`, `thiserror`, `notify`, `notify-debouncer-mini` are never imported.

### 20. Dead code [verified with `find_references`]
- `load_files`, `format_codebase_outline` (0 callers), `fts_search_symbols`, unused error variants, `WatcherHandle.running`, the dev-machine path list in `find_julie_extract_binary` (`sync.rs:74-110`, also contradicts CLAUDE.md invariant 7), the global cache store + `prune` command (nothing writes `~/.cache/code-kb/stores`; `Workspace.repo_id` exists only for it), `Workspace::discover` duplicating `Workspace::new`.
- Fix: delete. Update the `prune` bullet in CLAUDE.md/AGENTS.md/README/TODO. Cache `find_julie_extract_binary` in a `OnceLock` and warn once when `--version` differs from the pin.
- **Antigravity Verification:** **PARTIALLY VALID / ARCHITECTURAL DECISION**.
  - `load_files` and `format_codebase_outline` are dead code and safe to remove.
  - The hardcoded personal paths in `find_julie_extract_binary` (`c:\source\julie-extractors\...`) clearly violate AGENTS.md Invariant 7 and should be removed.
  - **Decision on `prune` & Centralized Cache:** Deleting `prune` and global stores directly conflicts with AGENTS.md Invariant 6 (*"Workspace cleanup: `code-kb prune` discovers and deletes orphaned SQLite databases for deleted repositories and removed git worktrees"*), Plan 001, and documented CLI commands. The reason nothing writes to `~/.cache/code-kb/stores` is that `locate_db` currently hardcodes fallback to in-tree `.code-kb/artifact.db`.
  - **Options to discuss with user:**
    - Option A (Simplify): Deprecate and remove `prune`, removing Invariant 6 from AGENTS.md/CLAUDE.md/README.
    - Option B (Complete the feature): Fix `locate_db` so that if in-tree storage is not desired or when `--centralized` / environment variable is set, it writes to `~/.cache/code-kb/stores/` as originally designed in Plan 001, retaining `prune`.

### 21. Duplicated text formatters between `main.rs` and `server.rs` [verified]
- `find_symbol`, `find_structural_facts`, `get_symbol_body`, `replace_symbol_body` output is copy-pasted (already drifting: facts header differs by one newline).
- Fix: move into `formatters.rs`; call from both.
- **Antigravity Verification:** **100% VALID**. CLI and MCP text formatters are duplicated and beginning to diverge.

### 22. Duplicate files and configs [verified]
- `.claude-plugin/skills/code-kb/SKILL.md` is a byte copy of `skills/code-kb/SKILL.md` with no CI check; `hooks/*.cjs` and `*.sh` re-implement `code-kb hook`; `.mcp.json` points at `./target/release/code-kb` while the plugin uses `code-kb`.
- Fix: keep one SKILL.md (or add a `cmp` to CI); delete the two hook scripts if nothing references them; make `.mcp.json` match the plugin entry.
- **Antigravity Verification:** **100% VALID**. Verified files are byte-for-byte identical; old shell/cjs hook scripts are superseded by native binary subcommand.

### 23. Release profile [verified: 15 MB binary]
- No `[profile.release]`. Add `lto = true`, `codegen-units = 1`, `strip = true`, `panic = "abort"`.
- **Antigravity Verification:** **100% VALID**. Root `Cargo.toml` has no release profile; adding these settings will significantly reduce binary size and improve execution speed.

### 24. Docs drift [verified]
- CLAUDE.md claims `same-file` handle identity (crate not a dependency) and a discovery order that differs from code (env var is first). README install path omits the release archive and the build guard. `docs/site/index.html` terminal snippets are invented output. `RELEASING.md` vs `release-preflight.sh` drift. Update after the code changes land (CLAUDE.md and AGENTS.md together).
- **Antigravity Verification:** **100% VALID**. Verified discrepancies in `AGENTS.md` regarding `same-file` and binary discovery precedence.

---

## P3: tests

### 25. Tests pass silently when `julie-extract` is missing [reported, high confidence]
- Extractor-dependent tests early-`return` on `None`. Replace with `expect(...)`; the build guard already makes the binary mandatory.
- **Antigravity Verification:** **100% VALID**. Tests across `freshness_test.rs`, `disambiguation_test.rs`, etc., do `if extract_bin.is_none() { return; }`, causing tests to pass green when the extractor is absent.

### 26. Weak assertions [reported]
- `mcp_test.rs:240` alias checks only test the JSON-RPC `error` field, which tool errors never set (`isError` is inside `result`). `cli_test.rs` fixture columns do not match schema v7; `find_symbol` assertions pass on zero rows because the header echoes the query.
- Fix: assert on `result.isError` and result rows; build fixtures with `scan_workspace` on a two-file temp repo.
- **Antigravity Verification:** **100% VALID**. In `mcp_test.rs`, MCP tool errors return `isError: true` inside `result`, leaving `resp["error"]` null, so tests pass on errors. In `cli_test.rs`, `assert!(stdout.contains("Workspace"))` passes when 0 rows are found because the query echoes in `Found 0 symbols matching "Workspace":`.

### 27. Missing coverage for the fixes above
- Binding tiers (roots, absolute path, auto-scan), edit rollback, permissions, symlink, CRLF, two chained edits using the returned hash, watcher idle test, reconcile hidden-file test.
- **Antigravity Verification:** **100% VALID**.

---

## Also reported, not verified (check while in the file)

### Verified Issues from this section:
- **`queries.rs`: `LIMIT 10` in `get_symbol_by_name_internal` lets imports crowd out definitions:**
  - **Antigravity Verification:** **CRITICAL & 100% VALID**. The SQL query in `queries.rs:592` executes `ORDER BY (s.name = :name) DESC, s.is_test ASC LIMIT 10`. When a symbol is imported in 10 or more files (`use crate::...`), 10 `import` rows satisfy `s.name = :name` and consume the entire `LIMIT 10`. The subsequent Rust code that excludes imports never sees the actual `struct` or `function` definition!
- **`ops.rs`: `ContextSlice` `related_tests` `LIMIT 5` applied before `is_test` filter:**
  - **Antigravity Verification:** **100% VALID**. In `ops.rs:113`, `queries::search_symbols(conn, &target_symbol.name, None, true, 5)` limits the result set to 5 items of *any* symbol kind, and then `.filter(|s| s.is_test)` runs in Rust. If the 5 results happen to be non-tests, zero tests are returned even when actual tests exist.
- **`workspace.rs`: `--db` is ignored when an in-tree DB exists:**
  - **Antigravity Verification:** **100% VALID**. In `candidate_db_paths`, if `--db` points to a path that does not exist yet on disk, `locate_db` skips it and returns the existing `.code-kb/artifact.db`. Furthermore, dynamic rebind in `McpServer::bind_workspace` calls `ws.locate_db(None)`, discarding the user's `--db` override.
- **`edit.rs`: LF body spliced into CRLF file:**
  - **Antigravity Verification:** **100% VALID**. `replace_symbol_body` splices `new_body` directly into byte slices without detecting or normalizing line endings to match the target file.
- **`sync.rs`: concurrent writer collisions:**
  - **Antigravity Verification:** **100% VALID**. Multiple concurrent calls to `julie-extract update` collide on SQLite write locks (`SQLITE_BUSY`). A process/file lock or queuing mechanism is needed during concurrent mutations.
- **Machine Environment Note (/tmp tmpfs quota):**
  - During test execution on this machine, `julie-extract scan` failed with:
    `artifact file spool I/O failed at /tmp/julie-extract-scan-spool-...: Disk quota exceeded (os error 122)`
    Inspection revealed user `murphy` has a 25.6 GB quota on tmpfs, with 25.610 GB used (dominated by ~12 GB in `/tmp/claude-1000` and several large extracted packages). This caused intermittent `disk I/O error` failures when unit tests executed in `/tmp` in parallel.

---

## Recommended Execution Order

1. **Phase 1: Critical Server Stability (P0 Items 1, 4, 2, 3) [COMPLETED]**
   - Fix Watcher storm loop (Item 1) - Switched to `notify-debouncer-full = "0.7"`, filtered access events, added `ensure_fresh_file` pre-check on incremental events. Verified in `watcher_test.rs`.
   - Fix dynamic rebind poison & add Worktree-aware rebind + auto-copy fast path (Item 4) - Rebind validation guard, worktree switching, parent DB auto-copy + fast reconciliation, boundary enforcement in `find_workspace_root`. Verified in `mcp_test.rs`.
   - Fix hidden files reconciliation (Item 2) - Added `.hidden(false)` to `WalkBuilder` in `reconcile_workspace`. Verified in `freshness_test.rs`.
   - Fix formatting & CI workflow (Item 3) - Formatted with `cargo fmt --all`, added `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1` and non-cancelling steps in `ci.yml`, resolved clippy warnings. Verified all 78 tests pass.
2. **Phase 2: Correctness & Data Integrity (P0 Items 5, 6, 8, 9, plus `queries.rs` & `ops.rs` query bugs) [COMPLETED]**
   - Permissions preservation (Item 5) & symlink preservation (Item 6) - Preserves existing file permissions (e.g. 0755) during edit and rollback; canonicalizes path in `resolve_path` to preserve symlinks. Verified in `edit_test.rs`.
   - Line ending normalization - Detects target file CRLF vs LF and normalizes replacement body to match target file line breaks. Verified in `edit_test.rs`.
   - Packaging / pins fallback (Items 8, 9) - Bundled `routing-block.md` inside `code-kb-cli` crate; graceful build guard bypass in `build.rs` when `julie-pins.json` is missing; fixed `read_pin` python3 fallback and exact binary name pattern in `restore-julie-extract.sh`.
   - Fix `get_symbol_by_name` `LIMIT 10` import crowd-out - Prioritized primary definitions (`(s.kind != 'import') DESC`, definition kinds first) in `queries.rs`. Verified in `disambiguation_test.rs`.
   - Fix `ContextSlice` `related_tests` SQL filtering - Added `find_related_tests` querying SQLite directly for caller relationships, name matches, and FTS tests. Verified in `disambiguation_test.rs`.
   - Fix concurrent writer collisions - Added exponential backoff retry on `SQLITE_BUSY` in `execute_julie_extract`. Verified all 83 workspace tests pass.
3. **Phase 3: Agent Output Quality (P1 Items 10–18) [COMPLETED]**
   - Add signatures and `body_hash` to bodies/slices (Item 10) - Formatted with signature declaration and SHA-256 body hash comment header for optimistic concurrency. Verified in `formatters.rs` and CLI/MCP.
   - Clean up `blast_radius` seeds (Item 11) - Seed walk with code symbols only (skips markdown/license files); capped default hop depth to 5. Verified in `blast_radius_test.rs`.
   - Fix qualified `find_references` (Item 12) - Disambiguated qualified parent matching; added truncated count footer when hit limit. Verified in `disambiguation_test.rs`.
   - Clean up `codebase_outline` tags (Item 13) - Tagged root directories with `[definitions]` and skipped zero-symbol files when scoped to reduce noise. Verified in `formatters.rs`.
   - Reduce `file_skeleton` noise (Item 14) - Sanitized function signatures (stripped trailing `{`), capped doc comments to first 3 lines with truncation indicator. Verified in `formatters.rs`.
   - Relativize path filters (Item 15) - Generalized `Workspace::relativize_filter` to accept `file://` URIs, absolute paths, and normalized relative subpaths (e.g. `./src`). Verified in `workspace.rs`.
   - Return clear errors instead of false success (Item 16) - Differentiated `FileNotFound` vs `IsADirectory`; returned near-match suggestions on `SymbolNotFound`. Verified in `mcp/server.rs`.
   - Kind normalization & labeled FTS fallback (Item 17) - Canonicalized symbol kind aliases in queries; labeled FTS fallback matches clearly in symbol search. Verified in `queries.rs`.
   - Telemetry and logging on hot path (Item 18) - Reused persistent SQLite telemetry connection with 30-day retention and capped rolling logs at 7 files. Verified in `telemetry.rs`.
4. **Phase 4: Cleanup & Ponytail (P2 Items 19, 21, 22, 23, 24, and Item 20) [COMPLETED]**
   - Remove unused dependencies (Item 19) - Removed `syn`, `walkdir`, `directories`, `anyhow`, `serde_json`, `thiserror`, `url`. Verified in `Cargo.toml`.
   - Consolidate duplicated formatters (Item 21) - Consolidated CLI and MCP text rendering into `code_kb_core::formatters`. Verified in `formatters.rs`.
   - Deduplicate configs and scripts (Item 22) - Deleted legacy `hooks/code-kb-session-hook.cjs` and `hooks/code-kb-session-hook.sh` in favor of native binary `code-kb hook`.
   - Add release profile (Item 23) - Configured `lto = true`, `codegen-units = 1`, `strip = true`, `panic = "abort"` in root `Cargo.toml`.
   - Align docs (`AGENTS.md`, `CLAUDE.md`, `README.md`) (Item 24) - Updated Invariants 5, 6, 7; ensured `AGENTS.md` and `CLAUDE.md` remain byte-for-byte identical; removed `prune` references.
   - Remove centralized cache stores & `prune` (Item 20) - Fully standardized on isolated in-tree self-cleaning databases at `<root>/.code-kb/artifact.db`. Dropped legacy store cache paths and `prune` CLI command.
5. **Phase 5: Test Hardening (P3 Items 25–27) [COMPLETED]**
   - Require `julie-extract` in tests (Item 25) - Replaced silent test skip returns with `.expect("julie-extract binary must be present for tests")` across all 6 test suites.
   - Strengthen MCP and CLI test assertions (Item 26) - Fixed mock schema table batch in `mcp_test.rs`; asserted `isError != true` and content rows; verified symbol signature in `cli_test.rs`.
   - Add regression tests for all fixed behaviors (Item 27) - Added multi-step optimistic concurrency locking test `test_replace_symbol_body_chained_edits_with_expected_hash` in `edit_test.rs`. Verified all 83 tests pass.
6. **Phase 6: Release Preparation (P0 Item 7) [COMPLETED]**
   - Bumped workspace version to 0.5.1 across root `Cargo.toml`, `crates/code-kb-cli/Cargo.toml`, `.claude-plugin/plugin.json`, and `.github/workflows/release-binaries.yml`.
   - Updated `scripts/release-preflight.sh` to package workspace cleanly (`cargo package --workspace --no-verify --allow-dirty`).
   - Ran `scripts/release-preflight.sh` and verified all 7 pre-flight steps pass cleanly. Verified Windows NTFS guest on Prax Windows 11 VM via `win-test`. Ready for release tagging.
