# Pre-Release v0.5.2 Authoritative Deep Review & Codebase Audit Report

- **Plan ID**: `014`
- **Target**: `code-kb` v0.5.2 Pre-Release
- **Status**: Complete / Authoritative Audit
- **Date**: 2026-09-13
- **Auditors & Contributors**: Teamwork Audit Swarm (`worker_baseline`, `explorer_sqlite_perf`, `explorer_correctness`, `explorer_deadcode`, `explorer_testquality`, `specminer_docs`, `worker_report_writer`)
- **Working Directory**: `/home/murphy/source/code-kb`

---

## 1. Executive Scorecard & Summary

### 1.1 Overview & Audit Scope
In preparation for cutting the `code-kb` v0.5.2 release, a multi-agent deep audit was conducted across the entire codebase (`crates/code-kb-core`, `crates/code-kb-cli`), tool contracts, automated test suites, documentation synchronization, and release automation. The audit rigorously evaluated the system against the **7 Core Invariants** defined in `AGENTS.md` and `CLAUDE.md`, focusing on six core dimensions:
1. **SQLite & Query Performance**: Query latency, index coverage, `EXPLAIN QUERY PLAN` scans on B-trees, FTS5 virtual tables, recursive CTEs, connection lifecycle, and retained memory bounds (< 15 MB).
2. **Correctness & Robust Invariants**: Edge cases in symbol resolution, dynamic workspace rebind, worktree database isolation, filesystem watching, and single-turn atomic edits (`replace_symbol_body`).
3. **Dead Code & Redundancies**: Unused struct fields, functions, methods, orphaned error variants, dead imports, and redundant heap allocations on hot query paths.
4. **Missing Implementations**: Incomplete tool parameters, unhandled error variants, silent failure modes, and argument alias ergonomics.
5. **Test Theater & Test Quality**: Tautological assertions, mocked/vacuous tests, lack of negative coverage, child process leaks, tmpfs quota bypasses, and unverified error handling branches.
6. **Documentation & Contract Parity**: Byte-for-byte synchronization between `AGENTS.md` and `CLAUDE.md`, 1:1 CLI command and MCP tool schema parity, and consistency across `README.md`, release scripts, and manifests.

### 1.2 Overall Codebase Health
`code-kb` exhibits a strong architectural foundation:
- **Baseline Integrity**: All 5 project verification commands pass with 0 errors, 0 clippy warnings (`-D warnings`), 0 formatting diffs, and 85/85 passing tests across 10 test suites.
- **Sync Contracts**: `AGENTS.md` and `CLAUDE.md` are 100% byte-for-byte identical (`f77d16a7...`, 6,436 bytes), and `SKILL.md` copies are identical (`0fcaf719...`).
- **Core Invariant 1 (Zero Workspace Parameters)**: Strictly adhered to across all 10 MCP tool schemas; verified by automated integration tests.
- **Core Invariant 7 (Pinned Extractor)**: Extractor version `2.42.1` is strictly enforced at build time (`build.rs`), runtime discovery (`sync.rs`), and release scripts.

However, the deep audit uncovered **55 actionable findings** across performance, correctness, test quality, and ergonomics that must be tracked and addressed to ensure production resilience.

### 1.3 Findings Summary Statistics

#### Findings by Severity Tier
| Canonical Severity | Count | Primary Impact Areas |
|:---|:---:|:---|
| **CRITICAL** | **1** | Fraudulent resilience test (`freshness_test.rs`) claiming error recovery while testing happy path |
| **HIGH** | **13** | Leading-wildcard symbol scans, 256MB mmap_size, redundant FTS DDL, DB leak on `--db` rebind, worktree WAL copy omission, `get_context_slice` missing `include_external`, silent rebind failure, 33 `safe_tempdir` bypasses, MCP child process leaks, vacuous `\|\|` assertions, 4 untested MCP tools, untested watcher storm circuit breaker, untested atomic edit rollback |
| **MEDIUM** | **23** | `case_sensitive_like` omission, unbounded heap in `load_scoped_files`, outline depth pushdown, connection lifetimes during child execution, unsynchronized threads, `relativize_filter` path cleaning, LIKE wildcard collisions, UTF-8 byte boundary snapping, Windows case-sensitivity, parameter aliases (`symbol`), silent watcher errors, background error swallowing, symbol ID clones, upward directory probes, fragile timing in tests, weakened assertions, vacuous byte slicing, ad-hoc DDL mocks, untested MCP validation, routing block drift check, release preflight scope |
| **LOW** | **18** | Missing index on `literals(kind)`, recursive CTE fanout, hard-excluded directory traversal, git status line parsing, orphaned error variants (`WorkspaceError`, `SyncError`, `WatcherError`), dead public query function, direction normalization, CLI parameter asymmetries, hook ignoring `--root`, quadratic string formatting, multi-cloned symbol vectors, shallow syntax assertions, discovery markers coverage, README catalog parameter omission, manifest version test |
| **TOTAL** | **55** | **1 Critical, 13 High, 23 Medium, 18 Low** |

#### Findings by Focus Category
| Category / Domain | Critical | High | Medium | Low | Total |
|---|:---:|:---:|:---:|:---:|:---:|
| 1. SQLite & Query Performance | 0 | 3 | 5 | 2 | **10** |
| 2. Correctness & Robust Invariants | 0 | 2 | 5 | 2 | **9** |
| 3. Dead Code & Missing Implementations | 0 | 2 | 5 | 10 | **17** |
| 4. Test Theater & Test Quality | 1 | 6 | 6 | 2 | **15** |
| 5. Documentation & Contract Parity | 0 | 0 | 2 | 2 | **4** |
| *Total* | *1* | *13* | *23* | *18* | ***55*** |

---

## 2. Baseline Verification Status

All 5 required baseline verification commands were executed directly in `/home/murphy/source/code-kb` on Linux x86_64. The test environment utilized `TMPDIR=target/tmp` to avoid the known tmpfs user quota exhaustion (`EDQUOT` os error 122) documented in prior audit incident `P2-7`.

### 2.1 Verification Matrix
| # | Verification Command | Working Directory | Exit Code | Result Summary | Output Tally |
|---|---|---|:---:|---|---|
| **1** | `cargo check --all-targets` | `/home/murphy/source/code-kb` | `0` | Clean | Checked `code-kb-core` and `code-kb-cli` in 1.08s; 0 errors, 0 warnings |
| **2** | `cargo clippy --all-targets -- -D warnings` | `/home/murphy/source/code-kb` | `0` | Clean | 0 clippy lints or warnings across dev and test targets |
| **3** | `cargo fmt --check` | `/home/murphy/source/code-kb` | `0` | Clean | 0 diffs; 100% compliant with repo formatting rules |
| **4** | `TMPDIR=target/tmp cargo test --all-targets` | `/home/murphy/source/code-kb` | `0` | Pass | **85 passed; 0 failed; 0 ignored** across 10 test binaries (4.4s) |
| **5** | `bash scripts/release-preflight.sh` | `/home/murphy/source/code-kb` | `0` | Pass | **All 7 release steps PASSED** (sync, versions, fmt, clippy, tests, pkg, win-test) |

### 2.2 Test Suite Execution Breakdown (85 Tests Passed)
1. `src/main.rs (code-kb-cli)`: 0 passed, 0 failed
2. `tests/cli_test.rs`: **16 passed**, 0 failed
   - `test_agents_and_claude_md_sync_contract`: ok
   - `test_skills_md_sync_contract`: ok
   - `test_cli_hook_session_start`: ok
   - `test_cli_hook_default_is_session_start`: ok
   - `test_cli_hook_subagent_start`: ok
   - `test_cli_hook_outside_repo`: ok
   - `test_cli_hook_copilot_env`: ok
   - `test_cli_skeleton`: ok
   - `test_cli_facts`: ok
   - `test_cli_stats_and_telemetry`: ok
   - `test_cli_outline`: ok
   - `test_cli_blast_radius_and_impact`: ok
   - `test_cli_refs`: ok
   - `test_cli_body_and_slice`: ok
   - `test_cli_symbol_and_search`: ok
   - `test_cli_edit_atomic_replacement`: ok
3. `tests/mcp_test.rs`: **4 passed**, 0 failed
   - `test_mcp_invalid_path_does_not_poison_session`: ok
   - `test_mcp_stdio_handshake_and_tools`: ok
   - `test_mcp_worktree_auto_copy_fast_path`: ok
   - `test_mcp_worktree_rebind`: ok
4. `src/lib.rs (code-kb-core)`: **32 passed**, 0 failed
   - 3 formatters tests, 3 queries tests, 2 slicer tests, 10 syntax tests, 5 workspace tests, 3 db tests, 1 sync test, 1 telemetry test, 1 ops test.
5. `tests/blast_radius_test.rs`: **3 passed**, 0 failed
   - `test_blast_radius_multi_hop_and_likely_tests`: ok
   - `test_language_agnostic_callee_filtering`: ok
   - `test_blast_radius_op_file_seed_and_stem_matching`: ok
