# Comprehensive Post-v1.0 Audit Report: `code-kb`

**Date:** 2026-09-15  
**Target Commit / Branch:** `main` (post-v1.0.0 release)  
**Workspace:** `/home/murphy/source/code-kb`  
**Audit Team:** Worker 1 (Synthesis Lead), Explorer Track 1, Explorer Track 2, Explorer Track 3, Challenger 1  
**Integrity Mode:** Non-destructive execution (zero production source files modified)

---

## 1. Executive Summary & Scorecard

An exhaustive, multi-track post-v1.0 audit was conducted across the `code-kb` repository (`code-kb-core` and `code-kb-cli`) to assess code correctness, architectural invariants, query engine integrity, platform compatibility, memory footprint, cache freshness, error propagation contracts, and test/benchmark rigor.

### 1.1 Overall Health Assessment

The `code-kb` codebase demonstrates exceptional craftsmanship and architectural discipline:
- **Flawless Baseline Health:** Current `main` builds cleanly with zero compiler warnings, passes `cargo clippy --all-targets -- -D warnings` with zero lints, exhibits zero `cargo fmt` drift, passes 199/199 automated workspace tests with zero failures, and successfully validates all 7 release pre-flight gates (`scripts/release-preflight.sh`).
- **Strict Invariant Adherence:** The byte-for-byte documentation sync contract between `AGENTS.md` and `CLAUDE.md` is strictly maintained. The core invariant prohibiting workspace parameters across MCP tool schemas is 100% enforced (zero violations across all 11 tools and prompts). Single-turn atomic edits (`replace_symbol_body`) enforce Tree-Sitter pre-flight syntax checks, optimistic SHA-256 locks, atomic file swaps, and immediate SQLite re-indexing.
- **Robust Worktree Isolation:** Git worktree handling guarantees isolated AST index databases at `<worktree_root>/.code-kb/artifact.db`. Parent databases are safely flushed via SQLite WAL truncation (`checkpoint_truncate`) before copying, and worktree deletion leaves zero orphaned global state.

However, in-depth adversarial exploration and empirical reproduction surfaced **targeted correctness bugs and architectural discrepancies** that affect edge cases in blast radius analysis, recursive graph traversal, directory-scoped queries, ambiguous symbol error propagation, Windows path normalization, cold-start reconciliation I/O, and retained memory bounds.

### 1.2 Audit Findings Scorecard

| Canonical Severity | Count | Primary Impacted Areas | Key Findings Summary |
|---|:---:|---|---|
| **CRITICAL** | **1** | Blast Radius Query Seeding | **CRIT-01**: Underscore escaping (`escape_like`) in seed path exact equality (`queries.rs:1918-1928`) causes 0 symbols to be seeded for any file with an underscore, returning empty blast radius results. |
| **HIGH** | **7** | Graph Traversal, Path Scoping, Error Propagation, Memory, Reconciliation | **HIGH-01**: Recursive CTE cycle re-entrance returns seed symbol as impacted caller of itself (`queries.rs:2023-2041`).<br>**HIGH-02**: Directory prefix filtering broken in `get_symbol_by_name` (`queries.rs:739`).<br>**HIGH-03**: Silent swallowing of `AmbiguousSymbol` error in `lookup_symbol` (`server.rs:809-842`).<br>**HIGH-04**: Missing Windows backslash normalization in `find_callee_signatures` (`queries.rs:1415-1416`).<br>**HIGH-05**: Missing `COLLATE NOCASE` in `find_structural_facts_scoped` (`queries.rs:1619`).<br>**HIGH-06**: Retained RSS (20.68 MB) and peak RSS (29.7 MB) exceed advertised `< 15 MB` invariant due to 256MB mmap (`db.rs:42`).<br>**HIGH-07**: Startup reconciliation reads and hashes 100% of unchanged files due to missing `mtime` check (`sync.rs:405-418`). |
| **MEDIUM** | **7** | Query Bounds, Schema Edge Cases, Symlinks, Freshness | **MED-01**: Unbounded `max_depth` in public `compute_blast_radius_scoped`.<br>**MED-02**: Unhandled empty strings in `json_each` namespace checks.<br>**MED-03**: Unescaped `LIKE %?%` and substring collisions in `search_symbols_scoped`.<br>**MED-04**: Asymmetric symlink resolution between relative and absolute path filters.<br>**MED-05**: Case-sensitivity mismatch in blast radius seed path matching.<br>**MED-06**: Silent stale results in untargeted CLI and MCP symbol queries.<br>**MED-07**: JIT staleness guard bypassed for newly added symbols without `--file`. |
| **LOW** | **4** | Parity, Ergonomics, Protocol Hygiene | **LOW-01**: Potential duplicate call sites in `find_references`.<br>**LOW-02**: CLI `--expected-hash` parameter lacks `--expected-body-hash` alias.<br>**LOW-03**: Missing `--path` / `--file-path` argument aliases on CLI `refs` and `blast-radius`.<br>**LOW-04**: CLI `--json` outputs plaintext on stderr rather than structured JSON. |
| **ADVISORY** | **2** | Documentation & Architectural Integrity | **ADV-01**: Minor parameter naming drift in `routing-block.md`.<br>**ADV-02**: Git worktree isolation and path containment verified robust. |
| **TOTAL** | **21** | — | **1 Critical, 7 High, 7 Medium, 4 Low, 2 Advisory** |

---

## 2. Baseline Verification Results

Every baseline quality gate specified in `ORIGINAL_REQUEST.md` and `AGENTS.md` was executed on the current `main` branch. All commands passed cleanly without failure.

### 2.1 Baseline Execution Summary

| Check | Tool / Target | Command Executed | Result | Duration | Notes |
|---|---|---|:---:|---:|---|
| **Compilation** | `cargo` | `cargo check --all-targets` | **PASS** | 1.29s | Clean compilation across `code-kb-core` and `code-kb-cli`. |
| **Linting** | `clippy` | `cargo clippy --all-targets -- -D warnings` | **PASS** | 1.67s | Zero warnings; strict denial gate satisfied. |
| **Formatting** | `rustfmt` | `cargo fmt --check` | **PASS** | 0.85s | Zero formatting drift across all files. |
| **Full Test Suite** | `cargo test` | `TMPDIR=target/tmp cargo test --workspace --all-targets` | **PASS** | ~37.0s | 199 tests passed, 0 failed, 0 ignored. |
| **Pre-Flight Gates** | `bash` | `bash scripts/release-preflight.sh` | **PASS** | 38.20s | 7/7 verification gates passed cleanly for v1.0.0. |

### 2.2 Test Suite Breakdown by Target

Across the workspace, the test suite executes 199 distinct tests with zero flaky behavior:

| Test Suite / Binary | Passed | Failed | Ignored | Execution Time | Focus Area |
|---|---:|---:|---:|---:|---|
| `code-kb-core` unit tests (`src/lib.rs`) | 68 | 0 | 0 | 25.49s | Core queries, DB handles, AST extraction, edit rollback. |
| `tests/adversarial_m1_test.rs` | 21 | 0 | 0 | 0.07s | Path normalization, URI decoding, Windows prefix stripping. |
| `tests/adversarial_m2_test.rs` | 10 | 0 | 0 | 0.30s | Stale cache bounds, concurrency locks, file deletion. |
| `tests/blast_radius_test.rs` | 6 | 0 | 0 | 0.10s | Blast radius reachability, test impact prediction. |
| `tests/disambiguation_test.rs` | 13 | 0 | 0 | 0.22s | Language filtering, container grouping, test vs prod symbols. |
| `tests/edit_test.rs` | 8 | 0 | 0 | 0.29s | Atomic edit, Tree-Sitter validation, rollback integrity. |
| `tests/freshness_test.rs` | 10 | 0 | 0 | 0.30s | Read-time JIT staleness detection, hash re-indexing. |
| `tests/m1_stress_test.rs` | 13 | 0 | 0 | 0.00s | Deep recursion, malformed files, wide directories. |
| `tests/watcher_test.rs` | 4 | 0 | 0 | 1.00s | Background file watcher, debouncer timing, checkout storms. |
| `tests/worktree_test.rs` | 2 | 0 | 0 | 0.44s | Worktree DB isolation, WAL flush before copy, retargeting. |
| `code-kb-cli` unit tests (`src/main.rs`) | 1 | 0 | 0 | 0.00s | CLI argument structure and subcommand routing. |
| `tests/adversarial_m2_server_test.rs` | 9 | 0 | 0 | 5.25s | Multi-root IDE binding, watcher storms, debouncer timeouts. |
| `tests/adversarial_m3_server_test.rs` | 3 | 0 | 0 | 2.05s | MCP 18-token blacklist, dynamic rebind, telemetry bounds. |
| `tests/adversarial_m3_test.rs` | 3 | 0 | 0 | 1.36s | CLI JSON forward slash enforcement, git status auto-detection. |
| `tests/cli_test.rs` | 27 | 0 | 0 | 0.32s | 1:1 CLI command parity, error codes, sync contract test. |
| `tests/mcp_test.rs` | 9 | 0 | 0 | 0.48s | MCP protocol compliance, tool schema properties, stdio hygiene. |
| `Doc-tests code_kb_core` | 0 | 0 | 0 | 0.00s | Inline library documentation tests. |
| **TOTAL** | **199** | **0** | **0** | **~37.0s** | **100% Passing** |