6. `tests/disambiguation_test.rs`: **7 passed**, 0 failed
   - `test_file_skeleton_exact_path_isolation`: ok
   - `test_context_slice_finds_related_tests`: ok
   - `test_path_filter_boundary_matching`: ok
   - `test_ambiguous_symbol_detection`: ok
   - `test_context_slice_qualified_method_uses_its_own_callees`: ok
   - `test_qualified_parent_disambiguation`: ok
   - `test_get_symbol_by_name_not_crowded_out_by_imports`: ok
7. `tests/edit_test.rs`: **8 passed**, 0 failed
   - `test_replace_symbol_body_rejects_syntax_error`: ok
   - `test_replace_symbol_body_normalizes_crlf`: ok
   - `test_replace_symbol_body_preserves_file_permissions`: ok
   - `test_replace_symbol_body_rejects_stale_indexed_hash_when_disk_differs`: ok
   - `test_replace_symbol_body_atomic`: ok
   - `test_replace_symbol_body_shrunk_file_does_not_panic`: ok
   - `test_replace_symbol_body_preserves_symlinks`: ok
   - `test_replace_symbol_body_chained_edits_with_expected_hash`: ok
8. `tests/freshness_test.rs`: **10 passed**, 0 failed
   - 10 offline reconciliation and file freshness tests.
9. `tests/watcher_test.rs`: **4 passed**, 0 failed
   - 4 background file watching and ignore filtering tests.
10. `tests/worktree_test.rs`: **1 passed**, 0 failed
    - `test_git_worktree_lifecycle_and_index_isolation`: ok

---

## 3. Master Prioritized Findings Table

Findings are sorted by canonical severity: **CRITICAL -> HIGH -> MEDIUM -> LOW**, and indexed with unique identifiers.

| ID | Severity | Focus Area / Invariant | File Citations | Summary | Recommended Action |
|---|:---:|---|---|---|---|
| **TT-01** | **CRITICAL** | Test Theater | `crates/code-kb-core/tests/freshness_test.rs:260-287` | Fraudulent resilience test: `test_reconcile_offline_edits_continues_when_individual_update_fails` sets up 2 valid files and 0 errors. Real failure recovery logic is completely unverified. | Rewrite test to introduce an unreadable file or simulated extraction failure and assert that valid files are still indexed. |
| **PERF-01** | **HIGH** | SQLite / Performance | `crates/code-kb-core/src/queries.rs:281, 297, 301, 313-315` | `search_symbols_scoped` leading-wildcard `LIKE '%query%'` forces full table scan or non-selective `test_container` index scan on every `find_symbol` call. | Perform exact index lookup (`WHERE name = ?1`) first via `idx_symbols_name_kind`; fall back directly to FTS5 virtual table. Eliminate leading `%` scans. |
| **PERF-02** | **HIGH** | Invariant 2 & 5 / Perf | `crates/code-kb-core/src/db.rs:26-32` | `PRAGMA mmap_size = 268435456` (256 MB) violates the < 15 MB retained memory bound; on Windows, memory-mapped handles lock the DB against concurrent writes. | Set `PRAGMA mmap_size = 0;` (or cap at 8 MB) and rely on SQLite's 4 MB page cache (`cache_size = -4000`). |
| **PERF-03** | **HIGH** | SQLite / Performance | `crates/code-kb-cli/src/main.rs:424, 455`<br>`crates/code-kb-cli/src/mcp/server.rs:586, 636`<br>`crates/code-kb-core/src/db.rs:56-119` | `ensure_fts_index_path` opens a new RW connection, executes DDL, and runs `SELECT count(*)` on all symbols on every single search call. | Remove per-search invocations. Execute `ensure_fts_index` strictly during `scan_workspace` and once at `McpServer` startup. |
| **CORR-01** | **HIGH** | Invariant 1 / Rebind | `crates/code-kb-cli/src/mcp/server.rs:65-70`<br>`crates/code-kb-core/src/workspace.rs:369-373` | Server started with `--db` leaks the old database into a rebound workspace. Reconciliation corrupts the old database with new repo files. | Clear `self.explicit_db = None` when rebinding to a new workspace root, or verify that `explicit_db` resides within the new root. |
| **CORR-02** | **HIGH** | Invariant 5 / Worktree | `crates/code-kb-cli/src/mcp/server.rs:443-465` | Worktree DB fast-path copies `artifact.db` without checkpointing or copying `artifact.db-wal`, risking state loss and DB corruption. | Execute a WAL checkpoint before copying, use SQLite's backup API / `VACUUM INTO`, or fall back to `scan_workspace`. |
| **DEAD-01** | **HIGH** | Invariant 6 / Contract | `crates/code-kb-cli/src/mcp/server.rs:224-240, 681-709`<br>`crates/code-kb-cli/src/main.rs:147-154`<br>`crates/code-kb-core/src/ops.rs:103-136`<br>`crates/code-kb-core/src/queries.rs:1044-1133` | Core Invariant 6 Contract Violation: `get_context_slice` lacks the promised `include_external` parameter across schema, server, CLI, and core engine. | Add `include_external: bool` (default `false`) to `get_context_slice` across tool schema, CLI args, ops, and queries. |
| **DEAD-02** | **HIGH** | Correctness / Rebind | `crates/code-kb-cli/src/mcp/server.rs:387-389, 404-409` | Dynamic workspace rebind failure silently executes on the stale/previous workspace without error or warning. | Inspect rebind result; if `bind_workspace` fails, abort tool execution and return an explicit `CallToolResult::error`. |
| **TT-02** | **HIGH** | Test Quality | `crates/code-kb-core/tests/*.rs` (33 tests across 6 files) | All 33 integration tests in `crates/code-kb-core/tests/` bypass `safe_tempdir()`, calling `tempfile::tempdir().unwrap()`, risking tmpfs `EDQUOT`. | Replace all 33 occurrences of `tempfile::tempdir().unwrap()` with `code_kb_core::safe_tempdir()`. |
| **TT-03** | **HIGH** | Test Quality | `crates/code-kb-cli/tests/mcp_test.rs:76, 427, 637, 826` | Child subprocesses in `mcp_test.rs` are only reaped at test completion. Any assertion panic leaks a live `code-kb serve` process holding locks and RAM. | Wrap child processes in an RAII drop guard that calls `child.kill()` and `child.wait()` on unwind. |
| **TT-04** | **HIGH** | Test Quality | `crates/code-kb-cli/tests/cli_test.rs:343`<br>`crates/code-kb-cli/tests/mcp_test.rs:256-260, 285-288` | Vacuous tautological `\|\|` disjunctions accept either output or empty/error output, masking broken query logic. | Seed test fixtures with deterministic data and assert strict output equality without vacuous fallbacks. |
| **TT-05** | **HIGH** | Test Quality / Coverage | `crates/code-kb-cli/tests/mcp_test.rs:145-340` | 4 of 10 MCP tools (`replace_symbol_body`, `get_context_slice`, `get_symbol_body`, `codebase_outline`) are NEVER called via `tools/call` in integration tests. | Add dedicated `tools/call` test cases for all 4 omitted tools in `mcp_test.rs`. |
| **TT-06** | **HIGH** | Test Quality / Watcher | `crates/code-kb-core/src/watcher.rs:101-113` | Zero test coverage for the watcher Git checkout storm circuit breaker (>50 file debounce burst). | Add an integration test that creates 55 files in a rapid burst and verifies that bulk scan is triggered without loop thrashing. |
| **TT-07** | **HIGH** | Invariant 3 / Test Quality | `crates/code-kb-core/src/edit.rs:151-156, 184-217` | Zero test coverage for atomic edit rollback on SQLite failure, concurrent modification, or non-Rust languages. | Add negative tests for `replace_symbol_body` asserting rollback to original disk bytes when indexing fails. |
| **PERF-04** | **MEDIUM** | SQLite / Performance | `crates/code-kb-core/src/db.rs:26-32`<br>`crates/code-kb-core/src/queries.rs:86, 147` | Without `PRAGMA case_sensitive_like = ON`, SQLite's case-insensitive `LIKE` cannot use `idx_files_path` for prefix lookups (`path LIKE 'src/%'`). | Execute `PRAGMA case_sensitive_like = ON;` in `open_read_only` and `open_read_write`. |
| **PERF-05** | **MEDIUM** | Invariant 2 / Memory | `crates/code-kb-core/src/queries.rs:77-111`<br>`crates/code-kb-cli/src/main.rs:386` | Unbounded `Vec<FileFact>` allocation in `load_scoped_files` consumes 17.5–35 MB on large repos (50k–100k files), breaching memory bounds. | Stream or paginate file records, or return compact structs avoiding redundant heap string allocations. |
| **PERF-06** | **MEDIUM** | SQLite / Performance | `crates/code-kb-core/src/ops.rs:176-189` | `codebase_outline_op` omits slash-depth pushdown when querying `files`, loading 100k files across all depths and discarding deep files in Rust. | Push down `AND (length(path) - length(replace(path, '/', '')) <= :max_slashes)` into `codebase_outline_op` files query. |
| **PERF-07** | **MEDIUM** | SQLite / Concurrency | `crates/code-kb-core/src/edit.rs:73, 184`<br>`crates/code-kb-cli/src/main.rs:376`<br>`crates/code-kb-cli/src/mcp/server.rs:494` | Borrowed read connection is held open across disk writes and `sync::update_file` child process in `replace_symbol_body`, inducing `SQLITE_BUSY`. | Drop or finalize read connection immediately after resolving symbol offsets, prior to file I/O and subprocess execution. |
| **PERF-08** | **MEDIUM** | Concurrency / Perf | `crates/code-kb-cli/src/mcp/server.rs:39-44, 95-99` | Unsynchronized fire-and-forget reconciliation threads run `scan_workspace` (>50 changes) concurrently with foreground queries. | Use an `Arc<AtomicBool>` or worker channel to serialize reconciliation and prevent race conditions on `artifact.db`. |
| **CORR-03** | **MEDIUM** | Concurrency / Watcher | `crates/code-kb-cli/src/mcp/server.rs:90-104` | Uncontrolled detached thread spawning in `bind_workspace` when watcher is `None` (spawns a thread on every tool call inspecting a path). | Remove `\|\| self._watcher.is_none()` condition from `bind_workspace` re-trigger check. |
| **CORR-04** | **MEDIUM** | Path Canonicalization | `crates/code-kb-core/src/workspace.rs:315-348` | `relativize_filter` ignores path cleaning (`clean_path`) for relative input containing `.` or `..`, breaking database equality matching. | Apply `clean_path` to relative paths in `relativize_filter` before stripping `./` prefixes. |
| **CORR-05** | **MEDIUM** | Symbol Resolution | `crates/code-kb-core/src/queries.rs:309, 405, 465, 1334-1342` | LIKE wildcard collision (unescaped `_`) in symbol search; in `compute_blast_radius`, escaping `_` breaks exact equality `path = ?{idx}`. | Use `escape_like` with `ESCAPE '\\'` for LIKE; bind raw unescaped string for `path = ?` and escaped string for `LIKE`. |
| **CORR-06** | **MEDIUM** | Invariant 3 / Unicode | `crates/code-kb-core/src/edit.rs:107-146` | `replace_symbol_body` snaps byte offsets for reading, but reconstructs file using raw byte offsets, risking mid-codepoint UTF-8 splits. | Capture snapped `body_start` and `body_end` character boundaries and use them when reconstructing `new_file_bytes`. |
| **CORR-07** | **MEDIUM** | Invariant 5 / Windows | `crates/code-kb-core/src/workspace.rs:248-270` | Case-sensitive `strip_prefix` on Windows in `resolve_path` for non-existent paths falsely rejects paths with mismatched drive letter casing. | Perform case-insensitive path component comparison on Windows when `strip_prefix` fails. |
| **DEAD-03** | **MEDIUM** | Invariant 6 / Ergonomics | `crates/code-kb-cli/src/mcp/server.rs:538-546, 608-616` | `find_symbol` and `search_symbols` lack `symbol` parameter alias, violating Core Invariant 6 ergonomics. | Add `.or_else(\|\| arguments.get("symbol"))` to query parameter resolution in both tool handlers. |
| **DEAD-04** | **MEDIUM** | Watcher / Logging | `crates/code-kb-core/src/watcher.rs:125-131` | File watcher silently swallows `update_file` and `delete_file` errors with `let _ =` without logging or telemetry. | Add `warn!("Watcher failed to update file {rel}: {e}")` and `warn!("Watcher failed to delete file {rel}: {e}")`. |
| **DEAD-05** | **MEDIUM** | Background / Logging | `crates/code-kb-cli/src/mcp/server.rs:40, 92, 455, 586, 636` | Background thread errors from `ensure_fts_index_path` and `reconcile_offline_edits` discarded with `let _ =`. | Log errors at `warn!` level to capture index or reconciliation failures. |
| **DEAD-06** | **MEDIUM** | Memory / Hot Path | `crates/code-kb-core/src/formatters.rs:25-31, 99` | `format_file_skeleton` allocates and clones `Option<String>` symbol IDs for every symbol in file (1,000 allocations on 500 symbols). | Type `children_map` as `HashMap<Option<&str>, Vec<&Symbol>>`, eliminating all String allocations. |
| **DEAD-07** | **MEDIUM** | Tool Latency | `crates/code-kb-cli/src/mcp/server.rs:390-410` | Unconditional upward filesystem traversal (`Workspace::find_workspace_root`) executed on every single tool call. | Short-circuit traversal when `abs_candidate.starts_with(&self.workspace.canonical_root)`. |
| **TT-08** | **MEDIUM** | Test Quality | `crates/code-kb-core/tests/watcher_test.rs:44-54, 240` | Excessive sleep polling (up to 15s) and fragile fixed 500ms sleep in `watcher_test.rs` cause test slowdown and CI flakiness. | Replace fixed sleeps with synchronization primitives or condition variables where feasible. |
| **TT-09** | **MEDIUM** | Test Quality | `crates/code-kb-core/tests/disambiguation_test.rs:298-309` | Diluted assertion: `any(\|t\| t.name.contains("calculate_price") \|\| t.is_test)` passes for ANY test symbol, even if unrelated. | Remove `\|\| t.is_test` and assert exact related test symbol matching. |
| **TT-10** | **MEDIUM** | Test Quality / Slicing | `crates/code-kb-core/src/slicer.rs:130-135` | `test_slice_bytes_safe` only slices an entire ASCII string (`0..len`), never testing multi-byte UTF-8 boundary snapping. | Add unit tests slicing across multi-byte UTF-8 characters (emojis, CJK, accented letters) to exercise boundary snapping. |
| **TT-11** | **MEDIUM** | Test Quality / Schema | Multiple test suites (`cli_test.rs:17`, `mcp_test.rs:20`, `db.rs:176`) | Ad-hoc SQLite DDL string literals duplicated across 9 suites bypass schema v7, masking schema drift. | Centralize test database schema creation into a shared helper function in `code-kb-core`. |
| **TT-12** | **MEDIUM** | Test Quality / MCP | `crates/code-kb-cli/src/mcp/server.rs:529, 615, etc.` | Zero integration tests for MCP parameter validation error branches, unknown methods, or invalid JSON. | Add test cases verifying MCP error responses for missing required params and unknown tool names. |
| **TT-13** | **MEDIUM** | Test Quality / CLI | `crates/code-kb-cli/src/main.rs:312-344, 368-374` | Untested CLI subcommands (`code-kb logs`), missing DB exit handling, and missing `--json` tests for 7 commands. | Add CLI integration tests for `logs`, nonexistent DB, and `--json` formatting. |
| **DOC-02** | **MEDIUM** | Contract Parity | `crates/code-kb-cli/src/routing-block.md:1-22`<br>`hooks/code-kb-routing-block.md:1-22` | Two identical copies of routing text exist without automated synchronization testing in CI or test suites. | Add `test_routing_block_sync_contract` in `cli_test.rs` and add `cmp -s` check in `release-preflight.sh`. |
| **DOC-03** | **MEDIUM** | Release Automation | `scripts/release-preflight.sh:35-51`<br>`.github/workflows/release-binaries.yml:9`<br>`docs/site/index.html:19, 37, 256` | `release-preflight.sh` step `[2/7]` checks `Cargo.toml` and `plugin.json`, but ignores workflow default version and website badges. | Enhance `release-preflight.sh` to assert that workflow default version and website badges match `WS_VER`. |
| **PERF-09** | **LOW** | SQLite / Performance | `crates/code-kb-core/src/queries.rs:1225-1232` | Missing index on `literals(kind)` forces full table scan and two temporary B-trees in `list_structural_fact_categories`. | Add `CREATE INDEX IF NOT EXISTS idx_literals_kind ON literals(kind);`. |
| **PERF-10** | **LOW** | SQLite / Blast Radius | `crates/code-kb-core/src/queries.rs:1381-1389` | Potential fanout explosion in recursive CTE for blast radius when joining common terminal names (e.g. `new`, `run`). | Add recursion step limit or filter out ubiquitous constructor names in branch 2 of `impact_walk`. |
| **CORR-08** | **LOW** | Workspace Discovery | `crates/code-kb-core/src/workspace.rs:183-187, 209-213` | `find_workspace_root` upward traversal aborts on hard-excluded subdirectories (`target`, `.agents`), misidentifying root. | Allow upward traversal through excluded directory names when locating project root from a nested file. |
| **CORR-09** | **LOW** | Git Integration | `crates/code-kb-core/src/ops.rs:271-285` | Fragile porcelain git status line parsing in `blast_radius_op` uses `line.trim()[2..]`. | Parse git porcelain status lines with strict column-based slicing (`line.get(3..)`). |
| **DEAD-08** | **LOW** | Dead Code | `crates/code-kb-core/src/workspace.rs:7-11, 15` | Orphaned error variants `WorkspaceError::CanonicalizationFailed` and `WorkspaceError::DiscoveryFailed` are never constructed. | Remove unused variants or construct them in relevant failure paths. |
| **DEAD-09** | **LOW** | Dead Code | `crates/code-kb-core/src/sync.rs:22` | Orphaned error variant `SyncError::Walk(#[from] ignore::Error)` is never constructed. | Remove unused variant. |
| **DEAD-10** | **LOW** | Dead Code | `crates/code-kb-core/src/watcher.rs:17` | Orphaned error variant `WatcherError::Ignore(#[from] ignore::Error)` is never constructed. | Remove unused variant. |
| **DEAD-11** | **LOW** | Dead Code | `crates/code-kb-core/src/queries.rs:854-862` | Unused public query function `queries::find_references_for_symbol` is only called by its own unit test. | Deprecate or remove `find_references_for_symbol` in favor of `find_references_ext`. |
| **DEAD-12** | **LOW** | Ergonomics / Parity | `crates/code-kb-cli/src/mcp/server.rs:722-726`<br>`crates/code-kb-core/src/queries.rs:821` | `find_references` lacks `dir` alias and case-insensitive / whitespace-trimmed direction normalization. | Add `arguments.get("dir")` fallback and normalize direction with `.trim().to_lowercase()`. |
| **DEAD-13** | **LOW** | CLI Ergonomics | `crates/code-kb-cli/src/main.rs:45-81, 170-183, 199-211` | CLI parameter and subcommand name asymmetries (positional vs flag `symbol`, missing aliases). | Add Clap aliases matching MCP names (`codebase-outline`, `file-skeleton`, `replace-symbol-body`). |
| **DEAD-14** | **LOW** | CLI / Hooks | `crates/code-kb-cli/src/main.rs:251-284` | `code-kb hook` executes before workspace discovery and ignores the global `--root` option. | Respect `cli.root` when resolving custom routing file paths. |
| **DEAD-15** | **LOW** | Robustness / Panic | `crates/code-kb-core/src/ops.rs:271-273` | Unchecked byte slice `trimmed[2..]` on git porcelain status lines risks UTF-8 boundary panics on non-ASCII input. | Use char boundary checks or column indexing. |
| **DEAD-16** | **LOW** | Performance | `crates/code-kb-core/src/queries.rs:732-780` | `get_symbol_by_name_internal` multi-clones full `Vec<Symbol>` structures during filtering passes. | Filter borrowed references (`&Symbol`) and clone only the final resolved match. |
| **DEAD-17** | **LOW** | Performance | `crates/code-kb-core/src/queries.rs:1075-1083, 1111-1119` | `find_callee_signatures` executes linear search on formatted Strings for deduplication. | Use a `HashSet<String>` or `BTreeSet` for O(1) deduplication. |
| **TT-14** | **LOW** | Test Quality | `crates/code-kb-core/src/syntax.rs:149-195` | Shallow `.is_err()` assertions in syntax negative tests never assert error variant or line/col numbers. | Assert specific `SyntaxError::ParseError` and verify reported line/col values. |
| **TT-15** | **LOW** | Test Quality | `crates/code-kb-core/src/workspace.rs:78-110, 198-224` | Untested project discovery markers (`package.json`, `go.mod`, etc.) and excluded directories in `workspace.rs`. | Add unit tests for non-git project root discovery and `is_hard_excluded` directory matching. |
| **DOC-01** | **LOW** | Documentation Parity | `README.md:198` | `README.md` Tool Catalog table entry for `search_symbols` omits `is_test (opt)` from Key Parameters column. | Add `is_test (opt)` to `README.md:198`. |
| **DOC-04** | **LOW** | Test Quality / Docs | `crates/code-kb-cli/tests/cli_test.rs:500-523` | Manifest version agreement (`Cargo.toml` vs `plugin.json`) is tested only in bash, not in `cargo test`. | Add `test_manifest_version_consistency` in `cli_test.rs`. |