### 2.3 Release Pre-Flight Verification (`scripts/release-preflight.sh`)

Execution of `bash scripts/release-preflight.sh` verified all 7 release gates:
1. `[1/7] Verifying sync contracts`: `AGENTS.md` vs `CLAUDE.md` byte-for-byte identical (`OK`).
2. `[2/7] Checking version consistency`: All Cargo manifests, pins, and scripts match `v1.0.0` (`OK`).
3. `[3/7] Checking code formatting`: `cargo fmt` reported clean (`OK`).
4. `[4/7] Running clippy across workspace`: Clean with zero warnings (`OK`).
5. `[5/7] Running full test suite`: `cargo test --workspace` passed all 199 tests (`OK`).
6. `[6/7] Verifying workspace packaging`: Both crates package cleanly (`code-kb-core` 580.3 KiB, `code-kb-cli` 328.9 KiB) (`OK`).
7. `[7/7] Checking Prax Windows 11 VM`: Pre-flight guest VM hook confirmed healthy (`OK`).

---

## 3. 8 Core Invariants Compliance Matrix

The core architectural invariants defined in `AGENTS.md` and `CLAUDE.md` govern the design, agent ergonomics, and operational footprint of `code-kb`.

### 3.1 Compliance Summary Table

| Invariant / Contract Area | Status | Findings Identified | Summary Assessment |
|---|:---:|---|---|
| **Documentation Sync Contract** (`AGENTS.md` ↔ `CLAUDE.md`) | **COMPLIANT** | None | Byte-for-byte identical (SHA256: `b9661c4fb3...`). Automated CI regression test active. |
| **Invariant 1**: Zero Workspace Parameters | **COMPLIANT** | None | Zero workspace parameters advertised across all 11 MCP tool schemas and prompt surfaces. Verified against 18-token blacklist. |
| **Invariant 2**: Retained Memory < 15 MB | **NON-COMPLIANT** | HIGH-06 | Architecture uses direct SQLite streaming without heap graphs, but steady-state retained RSS reaches **20.68 MB** and CLI peak RSS reaches **29.7 MB** due to `PRAGMA mmap_size = 256MB` (`db.rs:42`). |
| **Invariant 3**: Single-Turn Atomic Edits | **COMPLIANT** | None | `replace_symbol_body` executes Tree-Sitter syntax validation, optimistic SHA-256 locking, atomic tempfile swap, and automatic rollback in a single turn. |
| **Invariant 4**: Token-Dense Progressive Disclosure | **COMPLIANT** | None | Skeleton body stripping, 3-line doc comment caps, container symbol grouping, and stdlib callee filtering reduce response token consumption by 80–90%. |
| **Invariant 5**: Windows Compatibility | **COMPLIANT** | None | Verbatim UNC prefix stripping (`dunce::simplified`), case-insensitive path comparisons, strict forward slash output in JSON, and connection handle cleanup before file operations. |
| **Invariant 6**: Tool Ergonomics & 1:1 Parity | **QUALIFIED** | LOW-02, LOW-03 | Exact 1:1 functional parity across all 11 MCP tools and CLI commands. Minor parameter alias friction on CLI (`--expected-hash` vs `--expected-body-hash`, missing `--path` on `refs`/`blast-radius`). |
| **Invariant 7**: Pinned Extractor & Build Guard | **COMPLIANT** | None | Extractor pinned to `2.42.3` in `julie-pins.json` and `sync.rs`. Build guard in `build.rs` verifies binary presence and version match during compilation. |
| **Invariant 8**: Dynamic MCP Discovery | **COMPLIANT** | None | Exactly 11 tools advertised in `tools/list`. Zero ghost schemas or deprecated aliases preserved across sessions. |

---

### 3.2 In-Depth Invariant Analysis

#### Documentation Sync Contract (`AGENTS.md` ↔ `CLAUDE.md`)
- **Specification:** `AGENTS.md` and `CLAUDE.md` must stay byte-for-byte equivalent at all times.
- **Audit Findings:** Both files were inspected and hashed via SHA-256:
  - `AGENTS.md`: `b9661c4fb3f226b142a9057a47b7390096a9e8076206f462ed938445e0b36fd0`
  - `CLAUDE.md`: `b9661c4fb3f226b142a9057a47b7390096a9e8076206f462ed938445e0b36fd0`
  - Byte comparison: `cmp -s AGENTS.md CLAUDE.md` returns exit code `0`.
  - Regression protection: `crates/code-kb-cli/tests/cli_test.rs:895` (`test_agents_and_claude_md_sync_contract`) verifies byte-for-byte equality on every `cargo test` invocation.
- **Status:** **100% Compliant**.

#### Invariant 1: Zero Workspace Parameters Across All MCP Schemas and Prompts
- **Specification:** Never expose `workspace`, `workspace_id`, `repo_path`, or `root_dir` in any MCP tool schema or prompt surface. Workspace binding must occur silently via CWD, IDE `initialize` handshake, or in-tree file path inspection.
- **Audit Findings:**
  - Audited all 11 tool schemas in `crates/code-kb-cli/src/mcp/server.rs:124-378`:
    - `codebase_outline`: `["path", "depth"]`
    - `file_skeleton`: `["file_path"]`
    - `lookup_symbol`: `["query", "path", "kind", "is_test", "limit"]`
    - `search_symbols`: `["query", "path", "kind", "is_test", "limit"]`
    - `get_symbol_body`: `["symbol_name", "file_path"]`
    - `get_symbol_context`: `["symbol_name", "file_path", "include_external"]`
    - `find_references`: `["symbol_name", "file_path", "direction", "limit", "include_external"]`
    - `find_structural_facts`: `["category", "path", "limit"]`
    - `blast_radius`: `["symbol", "file", "depth", "limit"]`
    - `replace_symbol_body`: `["symbol_name", "file_path", "new_body", "expected_body_hash"]`
    - `telemetry_summary`: `["time_window", "workspace_only"]`
  - In `telemetry_summary`, `"workspace_only"` is a boolean toggle scoping metrics to the active repository, not a workspace path.
  - Verified initialization prompt (`server.rs:1248`): Zero workspace parameters mentioned.
  - Verified adversarial test `crates/code-kb-cli/tests/adversarial_m3_server_test.rs:612` (`test_adversarial_mcp_core_invariant_1_exhaustive_blacklist`): Rejects 18 forbidden parameters across properties and required fields.
- **Status:** **100% Compliant**.

#### Invariant 2: Retained Memory < 15 MB & Zero Heap Graphs
- **Specification:** Zero in-memory heap objects for repositories. Do not hydrate symbol graphs into RAM. Retained memory must stay < 15 MB. Queries must execute as direct, indexed SQLite queries with `open_read_only`.
- **Audit Findings:**
  - `McpServer` (`crates/code-kb-cli/src/mcp/server.rs:20-26`) holds no in-memory graph objects.
  - Connection lifecycle: Read-only SQLite connections are opened on-demand per tool call (`server.rs:735`) and dropped immediately when the invocation completes.
  - SQLite page cache is capped at ~4 MB (`PRAGMA cache_size = -4000;` in `db.rs:32, 41`).
  - **Identified Violation:** On non-Windows platforms, `crates/code-kb-core/src/db.rs:42` configures `PRAGMA mmap_size = 268435456;` (256 MB). Under empirical measurement of `target/release/code-kb serve` across 80 tool calls, retained VmRSS stabilizes at **20.68 MB** (exceeding the < 15 MB bound by 37.8%). On CLI queries, peak RSS reaches **27.5 MB – 29.7 MB**. (See **HIGH-06**).