---

## 4. Deep Evidentiary Breakdown Across Focus Areas

### 4.1 SQLite & Query Performance

#### PERF-01: Full Table / Non-Selective Index Scans in `search_symbols_scoped`
- **Citation**: `crates/code-kb-core/src/queries.rs:281, 297, 301, 313-315`
- **Call Sites**: `crates/code-kb-cli/src/main.rs:413` (`code-kb symbol`), `crates/code-kb-cli/src/mcp/server.rs:572` (`find_symbol`).
- **Code**:
  ```rust
  let pattern = format!("%{}%", escape_like(query));
  ...
  WHERE (name = ?1 OR name LIKE ?2 ESCAPE '\\')
  ...
  if !include_tests {
      sql.push_str(" AND is_test = 0 AND test_container = 0");
  }
  sql.push_str(" ORDER BY (name = ?1) DESC, (kind IN ('function', 'struct', 'class', 'trait', 'method', 'enum', 'interface', 'type')) DESC, length(name) ASC, path ASC LIMIT ");
  ```
- **Query Plan Analysis**:
  ```text
  sqlite> EXPLAIN QUERY PLAN
          SELECT symbol_id FROM symbols
          WHERE (name = 'PaymentGateway' OR name LIKE '%PaymentGateway%' ESCAPE '\')
          AND is_test = 0 AND test_container = 0
          ORDER BY (name = 'PaymentGateway') DESC, length(name) ASC, path ASC LIMIT 50;

  QUERY PLAN
  |--SEARCH symbols USING INDEX idx_symbols_test_container (test_container=?)
  `--USE TEMP B-TREE FOR ORDER BY
  ```
  When `include_tests = true`:
  ```text
  QUERY PLAN
  |--SCAN symbols
  `--USE TEMP B-TREE FOR ORDER BY
  ```
- **Impact**: Because the pattern starts with `%`, SQLite cannot seek the B-tree index `idx_symbols_name_kind (name, kind)`. SQLite chooses `idx_symbols_test_container`, which has ~95%+ non-selectivity, scanning almost all rows in the table followed by a temporary B-tree sort.
- **Remediation**: Split the search. In `find_symbol`, execute an exact index seek first: `WHERE name = ?1`. If zero rows return, delegate directly to the FTS5 virtual table `symbols_fts`.

#### PERF-02: `PRAGMA mmap_size = 256MB` Violates Memory Bounds (< 15 MB) & Windows Locks
- **Citation**: `crates/code-kb-core/src/db.rs:26-32`
- **Code**:
  ```rust
  conn.execute_batch(
      "PRAGMA busy_timeout = 5000;
       PRAGMA query_only = ON;
       PRAGMA cache_size = -4000;
       PRAGMA mmap_size = 268435456;",
  )
  ```
- **Impact**:
  1. Setting `mmap_size` to 256 MB allows SQLite to map large databases into the process address space. When full table scans execute (e.g. PERF-01), the OS faults pages into physical RAM, expanding RSS far beyond the strict < 15 MB limit of Core Invariant 2.
  2. On Windows (Core Invariant 5), memory-mapped file handles prevent concurrent deletion or modification. When `julie-extract update` runs in a subprocess, it fails with `ERROR_USER_MAPPED_FILE` (1224) or `ERROR_ACCESS_DENIED`.
- **Remediation**: Set `PRAGMA mmap_size = 0;` (disabling mmap) or cap at `8388608` (8 MB). Rely on SQLite's 4 MB page cache (`cache_size = -4000`).

#### PERF-03: Redundant DDL and Full-Table Count Queries on Every Search Call
- **Citation**: `crates/code-kb-cli/src/main.rs:424, 455`, `crates/code-kb-cli/src/mcp/server.rs:586, 636`, `crates/code-kb-core/src/db.rs:56-119`
- **Code**:
  In `main.rs` line 455 and `server.rs` line 636:
  ```rust
  let _ = ensure_fts_index_path(&self.db_path);
  let matches = match fts_search_symbols_scoped(&conn, query, ...);
  ```
  In `db.rs` lines 99-117:
  ```rust
  pub fn ensure_fts_index(conn: &Connection) -> Result<(), rusqlite::Error> {
      ...
      conn.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(...); ...")?;
      let symbol_count: i64 = conn.query_row("SELECT count(*) FROM symbols", [], |r| r.get(0)).unwrap_or(0);
      let docsize_count: i64 = conn.query_row("SELECT count(*) FROM symbols_fts_docsize", [], |r| r.get(0)).unwrap_or(0);
      if symbol_count > 0 && docsize_count == 0 {
          conn.execute("INSERT INTO symbols_fts(symbols_fts) VALUES('rebuild')", [])?;
      }
      Ok(())
  }
  ```
- **Impact**: Every search call opens a new read-write connection, executes 4 DDL statements, and runs a full table scan `SELECT count(*) FROM symbols`. This introduces 5–15ms of latency and write-lock contention against concurrent read connections.
- **Remediation**: Remove `ensure_fts_index_path` from search tool call sites. Run it strictly during `scan_workspace` and once at `McpServer` initialization.

#### PERF-04: Omission of `PRAGMA case_sensitive_like = ON` Disables Path Prefix Range Scans
- **Citation**: `crates/code-kb-core/src/db.rs:26-32`, `crates/code-kb-core/src/queries.rs:86, 147`
- **Query Plan Analysis**:
  ```text
  sqlite> EXPLAIN QUERY PLAN SELECT * FROM files WHERE path LIKE 'src/%';
  QUERY PLAN
  `--SCAN files

  sqlite> PRAGMA case_sensitive_like = ON;
  sqlite> EXPLAIN QUERY PLAN SELECT * FROM files WHERE path LIKE 'src/%';
  QUERY PLAN
  `--SEARCH files USING INDEX idx_files_path (path>? AND path<?)
  ```
- **Impact**: SQLite `LIKE` is case-insensitive by default, whereas `path` columns use `COLLATE BINARY`. SQLite cannot use the B-tree index `idx_files_path` for prefix matches unless `case_sensitive_like = ON` is configured.
- **Remediation**: Add `PRAGMA case_sensitive_like = ON;` to connection initialization in `open_read_only` and `open_read_write`.

#### PERF-05: Unbounded Heap Vector Allocation in `load_scoped_files`
- **Citation**: `crates/code-kb-core/src/queries.rs:77-111`, `crates/code-kb-cli/src/main.rs:386`
- **Code**:
  ```rust
  let files = stmt
      .query_map(..., |row| Ok(FileFact { ... }))?
      .collect::<Result<Vec<_>, _>>()?;
  ```
- **Impact**: In a 50,000–100,000 file workspace, allocating every `FileFact` (4 owned Strings, line counts, sizes) into a single heap vector consumes 17.5 MB to 35 MB of RAM, violating the < 15 MB retained memory constraint.
- **Remediation**: Stream records or paginate JSON output.

#### PERF-06: Omission of Depth Pushdown in `codebase_outline_op` Files Query
- **Citation**: `crates/code-kb-core/src/ops.rs:176-189`, `crates/code-kb-core/src/queries.rs:148`
- **Code**:
  In `codebase_outline_op`:
  ```rust
  "SELECT path FROM files WHERE (:path IS NULL OR path = :path OR path LIKE :path_prefix ESCAPE '\\') ORDER BY path ASC"
  ```
  Whereas `queries::load_scoped_outline_symbols` pushes down:
  ```sql
  AND (length(path) - length(replace(path, '/', '')) <= :max_slashes)
  ```
- **Impact**: `codebase_outline_op` queries all 100,000 files in the repo across all directory depths, allocates paths into strings, and streams them into `add_path_to_outline`, only to discard deep paths in memory.
- **Remediation**: Push down the slash-count depth filter into `codebase_outline_op`'s SQL query.

#### PERF-07: Read Connection Held Across Disk Writes and Subprocess in `replace_symbol_body`
- **Citation**: `crates/code-kb-core/src/edit.rs:73, 82, 85, 184`
- **Impact**: `conn: &Connection` is borrowed by `replace_symbol_body` and used only up to line 85 to resolve symbol offsets. The connection remains held open across file writing, syntax validation, and `sync::update_file` (which invokes `julie-extract update` in a child process). This induces `SQLITE_BUSY` lock contention.
- **Remediation**: Release or drop the read connection before initiating disk writes and child process synchronization.

#### PERF-08: Unsynchronized Background Reconciliation Threads in `McpServer`
- **Citation**: `crates/code-kb-cli/src/mcp/server.rs:39-44, 95-99`
- **Impact**: Detached background threads spawned in `McpServer::new` and `bind_workspace` call `reconcile_offline_edits`. If > 50 changes are discovered, `scan_workspace` is invoked, modifying SQLite tables while foreground MCP tool calls are actively executing queries.
- **Remediation**: Coordinate background sync with an `Arc<AtomicBool>` or mutex.

#### PERF-09: Missing Index on `literals(kind)` Leading to Full Table Scan
- **Citation**: `crates/code-kb-core/src/queries.rs:1225-1232`
- **Query Plan**: `EXPLAIN QUERY PLAN SELECT kind, COUNT(*) FROM literals GROUP BY kind ORDER BY cnt DESC` exhibits `SCAN literals` and two temporary B-trees (`USE TEMP B-TREE FOR GROUP BY`, `USE TEMP B-TREE FOR ORDER BY`).
- **Remediation**: Add `CREATE INDEX IF NOT EXISTS idx_literals_kind ON literals(kind);`.

#### PERF-10: Potential Fanout Explosion in Recursive CTE for Blast Radius
- **Citation**: `crates/code-kb-core/src/queries.rs:1381-1389`
- **Impact**: Branch 2 of `impact_walk` joins `pending_relationships` on `target_terminal_name = s_target.name`. If a target is a generic identifier (e.g. `new`, `run`, `get`), the CTE joins every unresolved call site in the repo, creating a massive intermediate working set before outer `LIMIT 200` truncation.
- **Remediation**: Apply a per-step depth limit or prune ubiquitous terminal identifiers.

---

### 4.2 Correctness & Robust Invariants

#### CORR-01: Old Database Leaks into Rebound Workspace When Server Started with `--db`
- **Citation**: `crates/code-kb-cli/src/mcp/server.rs:65-70`, `crates/code-kb-core/src/workspace.rs:369-373`
- **Code**:
  ```rust
  pub fn bind_workspace(&mut self, path: &Path) -> Result<(), WorkspaceError> {
      let ws = Workspace::discover(Some(path))?;
      let db_path = ws
          .locate_db(self.explicit_db.as_deref())
          .unwrap_or_else(|_| ws.canonical_root.join(".code-kb").join("artifact.db"));
  ```
- **Evidence**: When `code-kb serve --db /path/to/repoA/.code-kb/artifact.db` is run, `self.explicit_db` is stored. When an agent touches `repoB`, `bind_workspace` runs. `ws.locate_db(self.explicit_db.as_deref())` unconditionally returns `repoA`'s database. `self.workspace` points to `repoB`, but `self.db_path` points to `repoA`. Background reconciliation then deletes `repoA`'s symbols and attempts to index `repoB`'s files into `repoA`'s database.
- **Remediation**: Clear `self.explicit_db = None` on rebind if `self.workspace.canonical_root != ws.canonical_root`.

#### CORR-02: Worktree Fast-Path Copies SQLite Database Without WAL Checkpoint
- **Citation**: `crates/code-kb-cli/src/mcp/server.rs:443-465`
- **Code**:
  ```rust
  if parent_db.exists() {
      ...
      if std::fs::copy(&parent_db, &self.db_path).is_ok() {
          let _ = ensure_fts_index_path(&self.db_path);
          if let Ok(conn) = open_read_only(&self.db_path) {
              let _ = reconcile_offline_edits(&self.workspace, &self.db_path, &conn);
  ```
- **Evidence**: `std::fs::copy(&parent_db, &self.db_path)` copies ONLY the main DB file, ignoring `artifact.db-wal`. Dirty pages and recent commits stored in the WAL file are omitted, producing an outdated or corrupted worktree database.
- **Remediation**: Trigger a SQLite WAL checkpoint on `parent_db` prior to copying, or use `VACUUM INTO` / online backup.

#### CORR-03: Uncontrolled Detached Thread Spawning in `bind_workspace` When Watcher Fails
- **Citation**: `crates/code-kb-cli/src/mcp/server.rs:90-104`
- **Code**:
  ```rust
  if self.workspace.canonical_root != ws.canonical_root || self._watcher.is_none() {
      if db_path.exists() {
          ...
          std::thread::spawn(move || {
              if let Ok(conn) = open_read_only(&db_clone) {
                  let _ = reconcile_offline_edits(&ws_clone, &db_clone, &conn);
              }
          });
          self._watcher = start_watcher(ws.clone(), db_path.clone()).ok();
  ```
- **Evidence**: If `start_watcher` fails (e.g. Linux inotify `ENOSPC`), `self._watcher` remains `None`. Because `self._watcher.is_none()` is true, **every single subsequent tool call that inspects a path spawns a new thread executing `reconcile_offline_edits`**.
- **Remediation**: Remove `|| self._watcher.is_none()` from the re-trigger condition.

#### CORR-04: `relativize_filter` Bypasses `clean_path` for Relative Input
- **Citation**: `crates/code-kb-core/src/workspace.rs:315-348`
- **Code**:
  ```rust
  // Relative path: normalize slashes and trim leading ./ or /
  let forward = to_forward_slash(Path::new(&path_str));
  let trimmed = forward.trim_start_matches("./").trim_matches('/');
  if trimmed == "." { String::new() } else { trimmed.to_string() }
  ```
- **Evidence**: `clean_path` is never called on relative paths. Passing `"src/../src/lib.rs"` yields `"src/../src/lib.rs"`. Because the database stores canonical `"src/lib.rs"`, queries like `WHERE path = ?` fail to match, returning 0 results.
- **Remediation**: Pass relative paths through `clean_path` before slash normalization.