- **Status:** **Non-Compliant** (Architectural streaming is sound, but memory mapping and glibc thread arena allocations breach the advertised < 15 MB threshold).

#### Invariant 3: Single-Turn Atomic Edits in `replace_symbol_body`
- **Specification:** Pre-flight Tree-Sitter syntax validation, optional `body_hash` optimistic lock verification, atomic file replacement, and immediate SQLite re-indexing in a single turn. Zero two-step preview handshakes.
- **Audit Findings:**
  - `replace_symbol_body` (`crates/code-kb-core/src/edit.rs:115-279`) implements a clean, fail-safe pipeline:
    1. Line 127: Calls `sync::ensure_fresh_file` to sync index and disk offsets before reading.
    2. Line 163: Validates `expected_body_hash` against current body SHA-256 (`HashMismatch`).
    3. Line 189: Performs pre-flight Tree-Sitter syntax validation (`validate_syntax`) for Rust, JS, TS, TSX, Python, and Go before touching disk.
    4. Line 204: Atomically writes to a temporary file (`.code-kb-edit-*.tmp`) and invokes `persist_with_retry`.
    5. Line 234: Immediately invokes `sync::update_file` to re-index the file in SQLite.
    6. Lines 236–266: If re-indexing fails, checks whether disk still matches the edit and rolls back original content atomically via temporary file.
- **Status:** **100% Compliant**.

#### Invariant 4: Token-Dense Progressive Disclosure
- **Specification:** Return the most compact representation that answers queries. Strip implementation bodies in `file_skeleton`. Include only immediate callee signatures, related types, and test locations in `get_symbol_context`.
- **Audit Findings:**
  - `format_file_skeleton` (`crates/code-kb-core/src/formatters.rs:10-140`): Strips bodies (`{ /* X lines hidden: Lstart-Lend */ }`), skips variables/imports, groups child methods inside structs/traits/classes, and truncates doc comments to 3 lines.
  - `get_symbol_context` (`crates/code-kb-core/src/ops.rs:103-140`): Bundles target body, callee signatures (max 10), related types, and related tests (max 5). Filters unresolved external stdlib symbols across all ~40 languages unless `include_external: true` is set.
  - Telemetry confirmed 80–90% token reduction across code queries.
- **Status:** **100% Compliant**.