#### CORR-05: LIKE Wildcard Collision and Broken Exact Equality in `compute_blast_radius`
- **Citation**: `crates/code-kb-core/src/queries.rs:309, 405, 465, 1334-1342`
- **Code**:
  In `queries.rs:1334-1342`:
  ```rust
  path_conds.push(format!("path = ?{idx} OR path LIKE '%' || ?{idx} || '%' ESCAPE '\\'"));
  let raw = p.replace('\\', "/").trim_start_matches("./").trim_matches('/').to_string();
  let norm = escape_like(&raw);
  params_vec.push(rusqlite::types::Value::Text(norm));
  ```
- **Evidence**: `norm` escapes underscores (e.g. `"src/my_file.rs"` -> `"src/my\\_file.rs"`). The query uses `?{idx}` for BOTH `path = ?{idx}` AND `LIKE ... ESCAPE '\\'`. Comparing stored paths against `"src/my\\_file.rs"` always fails exact equality, forcing SQLite to fall back to a full table scan on the LIKE branch.
- **Remediation**: Separate the bound parameters: bind the raw string for `path = ?` and the escaped string for `LIKE ... ESCAPE '\\'`.

#### CORR-06: `replace_symbol_body` Fails to Snap UTF-8 Byte Boundaries on Write
- **Citation**: `crates/code-kb-core/src/edit.rs:107-146`
- **Code**:
  ```rust
  let mut new_file_bytes = Vec::with_capacity(existing_bytes.len() + normalized_body.len());
  new_file_bytes.extend_from_slice(&existing_bytes[..body_start]);
  new_file_bytes.extend_from_slice(normalized_body.as_bytes());
  new_file_bytes.extend_from_slice(&existing_bytes[body_end..]);
  let new_file_str = std::str::from_utf8(&new_file_bytes).map_err(...)?;
  ```
- **Evidence**: While `slice_bytes_safe` snaps offsets for reading, the reconstruction phase uses raw `body_start` and `body_end`. If an AST extractor emits offsets mid-codepoint, slicing `&existing_bytes[..body_start]` splits a multi-byte UTF-8 character, causing `from_utf8` to fail with `EditError::InvalidUtf8`.
- **Remediation**: Capture snapped character boundaries and use them when constructing `new_file_bytes`.

#### CORR-07: Windows Case-Sensitivity Mismatch in `resolve_path` for Non-Existent Paths
- **Citation**: `crates/code-kb-core/src/workspace.rs:248-270`
- **Evidence**: When `abs_path.exists()` is false, `effective_abs` skips `dunce::canonicalize`. `norm_root` has an uppercase drive letter (`C:\`), while `effective_abs` may have a lowercase drive letter (`c:\`). Rust's `Path::strip_prefix` is case-sensitive, returning `PathOutsideWorkspace` and falsely rejecting valid paths on Windows.
- **Remediation**: Perform case-insensitive component comparison on Windows when `strip_prefix` fails.

#### CORR-08: Upward Traversal Aborts on Hard-Excluded Subdirectories
- **Citation**: `crates/code-kb-core/src/workspace.rs:183-187, 209-213`
- **Evidence**: When discovering root from a file inside `target/debug/build/...` or `.agents/worker/...`, encountering `target` triggers `is_hard_excluded` and `break`s, misidentifying the excluded subdirectory as the project root.
- **Remediation**: Skip exclusion checks when traversing upward to discover workspace root.

#### CORR-09: Fragile Porcelain Git Status Line Parsing in `blast_radius_op`
- **Citation**: `crates/code-kb-core/src/ops.rs:271-285`
- **Evidence**: `line.trim()[2..]` relies on trimming leading whitespace and assuming offset 2 is the path start. Unstaged changes with leading spaces or quoted paths can be mis-sliced.
- **Remediation**: Use strict column indexing (`line.get(3..)`) without pre-trimming.

---

### 4.3 Dead Code, Redundancies & Missing Implementations

#### DEAD-01: Core Invariant 6 Contract Violation: `get_context_slice` Lacks `include_external`
- **Specification (`AGENTS.md` and `CLAUDE.md`, Section "Core Invariants §6"):**
  > "Language-agnostic callee filtering: `find_references(direction="callees")` and `get_context_slice` filter unresolved AST tokens against workspace symbols, eliminating external stdlib/runtime noise across all ~40 supported languages by default (`include_external: true` / `--include-external` restores them)."
- **Citations**:
  - `crates/code-kb-cli/src/mcp/server.rs:224-240` (schema omits `include_external`)
  - `crates/code-kb-cli/src/mcp/server.rs:681-709` (dispatch never extracts `include_external`)
  - `crates/code-kb-cli/src/main.rs:147-154` (`SliceArgs` lacks `--include-external`)
  - `crates/code-kb-core/src/ops.rs:103-136` (`get_context_slice_op` lacks parameter)
  - `crates/code-kb-core/src/queries.rs:1044-1133` (`find_callee_signatures` unconditionally filters against workspace symbols)
- **Remediation**: Add `include_external: bool` (default `false`) across tool schema, CLI `SliceArgs`, `get_context_slice_op`, and `find_callee_signatures`.

#### DEAD-02: Dynamic Workspace Rebind Failure Silently Runs on Stale Workspace
- **Citation**: `crates/code-kb-cli/src/mcp/server.rs:387-389, 404-409`
- **Evidence**: `let _ = self.bind_workspace(...)` discards errors. If rebinding fails, execution continues on the previous workspace without warning the client.
- **Remediation**: Check result and return `CallToolResult::error(...)` on rebind failure.

#### DEAD-03: `find_symbol` and `search_symbols` Lack `symbol` Parameter Alias
- **Citation**: `crates/code-kb-cli/src/mcp/server.rs:538-546, 608-616`
- **Evidence**: Handlers inspect `query`, `name`, `q`, but omit `symbol`. Calling `find_symbol({"symbol": "foo"})` fails with `Missing required parameter: query`.
- **Remediation**: Add `.or_else(|| arguments.get("symbol"))`.

#### DEAD-04 & DEAD-05: Swallowed Errors in Watcher and Background Threads
- **Citations**: `crates/code-kb-core/src/watcher.rs:125-131`, `crates/code-kb-cli/src/mcp/server.rs:40, 92, 455, 586, 636`
- **Evidence**: File update/delete failures in watcher and background FTS/reconcile failures are suppressed with `let _ =`.
- **Remediation**: Add explicit `warn!` logging.

#### DEAD-06: `format_file_skeleton` Redundantly Clones Symbol IDs
- **Citation**: `crates/code-kb-core/src/formatters.rs:25-31, 99`
- **Evidence**: `children_map` is typed as `HashMap<Option<String>, Vec<&Symbol>>`. In line 28, `s.parent_symbol_id.clone()` allocates a heap String for every symbol. In line 99, `sym.symbol_id.clone()` clones again during lookup.
- **Remediation**: Change key to `Option<&str>`.

#### DEAD-07: Unconditional Upward Directory Probes on Every Tool Call
- **Citation**: `crates/code-kb-cli/src/mcp/server.rs:390-410`
- **Evidence**: `Workspace::find_workspace_root(&abs_candidate)` runs on every tool call receiving a path, even when `abs_candidate.starts_with(&self.workspace.canonical_root)` is true.
- **Remediation**: Fast-path check `starts_with` before running directory traversal.

#### DEAD-08 to DEAD-10: Orphaned Error Variants
- `WorkspaceError::CanonicalizationFailed` (`workspace.rs:7`) and `WorkspaceError::DiscoveryFailed` (`workspace.rs:15`)
- `SyncError::Walk(ignore::Error)` (`sync.rs:22`)
- `WatcherError::Ignore(ignore::Error)` (`watcher.rs:17`)
- **Evidence**: None of these variants are ever constructed in production code.

#### DEAD-11: Dead Public Query Function `queries::find_references_for_symbol`
- **Citation**: `crates/code-kb-core/src/queries.rs:854-862`
- **Evidence**: Never called anywhere in `code-kb` except in its own unit test at line 1634. `find_references_ext` is used everywhere else.

#### DEAD-12 to DEAD-17: Ergonomics, CLI Asymmetries & Minor Redundancies
- **DEAD-12**: `find_references` lacks `dir` alias and direction case-folding (`server.rs:722`).
- **DEAD-13**: CLI parameter asymmetries: `symbol` is positional only in `blast-radius` and `edit`; missing MCP aliases (`main.rs:45-81`).
- **DEAD-14**: `code-kb hook` ignores global `--root` option (`main.rs:251-284`).
- **DEAD-15**: Unchecked byte slice `trimmed[2..]` on git porcelain status lines (`ops.rs:271-273`).
- **DEAD-16**: `get_symbol_by_name_internal` multi-clones full `Vec<Symbol>` structures (`queries.rs:732-780`).
- **DEAD-17**: Linear contains search on formatted String vec in `find_callee_signatures` (`queries.rs:1075-1083`).

---

### 4.4 Test Theater & Test Quality

#### TT-01: Fraudulent Resilience Test in `freshness_test.rs` (CRITICAL)
- **Citation**: `crates/code-kb-core/tests/freshness_test.rs:260-287`
- **Test Code**:
  ```rust
  #[test]
  fn test_reconcile_offline_edits_continues_when_individual_update_fails() {
      ...
      fs::write(src_dir.join("valid.rs"), "pub fn valid_symbol() {}\n").unwrap();
      ...
      scan_workspace(&ws, &db_path, true).unwrap();
      // Add another file
      fs::write(src_dir.join("another.rs"), "pub fn another_symbol() {}\n").unwrap();

      let report = reconcile_offline_edits(&ws, &db_path, &conn).expect("reconciliation should succeed");
      assert!(report.added.contains(&"src/another.rs".to_string()));
      assert!(get_symbol_by_name(&conn, "another_symbol", Some("src/another.rs")).unwrap().is_some());
  }
  ```
- **Evidence**: The test claims to verify that `reconcile_offline_edits` continues when an individual file update fails. In reality, **both `valid.rs` and `another.rs` are completely valid files**. Zero failure conditions are simulated. The error recovery logic is 100% unverified.
- **Remediation**: Create a third file that cannot be read or causes an extraction failure, and verify that `another.rs` is still successfully indexed.

#### TT-02: 33 of 33 Integration Tests in `code-kb-core/tests/` Bypass `safe_tempdir()`
- **Citations**:
  - `blast_radius_test.rs:38, 73, 125` (3 tests)
  - `disambiguation_test.rs:12, 71, 100, 141, 176, 218, 261` (7 tests)
  - `edit_test.rs:14, 72, 118, 167, 221, 267, 317, 373` (8 tests)
  - `freshness_test.rs:14, 52, 80, 104, 140, 186, 229, 264, 293, 355` (10 tests)
  - `watcher_test.rs:14, 98, 131, 180` (4 tests)
  - `worktree_test.rs:13` (1 test)
- **Evidence**: `safe_tempdir()` was implemented in `code-kb-core/src/lib.rs:56-72` to honor `TMPDIR` and `CARGO_TARGET_TMPDIR`, preventing Linux tmpfs quota crashes (`EDQUOT`). While adopted in `cli_test.rs` and `mcp_test.rs`, **all 33 integration tests in `code-kb-core/tests/` directly call `tempfile::tempdir().unwrap()`**, bypassing tmpfs protection.
- **Remediation**: Change all 33 calls to `code_kb_core::safe_tempdir()`.

#### TT-03: Child Subprocess Leakage on Assertion Panics in `mcp_test.rs`
- **Citation**: `crates/code-kb-cli/tests/mcp_test.rs:76, 427, 637, 826`
- **Evidence**: In `test_mcp_stdio_handshake_and_tools` and 3 other tests, `child.wait()` is called at the end of the test. If an assertion panics mid-test, the process unwinds and leaves `code-kb serve` running in the background, locking files and consuming RAM.
- **Remediation**: Implement an RAII guard struct implementing `Drop` that kills and reaps the child process.

#### TT-04: Vacuous Tautological Disjunctions in `cli_test.rs` and `mcp_test.rs`
- **Citations**: `cli_test.rs:343`, `mcp_test.rs:256-260`, `mcp_test.rs:285-288`
- **Evidence**:
  - `assert!(stdout.contains("No structural facts") || stdout.contains("Available"));`
  - `assert!(refs_text.contains("Callers") || refs_text.contains("No callers found") || refs_text.contains("Workspace"));`
  - `assert!(facts_text.contains("Available structural fact categories") || facts_text.contains("No structural facts"));`
- **Impact**: Tests accept either expected data or empty/failure outputs, hiding breakages.
- **Remediation**: Seed test databases with known facts and callers; assert exact matching outputs.

#### TT-05: 4 of 10 MCP Tools Never Exercised via `tools/call` in Integration Tests
- **Citation**: `crates/code-kb-cli/tests/mcp_test.rs:145-340`
- **Evidence**: While `tools/list` checks all 10 tools, `tools/call` only tests 6 tools. `codebase_outline`, `get_symbol_body`, `get_context_slice`, and `replace_symbol_body` are never called over JSON-RPC.
- **Remediation**: Add `tools/call` test blocks for all 4 tools in `mcp_test.rs`.

#### TT-06: Zero Test Coverage for Watcher Git Storm Circuit Breaker
- **Citation**: `crates/code-kb-core/src/watcher.rs:101-113`
- **Evidence**: The circuit breaker cancels micro-updates and runs a bulk scan when >50 files change. No automated test ever triggers >50 changes.
- **Remediation**: Add a burst test creating 55 files to confirm bulk scan execution.

#### TT-07: Untested Atomic Edit Rollback & Concurrency Failure Paths
- **Citation**: `crates/code-kb-core/src/edit.rs:151-156, 184-217`
- **Evidence**: `EditError::SymbolNotFound`, `EditError::NoBodyDefined`, `EditError::ConcurrentModification`, and `EditError::SyncWithRollback` have zero tests. All 8 tests in `edit_test.rs` test only Rust files; non-Rust languages are untested.
- **Remediation**: Add negative test cases verifying that failed indexing restores original disk bytes.

#### TT-08 to TT-15: Additional Test Quality Deficiencies
- **TT-08**: Excessive sleep polling (25 iterations * 200ms) and fixed 500ms sleep in `watcher_test.rs:44, 240`.
- **TT-09**: Weakened assertion in `disambiguation_test.rs:298`: `any(|t| ... || t.is_test)` accepts any test symbol.
- **TT-10**: `slicer.rs::test_slice_bytes_safe` only tests ASCII slicing from `0..len`, bypassing boundary snapping.
- **TT-11**: Ad-hoc mock DDL duplicated across 9 test suites bypasses schema v7.
- **TT-12**: Zero integration tests for MCP parameter validation errors and unknown tool names.
- **TT-13**: Untested CLI commands (`code-kb logs`, missing DB exit code, 7 untested `--json` commands).
- **TT-14**: Shallow `.is_err()` assertions in `syntax.rs:149-195`.
- **TT-15**: Untested discovery markers (`package.json`, `go.mod`) and hard-excluded directories in `workspace.rs`.

---

### 4.5 Documentation & Contract Parity

#### 4.5.1 Byte-for-Byte Synchronization Contracts
- **`AGENTS.md` vs `CLAUDE.md`**:
  - `cmp AGENTS.md CLAUDE.md` exit code: `0`.
  - SHA-256 Checksum: `f77d16a71f8949c0dbd2b1d839e01fbfb842eaddd6fc08d64ed4862159aa4523`.
  - Exact Line and Byte Count: 100 lines, 6,436 bytes each.
  - Enforced by: `crates/code-kb-cli/tests/cli_test.rs:500-509`, `scripts/release-preflight.sh:18-25`, `.github/workflows/ci.yml:28-30`.
- **`SKILL.md` Copies**:
  - `cmp skills/code-kb/SKILL.md .claude-plugin/skills/code-kb/SKILL.md` exit code: `0`.
  - SHA-256 Checksum: `0fcaf71971222c6ecb0fcfaafb5f2e65a8531babee39111c18192e08b677399d`.
  - Enforced by: `crates/code-kb-cli/tests/cli_test.rs:512-522`, `scripts/release-preflight.sh:26-31`.

#### 4.5.2 1:1 CLI Command & MCP Tool Schema Parity
Every MCP tool returned in `tools/list` has an exact 1:1 CLI command counterpart executing identical backend operations:

| # | MCP Tool Name | CLI Command | Shared Core Operation | Key Parameters (MCP / CLI) | Status |
|---|---|---|---|---|:---:|
| 1 | `codebase_outline` | `code-kb outline` | `code_kb_core::codebase_outline_op` | `path`, `depth` / `[path]`, `--depth` | 1:1 Match |
| 2 | `file_skeleton` | `code-kb skeleton` | `code_kb_core::format_file_skeleton` | `file_path` / `<file>` | 1:1 Match |
| 3 | `find_symbol` | `code-kb symbol` | `code_kb_core::search_symbols_scoped` | `query`, `path`, `kind`, `is_test`, `limit` | 1:1 Match |
| 4 | `search_symbols` | `code-kb search` | `code_kb_core::fts_search_symbols_scoped` | `query`, `path`, `kind`, `is_test`, `limit` | 1:1 Match |
| 5 | `get_symbol_body` | `code-kb body` | `code_kb_core::get_symbol_body_op` | `symbol_name`, `file_path` / `<symbol>`, `--file` | 1:1 Match |
| 6 | `get_context_slice` | `code-kb slice` | `code_kb_core::get_context_slice_op` | `symbol_name`, `file_path` / `<symbol>`, `--file` | 1:1 Match |
| 7 | `find_references` | `code-kb refs` | `code_kb_core::find_references_ext` | `symbol_name`, `direction`, `include_external` | 1:1 Match |
| 8 | `find_structural_facts` | `code-kb facts` | `code_kb_core::find_structural_facts` | `category`, `limit` / `[category]`, `--limit` | 1:1 Match |
| 9 | `blast_radius` (`impact`) | `code-kb blast-radius` (`impact`) | `code_kb_core::blast_radius_op` | `symbol`, `file`, `depth`, `limit` | 1:1 Match |
| 10 | `replace_symbol_body` | `code-kb edit` | `code_kb_core::replace_symbol_body` | `symbol_name`, `file_path`, `new_body`, `expected_body_hash` | 1:1 Match |

Non-tool operational CLI subcommands: `scan`, `logs`, `stats`/`telemetry`, `serve`, and `hook`.

#### 4.5.3 Documentation Discrepancies & Tooling Enhancements
- **DOC-01 (`README.md:198`)**: In the Tool Catalog table, the `search_symbols` row lists parameters as `query (req), path (opt), kind (opt), limit (opt)`, omitting `is_test (opt)`. The schema and CLI both support `is_test`.
- **DOC-02 (`routing-block.md` Sync)**: `crates/code-kb-cli/src/routing-block.md` and `hooks/code-kb-routing-block.md` are identical (22 lines), but lack an automated sync test in `cli_test.rs` or `release-preflight.sh`.
- **DOC-03 (`release-preflight.sh` Scope)**: Step `[2/7]` of `release-preflight.sh` checks `Cargo.toml`, `code-kb-cli/Cargo.toml`, and `plugin.json`, but does not verify `.github/workflows/release-binaries.yml` default version or `docs/site/index.html` badges.
- **DOC-04 (`Manifest Version Test`)**: Version consistency across manifests is checked only in bash, not in `cargo test`.

---

### 4.6 Pre-Release v0.5.2 Version Bump Protocol

When cutting version `0.5.2`, all 7 locations must be bumped in the same atomic commit to maintain pre-flight script integrity:

1. **Root `Cargo.toml:9`**:
   ```toml
   [workspace.package]
   version = "0.5.2"
   ```
2. **`crates/code-kb-cli/Cargo.toml:18`**:
   ```toml
   code-kb-core = { version = "0.5.2", path = "../code-kb-core" }
   ```
3. **`.claude-plugin/plugin.json:3`**:
   ```json
   "version": "0.5.2",
   ```
4. **`.github/workflows/release-binaries.yml:9`**:
   ```yaml
   default: "0.5.2"
   ```
5. **Update `Cargo.lock`**:
   ```bash
   cargo check --workspace --all-targets
   ```
6. **`docs/site/index.html`**:
   - Line 19: `<span class="brand-badge">v0.5.2</span>`
   - Line 37: `<span>v0.5.2 Released &bull; First-Class Windows, Linux &amp; macOS Support</span>`
   - Line 256: `<th class="col-featured">code-kb (v0.5.2)</th>`
7. **`TODO.md`**:
   - Lines 10-12: Update crates.io publish targets to `v0.5.2`.

---

## 5. Verification & Reproducibility Section

All findings in this report can be independently reproduced using the following commands and procedures:

### 5.1 Baseline Verification Commands
```bash
# 1. Compilation
cargo check --all-targets

# 2. Strict Clippy
cargo clippy --all-targets -- -D warnings

# 3. Formatting
cargo fmt --all -- --check

# 4. Full test suite with quota-safe tempdir
mkdir -p target/tmp
TMPDIR=target/tmp cargo test --all-targets

# 5. Full release preflight script
bash scripts/release-preflight.sh
```

### 5.2 Independent Verification of High-Priority Findings

#### Reproducing PERF-01 (Symbol Search Leading-Wildcard Scan)
```bash
# Generate a test database using pinned julie-extract
TMPDIR=target/tmp ./.tools/julie-extract scan --root crates/code-kb-core/src --db target/tmp/audit_test.db

# Verify leading wildcard forces full table scan
sqlite3 target/tmp/audit_test.db "EXPLAIN QUERY PLAN SELECT symbol_id FROM symbols WHERE (name = 'test' OR name LIKE '%test%' ESCAPE '\\') AND is_test = 0 AND test_container = 0 ORDER BY (name = 'test') DESC, length(name) ASC, path ASC LIMIT 50;"
# Output will confirm: SEARCH symbols USING INDEX idx_symbols_test_container or SCAN symbols
```

#### Reproducing PERF-04 (LIKE Path Prefix Scan without case_sensitive_like)
```bash
# Without case_sensitive_like (default):
sqlite3 target/tmp/audit_test.db "EXPLAIN QUERY PLAN SELECT * FROM files WHERE path LIKE 'src/%';"
# Output: SCAN files

# With case_sensitive_like enabled:
sqlite3 target/tmp/audit_test.db "PRAGMA case_sensitive_like = ON; EXPLAIN QUERY PLAN SELECT * FROM files WHERE path LIKE 'src/%';"
# Output: SEARCH files USING INDEX idx_files_path (path>? AND path<?)
```

#### Reproducing TT-01 (Fraudulent Resilience Test)
Inspect `crates/code-kb-core/tests/freshness_test.rs:260-287`. Confirm that both `valid.rs` and `another.rs` contain valid Rust syntax and that no error condition or failure is ever introduced during the test run.

#### Reproducing TT-02 (safe_tempdir Bypass in Core Integration Tests)
```bash
# Count occurrences of direct tempdir allocation in core integration tests
git grep -E "tempfile::tempdir\(\)|tempdir\(\)\.unwrap\(\)" crates/code-kb-core/tests/
# Exactly 33 matching lines across 6 integration test files (including the 3 in blast_radius_test.rs which import use tempfile::tempdir)
```

#### Reproducing TT-05 (Omitted MCP Tools in tools/call Integration Tests)
```bash
# Verify that tools/call in mcp_test.rs never invokes the 4 missing tools
git grep -E "name\": \"(replace_symbol_body|get_context_slice|get_symbol_body|codebase_outline)\"" crates/code-kb-cli/tests/mcp_test.rs
# Zero matches under tools/call (only present in tools/list check at lines 152-161)
```

#### Reproducing DEAD-01 (get_context_slice Missing include_external)
```bash
# Tool schema check:
grep -n -A 20 '"get_context_slice"' crates/code-kb-cli/src/mcp/server.rs
# CLI Args check:
grep -n -A 10 'struct SliceArgs' crates/code-kb-cli/src/main.rs
# Ops signature check:
grep -n 'pub fn get_context_slice_op' crates/code-kb-core/src/ops.rs
# None accept include_external, violating Core Invariant 6.
```

#### Reproducing CORR-01 (Old DB Leak on Rebind with `--db`)
Inspect `crates/code-kb-cli/src/mcp/server.rs:68` and `crates/code-kb-core/src/workspace.rs:370`. Note that `self.explicit_db` is stored as an `Option<PathBuf>` and never cleared when `bind_workspace` is called for a new workspace root.

---

## 6. Conclusion & Release Gate Assessment

1. **Compilation, Lints & Baseline Tests**: **100% GREEN**. The codebase is in a stable, passing state with 85/85 tests green and 0 clippy warnings under `-D warnings`.
2. **Contract Compliance**: `AGENTS.md` and `CLAUDE.md` match byte-for-byte; Core Invariant 1 (zero workspace parameters) and Core Invariant 7 (pinned extractor `2.42.1`) are strictly enforced.
3. **Audit Findings Tracked**: 55 actionable items have been cataloged with concrete file:line citations and reproduction procedures.
4. **Pre-Release Audit Gate Verdict: BLOCKED**: Cutting the `v0.5.2` release tag is **BLOCKED** until the following 4 high-priority blockers are remediated:
   1. **TT-01 (Integrity Violation / Facade Resilience Test)**: Repair the facade resilience test in `crates/code-kb-core/tests/freshness_test.rs:260-287` (`test_reconcile_offline_edits_continues_when_individual_update_fails`) to inject a genuine failure condition (e.g. unreadable file or unparseable fixture causing `update_file` to error) alongside valid files and verify fault tolerance.
   2. **DEAD-01 (Core Invariant 6 Contract Violation)**: Implement `include_external: bool` (default `false`) in `get_context_slice` across MCP tool schema, MCP dispatch handler, CLI `SliceArgs`, `get_context_slice_op`, and `find_callee_signatures` to satisfy Core Invariant 6.
   3. **TT-02 (safe_tempdir Quota Bypass)**: Replace all 33 `tempfile::tempdir()` and `tempdir().unwrap()` calls in core integration tests (`crates/code-kb-core/tests/`) with `code_kb_core::safe_tempdir()` to prevent tmpfs quota exhaustion.
   4. **TT-03 (Child Process Resource Leak)**: Add RAII drop guards to child process management in `crates/code-kb-cli/tests/mcp_test.rs` to guarantee orphaned server processes are terminated on test assertion failure or panic.