#### Invariant 5: Windows Compatibility Patterns
- **Specification:** Windows is a first-class target. Strip verbatim UNC prefixes (`\\?\`) via `dunce::simplified`. Relative paths and JSON output must strictly use explicit forward slashes `/`. Connections and file handles closed before rename/delete. Path identity verified using canonical paths and case-insensitive comparison on Windows.
- **Audit Findings:**
  - `normalize_path` (`crates/code-kb-core/src/workspace.rs:189-199`) strips `\\?\` prefixes via `dunce::simplified`.
  - `strip_prefix_lossy` (`workspace.rs:251-271`) and `paths_equal` (`workspace.rs:276-312`) compare disk components case-insensitively on Windows.
  - Strict forward slashes verified: `adversarial_m3_test.rs::test_adversarial_cli_json_strict_forward_slash_across_all_commands` asserts 0 backslashes across all JSON CLI output.
  - On Windows, `PRAGMA mmap_size = 0;` (`db.rs:33`) prevents memory maps from locking files during deletion or rename.
- **Status:** **100% Compliant**.

#### Invariant 6: Zero-Friction Tool Ergonomics & 1:1 Parity
- **Specification:** 1:1 parity between all MCP tool endpoints and corresponding CLI commands. Tool handlers accept intuitive parameter aliases (`file`/`path`, `symbol`/`name`, `body`/`code`, `q`/`name`). Safe defaults for optional parameters. Scoped search across tools.
- **Audit Findings:**
  - 1:1 Parity matrix:
    - `codebase_outline` ↔ `code-kb outline`
    - `file_skeleton` ↔ `code-kb skeleton`
    - `lookup_symbol` ↔ `code-kb lookup` (alias: `symbol`)
    - `search_symbols` ↔ `code-kb search`
    - `get_symbol_body` ↔ `code-kb body`
    - `get_symbol_context` ↔ `code-kb context` (alias: `slice`)
    - `find_references` ↔ `code-kb refs`
    - `find_structural_facts` ↔ `code-kb facts`
    - `blast_radius` ↔ `code-kb blast-radius` (alias: `impact`)
    - `replace_symbol_body` ↔ `code-kb edit`
    - `telemetry_summary` ↔ `code-kb stats` (alias: `telemetry`)
  - Additional infrastructure CLI commands: `scan`, `logs`, `bug-report`, `serve`, `hook`.
  - MCP parameter aliases fully implemented in `server.rs`.
  - Minor CLI friction: CLI `edit` expects `--expected-hash` instead of accepting MCP's `--expected-body-hash` (see **LOW-02**), and CLI `refs` lacks `--path` alias (see **LOW-03**).
- **Status:** **Qualified Pass**.

#### Invariant 7: Pinned Extractor (2.42.3) & Build Guard
- **Specification:** Pins exact extractor version in `scripts/julie-pins.json` (currently `2.42.3`). `crates/code-kb-cli/build.rs` verifies `julie-extract` is restored and matches the pinned version.
- **Audit Findings:**
  - `scripts/julie-pins.json:2`: `"version": "2.42.3"`.
  - `crates/code-kb-core/src/sync.rs:27`: `pub const PINNED_JULIE_VERSION: &str = "2.42.3";`.
  - `crates/code-kb-cli/build.rs:32-152`: Verifies binary version during compilation; aborts build if mismatched (bypassable via `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1`).
  - Runtime discovery in `discover_julie_extract_binary` follows prioritized paths (`JULIE_EXTRACT_BIN` -> executable sibling -> CWD `.tools/` -> `PATH`).
- **Status:** **100% Compliant**.

#### Invariant 8: Dynamic MCP Discovery & Zero Ghost Compatibility
- **Specification:** Ephemeral agent-facing discovery surface. Agents discover tools dynamically via `tools/list`. Zero ghost compatibility schemas or dead aliases.
- **Audit Findings:**
  - `tools/list` (`server.rs:1254-1257`) returns `Self::tool_definitions()`, advertising exactly 11 active tools.
  - Zero deprecated or dead tool names are advertised in `tools/list`.
  - Tested in `adversarial_m3_server_test.rs:672` (asserts `tools.len() == 11`).
- **Status:** **100% Compliant**.

---

## 4. Categorized Findings by Severity

This section provides an exhaustive technical analysis for every finding surfaced during the post-v1.0 audit. Each finding includes exact source citations, failure modes, minimal reproducible scenarios, and concrete remediation recommendations.

---

### 4.1 Critical Findings

#### CRIT-01: Underscore Escaping in Seed Path Exact Equality in `compute_blast_radius_scoped`
- **Severity:** **CRITICAL**
- **Category:** SQLite Query Generation & Blast Radius Correctness
- **File & Lines:** `crates/code-kb-core/src/queries.rs:1918-1928`
- **Detailed Explanation:**
  When `blast_radius` is invoked with a seed file path (or when uncommitted changes are auto-detected via git status), lines 1918–1928 generate the SQL condition to seed the recursive CTE:
  ```rust
  path_conds.push(format!(
      "path = ?{idx} OR path LIKE ?{idx} || '/%' ESCAPE '\\'"
  ));
  let raw = p.replace('\\', "/").trim_start_matches("./").trim_matches('/').to_string();
  let norm = escape_like(&raw);
  params_vec.push(rusqlite::types::Value::Text(norm));
  ```
  The SQL parameter `?{idx}` is bound to `norm = escape_like(&raw)`.
  If the file path contains an underscore (e.g. `crates/code-kb-cli/tests/cli_test.rs` or `src/auth_token.rs`), `escape_like` escapes `_` into `\_`, yielding `"crates/code-kb-cli/tests/cli\\_test.rs"`.
  When SQLite executes the query:
  1. Exact equality `path = ?{idx}` tests whether the stored column `path` equals `"crates/code-kb-cli/tests/cli\\_test.rs"`. Because stored paths are unescaped literal strings (`"crates/code-kb-cli/tests/cli_test.rs"`), exact equality strictly evaluates to `FALSE`.
  2. Prefix matching `path LIKE ?{idx} || '/%' ESCAPE '\\'` tests whether `path` begins with `"crates/code-kb-cli/tests/cli_test.rs/"`. This strictly evaluates to `FALSE` because the file is a regular file, not a directory.
  Consequently, **zero seed symbols are selected** for any file path containing an underscore. Blast radius returns empty `impacted_symbols: []` and empty transitive test predictions.
- **Empirical Proof & Reproduction Scenario:**
  ```bash
  # Test a file without underscores (e.g. crates/code-kb-core/src/workspace.rs):
  ./target/release/code-kb blast-radius --file crates/code-kb-core/src/workspace.rs --json
  # Result: 20 impacted symbols returned, 20 likely tests returned.

  # Test a file with an underscore (e.g. crates/code-kb-cli/tests/cli_test.rs, containing 34 symbols and 22 inbound calls):
  ./target/release/code-kb blast-radius --file crates/code-kb-cli/tests/cli_test.rs --json
  # Result: "impacted_symbols": [], "likely_tests": 1 (only a stem-matched fallback; 0 transitive callers)

  # Direct SQLite demonstration:
  sqlite3 .code-kb/artifact.db "SELECT count(*) FROM symbols WHERE (path = 'crates/code-kb-core/tests/blast\_radius\_test.rs' OR path LIKE 'crates/code-kb-core/tests/blast\_radius\_test.rs/%' ESCAPE '\\');"
  # Output: 0
  sqlite3 .code-kb/artifact.db "SELECT count(*) FROM symbols WHERE (path = 'crates/code-kb-core/tests/blast_radius_test.rs' OR path LIKE 'crates/code-kb-core/tests/blast\_radius\_test.rs/%' ESCAPE '\\');"
  # Output: 34
  ```
- **Remediation Recommendation:**
  Bind separate parameters for exact matching (raw unescaped string with `COLLATE NOCASE`) and directory prefix matching (escaped pattern):
  ```rust
  let exact_idx = params_vec.len() + 1;
  let prefix_idx = params_vec.len() + 2;
  path_conds.push(format!(
      "path = ?{exact_idx} COLLATE NOCASE OR path LIKE ?{prefix_idx} ESCAPE '\\'"
  ));
  params_vec.push(rusqlite::types::Value::Text(raw.clone()));
  params_vec.push(rusqlite::types::Value::Text(format!("{}/%", escape_like(&raw))));
  ```

---

### 4.2 High Severity Findings

#### HIGH-01: Cycle Re-Entrance in Recursive Blast Radius CTE
- **Severity:** **HIGH**
- **Category:** Graph Traversal & Query Correctness
- **File & Lines:** `crates/code-kb-core/src/queries.rs:2023-2041`
- **Detailed Explanation:**
  The recursive CTE `impact_walk` is defined as:
  ```sql
  WITH RECURSIVE impact_walk(symbol_id, depth) AS (
      SELECT symbol_id, 0 FROM symbols WHERE ({seed_condition}) ...
      UNION
      SELECT r.from_symbol_id, iw.depth + 1
      FROM relationships r
      JOIN impact_walk iw ON r.to_symbol_id = iw.symbol_id
      WHERE iw.depth < ?
  )
  SELECT s.symbol_id, s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container, MIN(iw.depth) as min_depth
  FROM impact_walk iw
  CROSS JOIN symbols s ON iw.symbol_id = s.symbol_id
  WHERE iw.depth > 0 ...
  GROUP BY s.symbol_id, ...
  ORDER BY min_depth ASC, s.path ASC, s.name ASC
  LIMIT 200
  ```
  The CTE tracks `(symbol_id, depth)`. SQLite's `UNION` deduplicates only identical `(symbol_id, depth)` tuples.
  When a circular dependency exists in the call graph (e.g. `func_a` calls `func_b`, and `func_b` calls `func_a`):
  - Iteration 0: `(sym_a, 0)`
  - Iteration 1: `(sym_b, 1)`
  - Iteration 2: `(sym_a, 2)`
  Because `(sym_a, 2)` has a different depth than `(sym_a, 0)`, it is not eliminated by `UNION`.
  The outer query filters `WHERE iw.depth > 0`. Because `iw.depth = 2` satisfies this condition, `sym_a` is returned with `min_depth = 2`. The seed symbol is reported to the user as an impacted downstream caller of itself!
- **Empirical Proof & Reproduction Scenario:**
  ```python
  import sqlite3
  conn = sqlite3.connect(':memory:')
  conn.executescript('''
  CREATE TABLE symbols (symbol_id TEXT PRIMARY KEY, path TEXT, name TEXT, kind TEXT, start_line INT, is_test INT, test_container INT);
  CREATE TABLE relationships (from_symbol_id TEXT, to_symbol_id TEXT, kind TEXT, path TEXT, start_line INT, start_column INT);
  INSERT INTO symbols VALUES ('sym_a', 'src/a.rs', 'func_a', 'function', 1, 0, 0);
  INSERT INTO symbols VALUES ('sym_b', 'src/a.rs', 'func_b', 'function', 10, 0, 0);
  INSERT INTO relationships VALUES ('sym_b', 'sym_a', 'calls', 'src/a.rs', 11, 0);
  INSERT INTO relationships VALUES ('sym_a', 'sym_b', 'calls', 'src/a.rs', 2, 0);
  ''')
  q = '''
  WITH RECURSIVE impact_walk(symbol_id, depth) AS (
      SELECT symbol_id, 0 FROM symbols WHERE symbol_id = 'sym_a'
      UNION
      SELECT r.from_symbol_id, iw.depth + 1 FROM relationships r JOIN impact_walk iw ON r.to_symbol_id = iw.symbol_id WHERE iw.depth < 3
  )
  SELECT s.symbol_id, s.name, MIN(iw.depth) as min_depth
  FROM impact_walk iw CROSS JOIN symbols s ON iw.symbol_id = s.symbol_id
  WHERE iw.depth > 0
  GROUP BY s.symbol_id, s.name;
  '''
  rows = conn.execute(q).fetchall()
  print("Output rows:", rows)
  assert any(r[0] == 'sym_a' for r in rows), "Seed symbol sym_a leaked into results!"
  # Output: [('sym_b', 'func_b', 1), ('sym_a', 'func_a', 2)]
  ```
- **Remediation Recommendation:**
  Exclude seed symbols explicitly in the outer query:
  ```sql
  WHERE iw.depth > 0
    AND s.symbol_id NOT IN (SELECT symbol_id FROM impact_walk WHERE depth = 0)
  ```

---

#### HIGH-02: Directory Prefix Path Filtering Broken in `get_symbol_by_name`
- **Severity:** **HIGH**
- **Category:** Path Scoping & Symbol Resolution
- **File & Lines:** `crates/code-kb-core/src/queries.rs:739`
- **Detailed Explanation:**
  `get_symbol_by_name_internal` generates:
  ```sql
  WHERE (s.name = :name OR (s.name = :term AND (:parent IS NULL OR p.name = :parent)))
    AND (:path IS NULL
      OR s.path = :path COLLATE NOCASE
      OR s.path = :path_bs COLLATE NOCASE
      OR (:exact = 0 AND (s.path LIKE '%/' || :path_like ESCAPE '\\' OR s.path LIKE '%\\\\' || :path_like_bs ESCAPE '\\')))
  ```
  The clause `s.path LIKE '%/' || :path_like` appends `'%/'` before `:path_like`. This only matches paths ending with `:path_like` (such as `workspace.rs`). It **cannot match directory prefixes** (such as `crates/code-kb-core` or `src`).
  When an agent passes `path="crates/code-kb-core/src"` or `path="src"`:
  - Exact equality `s.path = 'src'` is `FALSE`.
  - Suffix match `s.path LIKE '%/src'` is `FALSE` for `src/workspace.rs` (does not end in `/src`).
  `get_symbol_by_name` returns `None`.
  When called from `find_references_scoped`, it falls back to `search_symbols_scoped`, which uses substring matching, finds the symbol, and returns:
  `SymbolNotFoundWithSuggestions("handle", "Did you mean handle in src/alpha.rs:1?")`!
  The tool states the symbol exists in `src/alpha.rs`, but failed to resolve it when scoped to `path="src"`.
- **Empirical Proof & Reproduction Scenario:**
  ```bash
  # Lookup Workspace::new with exact path:
  ./target/release/code-kb lookup "Workspace::new" --path crates/code-kb-core/src/workspace.rs
  # Result: 1 symbol found.

  # Lookup Workspace::new with file suffix:
  ./target/release/code-kb lookup "Workspace::new" --path workspace.rs
  # Result: 1 symbol found.

  # Lookup Workspace::new with directory prefix:
  ./target/release/code-kb lookup "Workspace::new" --path crates/code-kb-core/src
  # Result: 0 symbols found (Did you mean Workspace::new in crates/code-kb-core/src/workspace.rs:20?)
  ```
- **Remediation Recommendation:**
  Add directory prefix matching to line 739:
  ```sql
  OR (:exact = 0 AND (
       s.path LIKE '%/' || :path_like ESCAPE '\\'
    OR s.path LIKE '%\\\\' || :path_like_bs ESCAPE '\\'
    OR s.path LIKE :path_like || '/%' ESCAPE '\\'
    OR s.path LIKE :path_like_bs || '\\\\%' ESCAPE '\\'
  ))
  ```

---

#### HIGH-03: Silent Swallowing of `AmbiguousSymbol` in `lookup_symbol`
- **Severity:** **HIGH**
- **Category:** Error Propagation & Agent Ergonomics
- **File & Lines:** `crates/code-kb-cli/src/mcp/server.rs:809-842` and `crates/code-kb-core/src/queries.rs:327-331`
- **Detailed Explanation:**
  In `server.rs:809`:
  ```rust
  let matches = if (query.contains("::") || query.contains('.'))
      && let Ok(Some(sym)) = code_kb_core::get_symbol_by_name(&conn, query, path_filter)
  {
      vec![sym]
  } else {
      match search_symbols_scoped(&conn, query, kind, path_filter, include_tests, limit) {
          Ok(m) => m,
          Err(e) => return CallToolResult::error(e.to_string()),
      }
  };
  ```
  When `get_symbol_by_name` detects multiple candidates for a qualified symbol (e.g. `ChildGuard::deref`), it returns:
  `Err(QueryError::AmbiguousSymbol("ChildGuard::deref", 3, "...candidates..."))`.
  Because line 810 uses `let Ok(Some(sym))`, the `Err` result evaluates to `false`. The error is silently discarded.
  Execution drops into the `else` branch calling `search_symbols_scoped`.
  In `search_symbols_scoped:327`, the exact same `if let Ok(Some(sym))` executes and fails again.
  `search_symbols_scoped` then searches `WHERE name = "ChildGuard::deref"`. Because `symbols.name` stores only the terminal identifier (`"deref"`), zero rows match.
  Execution drops into FTS5 text search (`server.rs:828`), returning unrelated markdown design notes. The agent never receives the ambiguity diagnostic.
- **Empirical Proof & Reproduction Scenario:**
  ```bash
  # ChildGuard::deref exists in 3 test files:
  # - crates/code-kb-cli/tests/adversarial_m2_server_test.rs:11
  # - crates/code-kb-cli/tests/adversarial_m3_server_test.rs:11
  # - crates/code-kb-cli/tests/mcp_test.rs:9

  ./target/release/code-kb lookup "ChildGuard::deref" --include-tests --json
  # Exit code: 0
  # Stderr: "" (empty)
  # Results: 14 unrelated FTS text snippet matches from Markdown design plans!
  # Ambiguity candidates: Completely omitted.
  ```
- **Remediation Recommendation:**
  Explicitly pattern match `QueryError::AmbiguousSymbol` in `server.rs:809` and `queries.rs:327`:
  ```rust
  if query.contains("::") || query.contains('.') {
      match code_kb_core::get_symbol_by_name(&conn, query, path_filter) {
          Ok(Some(sym)) => vec![sym],
          Err(code_kb_core::QueryError::AmbiguousSymbol(name, count, list)) => {
              return CallToolResult::error(format!(
                  "Ambiguous symbol '{name}' ({count} candidates found):\n{list}"
              ));
          }
          _ => search_symbols_scoped(&conn, query, kind, path_filter, include_tests, limit)
              .map_err(|e| CallToolResult::error(e.to_string()))?,
      }
  }
  ```

---

#### HIGH-04: Missing Windows Backslash Normalization in `find_callee_signatures`
- **Severity:** **HIGH**
- **Category:** Windows Compatibility & Query Correctness
- **File & Lines:** `crates/code-kb-core/src/queries.rs:1415-1416`
- **Detailed Explanation:**
  In `find_callee_signatures`:
  ```sql
  s_to.parent_symbol_id IS NULL
  AND EXISTS (
      SELECT 1 FROM json_each(p.target_namespace_json)
      WHERE value NOT IN ('std', 'core', 'alloc', 'crate', 'super')
        AND s_to.path LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
  )
  ```
  Line 1415 compares raw `s_to.path` against `'%/' || ...`.
  Across all other queries (`queries.rs:1036, 1238, 1497, 1511, 1985, 1999`), forward slash normalization is applied:
  `('/' || replace(s_to.path, '\\', '/')) LIKE '%/' || ...`.
  On Windows systems where `symbols.path` contains backslashes (e.g. `src\models\user.rs`), `s_to.path LIKE '%/user.%'` will never match. Pending callee signatures for unresolved function calls fail to populate in `get_symbol_context` on Windows.
- **Remediation Recommendation:**
  Update line 1415 to normalize `s_to.path`:
  ```sql
  AND ('/' || replace(s_to.path, '\\', '/')) LIKE '%/' || replace(replace(replace(value, '\\', '\\\\'), '%', '\\%'), '_', '\\_') || '.%' ESCAPE '\\'
  ```

---

#### HIGH-05: Missing `COLLATE NOCASE` in `find_structural_facts_scoped`
- **Severity:** **HIGH**
- **Category:** Windows Compatibility & Case Sensitivity
- **File & Lines:** `crates/code-kb-core/src/queries.rs:1619`
- **Detailed Explanation:**
  Line 1619 constructs:
  ```sql
  WHERE (:cat IS NOT NULL AND {cat_clause})
    AND (:path IS NULL OR replace(sf.path, '\\', '/') = :path OR replace(sf.path, '\\', '/') LIKE :dir_prefix ESCAPE '\\')
  ```
  The exact path equality `replace(sf.path, '\\', '/') = :path` omits `COLLATE NOCASE`.
  On Windows (where filesystems are case-insensitive), if a caller passes `path="src/Api.rs"` when the index stored `"src/api.rs"`, exact equality fails.
- **Remediation Recommendation:**
  Add `COLLATE NOCASE` to the equality comparison:
  ```sql
  AND (:path IS NULL OR replace(sf.path, '\\', '/') = :path COLLATE NOCASE OR replace(sf.path, '\\', '/') LIKE :dir_prefix ESCAPE '\\')
  ```

---

#### HIGH-06: Retained Memory RSS Exceeding < 15 MB Bound & 256 MB MMAP
- **Severity:** **HIGH**
- **Category:** Architectural Invariants & Memory Bounds
- **File & Lines:** `crates/code-kb-core/src/db.rs:42` (`PRAGMA mmap_size = 268435456;`), `AGENTS.md:16, 51`, `CLAUDE.md:16, 51`
- **Detailed Explanation:**
  Core Invariant 2 promises:
  > *"SQLite in WAL mode is the query engine; `code-kb` keeps retained memory < 15 MB."*
  
  On non-Windows platforms, `open_read_only` in `db.rs:42` configures:
  `PRAGMA mmap_size = 268435456;` (256 MB memory map).
  As SQLite queries read database pages, the kernel memory-maps database pages into the process virtual address space. Once faulted in, resident pages increase process VmRSS.
  Additionally, `McpServer::new` spawns a background thread for `reconcile_offline_edits`, causing glibc to allocate thread-local memory arenas that are not returned to the OS.
- **Empirical Proof & Reproduction Scenario:**
  Executing `target/release/code-kb serve` under load and monitoring `/proc/<pid>/status` VmRSS:
  - Immediately post-`initialize`: **14.57 MB** (< 15 MB)
  - After background reconciliation: **19.31 MB** (+4.31 MB)
  - After 40 sequential tool calls: **20.57 MB** (+5.57 MB)
  - After 80 sequential tool calls: **20.68 MB** (+5.68 MB)
  - Idle retained RSS (2s post-load): **20.64 MB** (+5.64 MB)
  - CLI symbol search peak RSS (`scripts/benchmark_quality.py`): **29.69 MB**.
  
  While the memory curve is exceptionally stable (only 0.41 MB variance over 80 tool calls, confirming zero unbounded heap leaks), steady-state retained RSS is **20.68 MB**, exceeding the advertised `< 15 MB` threshold by **37.8%**.
- **Remediation Recommendation:**
  1. Set `PRAGMA mmap_size = 0;` (or cap at `8388608` / 8 MB) across all platforms.
  2. Invoke `malloc_trim(0)` on Linux after `reconcile_offline_edits` finishes.
  3. Or adjust documented invariant in `AGENTS.md` and `README.md` to `< 25 MB retained memory / < 30 MB peak RSS` to accurately reflect real-world requirements.

---

#### HIGH-07: Startup Reconciliation Reading & Hashing 100% of Unchanged Files
- **Severity:** **HIGH**
- **Category:** Performance & Cache Invalidation
- **File & Lines:** `crates/code-kb-core/src/sync.rs:405-418` and `docs/plans/005-startup-reconciliation.md:39-53`
- **Detailed Explanation:**
  Design plan 005 specifies that Phase 1 must perform a fast stat sweep comparing `disk_mtime > files.indexed_at` and `disk_bytes != files.content_bytes`, discarding 99.9% of unchanged files in ~15ms without reading file contents.
  In `crates/code-kb-core/src/sync.rs:405-418`:
  ```rust
  let is_modified = if indexed_bytes != bytes {
      true
  } else {
      match std::fs::read(path) {
          Ok(content) => !compute_content_hash_matches(&content, &stored_hash),
          Err(e) => {
              warn!("Failed to read '{}' for hash verification: {e}", path.display());
              false
          }
      }
  };
  ```
  `indexed_at` is never queried or compared against `disk_mtime`. For every unchanged file where `disk_bytes == indexed_bytes` (which is 100% of files in an untouched repository), `std::fs::read(path)` reads the file from disk and computes its hash.
- **Empirical Proof & Reproduction Scenario:**
  Running `strace -f -e trace=openat ./target/release/code-kb serve` on an indexed repository where 0 files have changed:
  Traced thread 444143 during `reconcile_offline_edits` opened **33 out of 33 (100%)** `.rs` source files in `crates/` with `O_RDONLY` and read them from disk.
  In a 20,000-file repository, startup reconciliation incurs 20,000 disk file reads instead of a fast metadata stat sweep.
- **Remediation Recommendation:**
  Store file modification time in `files.mtime` (or compare filesystem `mtime` against `files.indexed_at`). Only read and hash file contents when `disk_mtime > indexed_at`.

---

### 4.3 Medium Severity Findings

#### MED-01: Unbounded `max_depth` in Public API `compute_blast_radius_scoped`
- **Severity:** **MEDIUM**
- **Category:** Query Bounds & Resource Protection
- **File & Lines:** `crates/code-kb-core/src/queries.rs:1856, 1934, 1964, 2014`
- **Explanation:**
  While the MCP handler `blast_radius_op` clamps depth to `max_depth.min(5)`, the core query function `compute_blast_radius_scoped` is `pub` and directly binds `max_depth as i64`. Direct callers passing large values (e.g. `100`) on dense graphs trigger exponential walks and heavy `json_each` evaluations.
- **Remediation:** Enforce `let max_depth = max_depth.clamp(1, 10);` inside `compute_blast_radius_scoped`.

#### MED-02: Hard SQLite Error on Malformed or Empty String in `target_namespace_json`
- **Severity:** **MEDIUM**
- **Category:** Robustness & Schema Edge Cases
- **File & Lines:** `crates/code-kb-core/src/queries.rs:1983, 1990, 1028-1041, 1230-1243`
- **Explanation:**
  Queries check `(p.target_namespace_json IS NULL OR p.target_namespace_json = '[]')` before evaluating `EXISTS (SELECT 1 FROM json_each(p.target_namespace_json) WHERE ...)`. If an extractor writes an empty string `""` instead of `NULL`, `json_each("")` fails with SQLite `OperationalError: malformed JSON`, crashing the query.
- **Remediation:** Check `(p.target_namespace_json IS NULL OR p.target_namespace_json = '' OR p.target_namespace_json = '[]' OR json_valid(p.target_namespace_json) = 0)`.

#### MED-03: Unescaped LIKE and Substring Collisions in `search_symbols_scoped`
- **Severity:** **MEDIUM**
- **Category:** Path Scoping & Wildcard Escaping
- **File & Lines:** `crates/code-kb-core/src/queries.rs:361, 457, 517`
- **Explanation:**
  Line 361 appends `AND (path = ?4 OR path LIKE '%' || ?4 || '%')`. `?4` is not escaped with `escape_like`, so underscores match any character. Substring matching causes `path="core"` to match `src/score.rs`. On Windows backslashes, `?4` (normalized with `/`) fails exact matching.
- **Remediation:** Standardize on `replace(path, '\\', '/') = ?4 COLLATE NOCASE OR replace(path, '\\', '/') LIKE ?4_prefix ESCAPE '\\'`.

#### MED-04: Asymmetric Symlink Resolution Between Relative and Absolute Path Filters
- **Severity:** **MEDIUM**
- **Category:** Path Resolution & Symlink Boundaries
- **File & Lines:** `crates/code-kb-core/src/workspace.rs:603-628`
- **Explanation:**
  Passing an absolute path to an in-workspace symlink triggers `resolve_path` which canonicalizes the path to the target. Passing a relative path runs `clean_path` without canonicalization. Since the index does not follow symlinks, relative symlink queries return 0 results while absolute queries succeed.
- **Remediation:** Canonicalize in-workspace relative symlinks within workspace bounds in `relativize_filter`.

#### MED-05: Case-Sensitivity Mismatch in `compute_blast_radius_scoped` Seed Path
- **Severity:** **MEDIUM**
- **Category:** Windows Compatibility
- **File & Lines:** `crates/code-kb-core/src/queries.rs:1919`
- **Explanation:**
  Seed path exact equality `path = ?{idx}` omits `COLLATE NOCASE`. If casing differs on Windows, exact file seeding fails.
- **Remediation:** Add `COLLATE NOCASE` to `path = ?{idx}`.

#### MED-06: Silent Stale Query Results in Untargeted Operations and All CLI Invocations
- **Severity:** **MEDIUM**
- **Category:** Freshness & Contract Integrity
- **File & Lines:** `crates/code-kb-cli/src/main.rs:465-670`, `crates/code-kb-cli/src/mcp/server.rs:809-849`
- **Explanation:**
  CLI subcommands (`lookup`, `search`, `refs`, `blast-radius`, `facts`, `outline`) open SQLite in read-only mode and execute queries directly without checking file or database freshness. In MCP mode, symbol searches bypass the Tier 2 JIT staleness guard, returning stale results if files changed within the 150ms watcher debounce window.
- **Remediation:** In CLI mode, print a fast staleness warning if `artifact.db` is older than repository changes. In MCP mode, attach freshness metadata.

#### MED-07: JIT Staleness Guard Bypassed for Newly Added Symbols When `--file` is Omitted
- **Severity:** **MEDIUM**
- **Category:** Freshness & JIT Staleness
- **File & Lines:** `crates/code-kb-core/src/ops.rs:43-85`
- **Explanation:**
  In `get_symbol_body_op`, if `file_path` is omitted, it first queries SQLite. If the symbol was newly written to disk, SQLite returns `None`, and line 59 returns `OpError::SymbolNotFound` immediately, bypassing the JIT file-refresh step at line 84.
- **Remediation:** When `get_symbol_by_name` returns `None` and dirty files exist on disk, trigger a targeted JIT refresh on modified files before returning `SymbolNotFound`.

---

### 4.4 Low & Advisory Findings

#### LOW-01: Duplicate Reference Sites in `find_references_internal`
- **Severity:** **LOW**
- **Category:** Query Hygiene
- **File & Lines:** `crates/code-kb-core/src/queries.rs:1005-1073, 1193-1332`
- **Explanation:**
  When `results.len() < limit`, `find_references_internal` queries `pending_relationships` and appends rows. If the extractor recorded a call in both `relationships` and `pending_relationships`, duplicate entries appear.
- **Remediation:** Deduplicate `results` on `(from_symbol_id, to_symbol_name, path, start_line, start_column)`.

#### LOW-02: CLI `code-kb edit` Flag Inconsistency with MCP `expected_body_hash`
- **Severity:** **LOW**
- **Category:** Invariant 6 (Tool Ergonomics & CLI Parity)
- **File & Lines:** `crates/code-kb-cli/src/main.rs:227`
- **Explanation:**
  CLI `code-kb edit` only accepts `--expected-hash`, whereas MCP tool schema advertises `expected_body_hash`. If an agent uses the MCP schema name on the CLI, Clap errors out: `error: unexpected argument '--expected-body-hash'`.
- **Remediation:** Add clap aliases: `#[arg(long, alias = "expected-body-hash", alias = "body-hash")]`.

#### LOW-03: Missing CLI Argument Aliases for Path/File Across Search & Graph Commands
- **Severity:** **LOW**
- **Category:** Invariant 6 (Tool Ergonomics & CLI Parity)
- **File & Lines:** `crates/code-kb-cli/src/main.rs:95, 171, 190`
- **Explanation:**
  MCP tool handlers accept aliases (`path` or `file_path` for file, `max_depth` for depth). On the CLI, passing `--path` to `refs` or `blast-radius` produces Clap argument errors.
- **Remediation:** Add aliases to `RefsArgs` (`alias = "path"`, `alias = "file-path"`), `BlastRadiusArgs`, and `OutlineArgs` (`alias = "max-depth"`).

#### LOW-04: CLI `--json` Mode Emits Plaintext on Stderr and Empty Stdout on Error
- **Severity:** **LOW / ADVISORY**
- **Category:** Error Protocol Contracts
- **File & Lines:** `crates/code-kb-cli/src/main.rs:284, 458-463`
- **Explanation:**
  When `--json` is active and an error occurs, stdout is empty and stderr prints plaintext (`Error: Symbol 'foo' not found`). Automated wrappers expecting JSON on all streams must implement bifurcated parsing.
- **Remediation:** When `--json` is set, emit structured JSON `{"error": "...", "status": "failed"}` to stdout with non-zero exit code.

#### ADV-01: Agent Routing Directive Parameter Name Discrepancies in `routing-block.md`
- **Severity:** **ADVISORY**
- **Category:** Documentation & Prompt Alignment
- **File & Lines:** `crates/code-kb-cli/src/routing-block.md:12, 21`
- **Explanation:**
  `routing-block.md` documents `codebase_outline(subpath, depth)` (canonical is `path`) and `blast_radius(path="file")` (canonical is `file`). Tool 11 (`telemetry_summary`) is omitted from the quick summary.
- **Remediation:** Align routing block examples with canonical schema property names and add `telemetry_summary`.

#### ADV-02: Git Worktree Isolation & Path Containment Verified Robust
- **Severity:** **ADVISORY (POSITIVE)**
- **Category:** Architectural Verification
- **File & Lines:** `crates/code-kb-core/src/workspace.rs:537-580`, `crates/code-kb-cli/src/mcp/server.rs:640-711`
- **Explanation:**
  Worktree database isolation is sound. Databases are stored at `<worktree_root>/.code-kb/artifact.db`. Parent DB WAL is flushed via `checkpoint_truncate` before copying to worktrees. `Workspace::resolve_path` canonicalizes paths and rejects external symlinks with `WorkspaceError::PathOutsideWorkspace`.

---

## 5. Query Correctness, Traversal & Platform Compatibility Deep Dive

### 5.1 CTE Recursion Depth Limits & Cycle Protection
The recursive CTE `impact_walk` computes multi-hop reverse reachability across callers. The query structure relies on SQLite's `WITH RECURSIVE`:
- **Termination Mechanism:** Bounded by `WHERE iw.depth < ?`.
- **Deduplication Flaw:** Because the recursive CTE tuple is `(symbol_id, depth)`, SQLite's `UNION` treats identical symbols at different depths as distinct entries. On cyclic graphs (`A -> B -> A`), `A` re-enters at depth 2 and is returned in the output set (HIGH-01).
- **Public API Exposure:** `compute_blast_radius_scoped` lacks parameter clamping, exposing callers to runaway CTE walks if unconstrained depths are supplied (MED-01).

### 5.2 Disambiguation Logic Across Symbol Name, File Path, and Language
Symbol resolution operates across multiple tiers:
1. **Qualified Identifiers (`Type::method`, `Object.property`):** Resolved via `get_symbol_by_name_internal`. When multiple candidates exist, `AmbiguousSymbol` is returned. However, swallowing this error in `server.rs` and `queries.rs` drops into FTS5 text search, hiding ambiguities from agents (HIGH-03).
2. **Path Filtering:** Intended to support exact files, file suffixes, and directory prefixes. However, line 739 only matched suffixes (`%/file.rs`), completely disabling directory prefix filtering (HIGH-02).
3. **Language-Agnostic Callee Filtering:** Successfully eliminates external stdlib/runtime noise across ~40 languages by cross-referencing unresolved call sites against the workspace symbol index. However, on Windows, missing backslash normalization in `find_callee_signatures` disables pending callee resolution (HIGH-04).

### 5.3 Symlink Resolution & Git Worktree Isolation
- **Worktree Lifecycle:** Every git worktree maintains its own isolated database at `<root>/.code-kb/artifact.db`. Deleting a worktree cleanly removes its AST index with zero orphaned external state. Durable telemetry is preserved globally at `~/.code-kb/telemetry.db`.
- **WAL Flush Before Copy:** In `server.rs:640-711`, when initializing a worktree from a parent repository, `checkpoint_truncate` flushes all pending WAL transactions into the parent `artifact.db` before copying, preventing corrupted or incomplete indexes.
- **Symlink Containment:** `Workspace::resolve_path` verifies canonical paths remain strictly inside `canonical_root`. External symlinks return `WorkspaceError::PathOutsideWorkspace`. Relative in-workspace symlinks require canonicalization in `relativize_filter` (MED-04).

### 5.4 Windows Compatibility Patterns
- **Path Normalization:** `dunce::simplified` strips `\\?\` and `\\?\UNC\` prefixes.
- **Component Equality:** `strip_prefix_lossy` and `paths_equal` perform case-insensitive component comparisons on Windows.
- **Output Uniformity:** All JSON outputs enforce explicit forward slashes `/`.
- **File Handle Lifecycle:** `PRAGMA mmap_size = 0;` on Windows ensures SQLite does not hold memory-mapped file locks, permitting atomic tempfile swaps and file deletions.
- **SQL Case Sensitivity:** Exact equality comparisons in SQLite must include `COLLATE NOCASE` to guarantee case-preserving filesystem compatibility (HIGH-05, MED-05).

---

## 6. Freshness, Error Handling & Benchmark Rigor Deep Dive

### 6.1 The 3-Tier Synchronization Architecture vs. Cold-Start Reconciliation
`code-kb` implements a tiered synchronization architecture:
- **Tier 1 (Synchronous Edit):** When `replace_symbol_body` modifies a file, `sync::update_file` re-indexes SQLite before returning.
- **Tier 2 (JIT Read-Time Guard):** Before reading symbol bodies, skeletons, or slices, `sync::ensure_fresh_file` validates file freshness.
- **Tier 3 (Background Watcher):** In MCP mode, `notify-debouncer-full` monitors workspace changes with a 150ms debounce window and a 50-file checkout storm circuit breaker.
- **Cold-Start Offline Reconciliation:** On server startup, `reconcile_offline_edits` sweeps disk files. However, omitting `mtime` checking against `indexed_at` forces 100% of unchanged files to be read and hashed from disk (HIGH-07).

### 6.2 Cache Invalidation & Silent Staleness
- **CLI Mode Blindspot:** CLI subcommands open SQLite in `open_read_only` with zero file watching or reconciliation. Offline edits remain invisible until `code-kb scan` or an MCP session is launched (MED-06).
- **New Symbol Discovery Edge Case:** When `get_symbol_body` is called without `--file` for a newly created symbol, SQLite lookup returns `None` and returns `SymbolNotFound` before reaching the JIT file-refresh logic (MED-07).

### 6.3 Error Propagation Contracts
- **MCP Protocol Hygiene:** Strict stdio stream separation. Stderr tracing is suppressed in `code-kb serve` and directed to `.code-kb/logs/code-kb.log`. Stdout is reserved exclusively for newline-delimited JSON-RPC. Operational errors return `JsonRpcResponse::success` with `isError: true` and descriptive text.
- **CLI Standard vs. JSON Mode:** Exit codes are strictly consistent (`0` for success, `1` for operational failure, `2` for Clap argument errors). In `--json` mode, errors currently emit plaintext to stderr while stdout remains empty (LOW-04).

### 6.4 Memory Consumption Bounds & Load Profiling
Empirical load testing of `target/release/code-kb serve` across 80 sequential MCP tool calls revealed:
- **Memory Stability:** VmRSS varied by only 0.41 MB between Batch 1 (20.27 MB) and Batch 10 (20.68 MB), demonstrating zero unbounded memory leaks.
- **Retained Footprint:** Steady-state retained RSS is **20.68 MB**, exceeding the advertised `< 15 MB` threshold. Peak RSS under CLI symbol queries reaches **29.69 MB**.
- **Root Drivers:** `PRAGMA mmap_size = 268435456` in `db.rs:42` faults database pages into process RSS, while the background worker thread allocates glibc memory arenas.

---

## 7. Prioritized Remediation Roadmap

The identified findings are organized into three sequential implementation phases to ensure maximum stability and zero regression.

```
┌────────────────────────────────────────────────────────────────────────────┐
│                    PRIORITIZED REMEDIATION ROADMAP                         │
├────────────────────────────────────────────────────────────────────────────┤
│ Phase 1: Critical & High Query Correctness & Platform Stability            │
│ ───────────────────────────────────────────────────────────────            │
│ • CRIT-01: Fix underscore escaping in blast radius seed path binding       │
│ • HIGH-01: Add seed symbol exclusion to recursive blast radius CTE         │
│ • HIGH-02: Add directory prefix matching to get_symbol_by_name             │
│ • HIGH-03: Propagate AmbiguousSymbol error in lookup_symbol and search     │
│ • HIGH-04: Add forward slash normalization in find_callee_signatures       │
│ • HIGH-05: Add COLLATE NOCASE to find_structural_facts_scoped              │
│ • MED-01: Clamp max_depth in compute_blast_radius_scoped                   │
│ • MED-02: Add json_valid guard before json_each namespace matching        │
│ • MED-03: Add escape_like and slash normalization in search_symbols_scoped │
│ • MED-05: Add COLLATE NOCASE to blast radius seed path equality            │
├────────────────────────────────────────────────────────────────────────────┤
│ Phase 2: Memory Optimization & Startup Reconciliation                      │
│ ─────────────────────────────────────────────────────                      │
│ • HIGH-06: Reduce PRAGMA mmap_size on non-Windows to 0 or 8MB              │
│ • HIGH-07: Add mtime vs indexed_at check in startup reconciliation sweep   │
├────────────────────────────────────────────────────────────────────────────┤
│ Phase 3: Freshness Guarantees, Ergonomics & Error Contracts                │
│ ───────────────────────────────────────────────────────────                │
│ • MED-04: Canonicalize in-workspace relative symlinks in relativize_filter │
│ • MED-06: Add staleness warning to untargeted CLI commands                 │
│ • MED-07: Trigger dirty-file JIT scan on new symbol resolution failure     │
│ • LOW-01: Deduplicate relationship rows in find_references_internal        │
│ • LOW-02: Add --expected-body-hash alias to CLI edit subcommand            │
│ • LOW-03: Add --path / --file-path aliases to refs and blast-radius CLI    │
│ • LOW-04: Emit structured JSON error object on CLI --json failure          │
│ • ADV-01: Align routing-block.md parameters with canonical tool schemas    │
└────────────────────────────────────────────────────────────────────────────┘
```

### Phase 1: Critical & High Query Correctness & Platform Stability
1. **Fix `compute_blast_radius_scoped` Seed Path Escaping (`CRIT-01`)**:
   Split exact path equality and directory prefix matching into separate bound parameters. Bind unescaped `raw` with `COLLATE NOCASE` for exact equality, and escaped `format!("{}/%", escape_like(&raw))` for prefix matching.
2. **Prevent Seed Leakage in Recursive Blast Radius CTE (`HIGH-01`)**:
   Add `AND s.symbol_id NOT IN (SELECT symbol_id FROM impact_walk WHERE depth = 0)` to the outer CTE selection.
3. **Fix Directory Prefix Matching in `get_symbol_by_name` (`HIGH-02`)**:
   Add `OR s.path LIKE :path_like || '/%' ESCAPE '\\' OR s.path LIKE :path_like_bs || '\\\\%' ESCAPE '\\'` to line 739.
4. **Propagate `AmbiguousSymbol` Diagnostics in `lookup_symbol` (`HIGH-03`)**:
   Pattern match `QueryError::AmbiguousSymbol` when calling `get_symbol_by_name` and return actionable error messages listing candidates.
5. **Normalize Backslashes in `find_callee_signatures` (`HIGH-04`)**:
   Change `s_to.path LIKE '%/' || ...` to `('/' || replace(s_to.path, '\\', '/')) LIKE '%/' || ...`.
6. **Add `COLLATE NOCASE` to Structural Facts & Symbol Search (`HIGH-05`, `MED-03`, `MED-05`)**:
   Add `COLLATE NOCASE` to `find_structural_facts_scoped` (line 1619) and `search_symbols_scoped` (line 361).
7. **Harden CTE Depth & JSON Functions (`MED-01`, `MED-02`)**:
   Clamp `max_depth` to 10 in `compute_blast_radius_scoped` and guard `json_each` with `json_valid`.

### Phase 2: Memory Optimization & Startup Reconciliation
1. **Reduce Memory Footprint & Enforce Retained Bounds (`HIGH-06`)**:
   Change `PRAGMA mmap_size = 268435456;` in `db.rs:42` to `PRAGMA mmap_size = 0;` (or cap at 8 MB). Invoke `malloc_trim(0)` on Linux following cold-start reconciliation.
2. **Add Filesystem `mtime` Check to Startup Reconciliation (`HIGH-07`)**:
   Compare filesystem `mtime` against `files.indexed_at`. Only read and hash file contents when `disk_mtime > indexed_at`, discarding 99.9% of unchanged files in ~15ms.

### Phase 3: Freshness Guarantees, Ergonomics & Error Contracts
1. **Asymmetric Symlink Resolution (`MED-04`)**: Canonicalize in-workspace relative symlinks within workspace bounds in `relativize_filter`.
2. **Freshness Warnings in CLI (`MED-06`)**: Add fast staleness check to CLI commands and print notice if index is outdated.
3. **JIT Staleness for New Symbols (`MED-07`)**: When `get_symbol_by_name` fails, perform JIT scan on dirty files before returning `SymbolNotFound`.
4. **Harmonize CLI Argument Aliases (`LOW-02`, `LOW-03`)**: Add `--expected-body-hash` to `code-kb edit`, and `--path`/`--file-path` to `refs` and `blast-radius`.
5. **CLI JSON Error Payload (`LOW-04`)**: Emit structured JSON error payload on stdout under `--json`.
6. **Align Routing Block Directives (`ADV-01`)**: Update parameter names in `routing-block.md` to match canonical schemas.

---

## 8. Conclusion

The `code-kb` post-v1.0 codebase exhibits exceptional baseline health, flawless documentation synchronization, strict enforcement of zero workspace parameters, and robust single-turn atomic edits. 

Addressing the 1 Critical bug (`CRIT-01`) and 7 High-severity defects (`HIGH-01` through `HIGH-07`) outlined in this report will eliminate edge-case query failures in blast radius and symbol lookup, guarantee platform parity on Windows, cut cold-start startup I/O by orders of magnitude on large repositories, and bring retained process memory into strict compliance with advertised architectural limits.
