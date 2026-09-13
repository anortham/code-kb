# Post-v0.5.1 Comprehensive Project Audit: Code Correctness, Performance Bounds, Dead Code, and Documentation Synchronization

- **Plan ID**: `013`
- **Target**: `code-kb` post-v0.5.1
- **Status**: Proposed / Reviewed
- **Date**: 2026-09-13
- **Auditors & Contributors**: Teamwork Audit Swarm (Baseline, Explorers 1 & 2, Spec Miner, Synthesis)

---

## 1. Executive Summary & Scope

### 1.1 Overview
Following the release of `code-kb` v0.5.1 (commits `dea07ea` and `06098c5`), a multi-agent architectural audit was conducted across the entire codebase (`crates/code-kb-core` and `crates/code-kb-cli`), tool contracts, documentation, and operational environments. The audit systematically verified:
- **R1 (Logic, Correctness & Invariants)**: Full verification of runtime logic, panic safety, error propagation, path canonicalization, and compliance with the 7 Core Invariants in `AGENTS.md`.
- **R2 (Performance, Watcher & Resource Bounds)**: SQLite query execution plans, FTS5 virtual table scans, recursive CTE scalability, filesystem watcher event storm resilience, and heap memory retention within the `< 15 MB` threshold.
- **R3 (Dead Code, Redundancy & Dependencies)**: Unused functions, redundant trimming/preparations, orphaned queries/exports, and Cargo workspace dependency inheritance.
- **R4 (Documentation, Spec & Schema Synchronization)**: Byte-for-byte synchronization of `AGENTS.md` and `CLAUDE.md`, 1:1 parity between CLI commands and MCP tool schemas, and documentation consistency across `README.md`, `docs/site/`, and release plans.
- **R5 (Prioritized Review Findings Plan)**: Synthesis of all audited observations into an actionable, prioritized remediation plan with reproduction scripts and concrete code diffs.

### 1.2 High-Level Scorecard

| Dimension | Post-v0.5.1 Status | Summary Scorecard |
|---|---|---|
| **Baseline Quality** | **PASS** | 83/83 tests pass (10 suites); 0 clippy warnings (`-D warnings`); 0 `cargo fmt` diffs. |
| **Sync Contract** | **PASS** | `AGENTS.md` and `CLAUDE.md` match byte-for-byte (6,436 bytes, 0 differences). |
| **Invariant 1 (MCP Workspace Invariant)** | **PASS** | 0 workspace parameters exposed across all 10 MCP tool schemas; verified by automated integration test. |
| **Query Performance** | **ACTION REQUIRED** | 2 major SQLite optimizer inversions discovered (**623x** slowdown in FTS5, **48x** slowdown in CTE blast radius). |
| **Concurrency & Watcher Resiliency** | **ACTION REQUIRED** | Watcher thread exhibits panic risk on root matcher construction; synchronous blocking callback in debounce loop. |
| **Path Identity (Windows / Worktrees)** | **ACTION REQUIRED** | `find_workspace_root` returns uncanonicalized path, causing redundant rebind loops on case-insensitive or symlinked roots. |
| **Logic & Error Taxonomy** | **ACTION REQUIRED** | Callee resolution drops ambiguous methods via N+1 queries; `blast_radius_op` drops `file` when `symbol` provided; error strings malformed. |
| **Dead Code & Dependencies** | **ACTION REQUIRED** | Dead functions, redundant file stem trimming, duplicate Cargo dependencies, missing `.agents` exclusion. |
| **Documentation & CLI Parity** | **ACTION REQUIRED** | Stale versions in `docs/site/` and `TODO.md`, `README.md` default depth drift, missing `--json` in `edit`. |

### 1.3 Finding Counts by Severity Tier
- **P0 Blocker**: **0** (Codebase compiles cleanly, zero compiler/clippy warnings, all tests pass).
- **P1 Performance / Bug / Correctness**: **8** (P1-1 to P1-8).
- **P2 Dead Code / Technical Debt / Edge Cases**: **12** (P2-1 to P2-12).
- **P3 Documentation / CLI Polish / Minor Ergonomics**: **11** (P3-1 to P3-11).
- **Total Actionable Items**: **31**.

---

## 2. Baseline Validation & Environment Analysis

### 2.1 Programmatic Baseline Results

All baseline verification commands were executed natively within `/home/murphy/source/code-kb` on Linux x86_64:

```text
1. cargo test --all-targets (with isolated TMPDIR):
   Total: 83 passed; 0 failed; 0 ignored; 0 filtered out (finished in 2.72s)
   - code-kb-cli (src/main.rs): 0 passed
   - code-kb-cli (tests/cli_test.rs): 14 passed
   - code-kb-cli (tests/mcp_test.rs): 4 passed
   - code-kb-core (src/lib.rs): 32 passed
   - code-kb-core (tests/blast_radius_test.rs): 3 passed
   - code-kb-core (tests/disambiguation_test.rs): 7 passed
   - code-kb-core (tests/edit_test.rs): 8 passed
   - code-kb-core (tests/freshness_test.rs): 10 passed
   - code-kb-core (tests/watcher_test.rs): 4 passed
   - code-kb-core (tests/worktree_test.rs): 1 passed

2. cargo clippy --all-targets -- -D warnings:
   Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.09s
   Result: 0 warnings, 0 errors across workspace.

3. cargo fmt --all -- --check:
   Result: Exit code 0 (100% compliant with rustfmt standard).

4. diff -u AGENTS.md CLAUDE.md && cmp AGENTS.md CLAUDE.md:
   Result: Exit code 0 (0 bytes differing; 6,436 bytes each; MD5: 9f3bbaca5251270bbdd93ec437d5f5d4).

5. cargo test --test mcp_test:
   Result: 4 passed; 0 failed. Verifies Invariant 1 (zero workspace parameters exposed).
```

### 2.2 Host Environment tmpfs Quota Exhaustion Analysis

During initial execution of `cargo test --all-targets` and `cargo test --test mcp_test` with default system settings (`TMPDIR` unset), 9 tests in `cli_test.rs` and 4 tests in `mcp_test.rs` failed with:
```text
called Result::unwrap() on an Err value: Os { code: 122, kind: QuotaExceeded, message: "Disk quota exceeded" }
SqliteFailure(Error { code: SystemIoFailure, extended_code: 4874 }, Some("disk I/O error"))
db_open_failed: could not create SQLite artifact: disk I/O error (/tmp/.tmpHrTW63/.code-kb/artifact.db)
```

#### Root-Cause Diagnostics
Direct inspection using `quota -v` and `df -h /tmp` revealed:
```text
Disk quotas for user murphy (uid 1000): 
     Filesystem  blocks   quota   limit   grace   files   quota   limit   grace
          tmpfs   33808  26225801 26225801              80       0       0        
          tmpfs 26225724  26225801 26225801           48027       0       0        
```
- The host user reached the `26,225,801` block quota (~25.6 GB) on `tmpfs` (`/tmp`), leaving only ~77 KB of free space.
- Existing background artifacts (e.g., `/tmp/claude-1000` consuming 12 GB, `/tmp/miller-sqlite-3534` consuming 3.0 GB, and packaging scratch directories) held the tmpfs quota at 99.999% capacity.
- Rust's `tempfile::tempdir()` defaults to `std::env::temp_dir()`, which resolves to `/tmp`. When SQLite attempted to write WAL shared-memory files (`-shm`) or journal files, the kernel returned `EDQUOT` (122), causing SQLite `SQLITE_IOERR_WRITE` / `SQLITE_IOERR_SHMOPEN` (extended code 4874).

#### Resolution & Verification
Executing the test suite with `TMPDIR=$PWD/target/tmp` redirected temporary directory allocations to the NVMe filesystem (`/dev/nvme0n1p3`), where 1.4 TB of free space exists without user quotas. Under this configuration, **100% of all 83 tests passed cleanly**.
This confirmed that the failure was strictly an external environmental quota condition on `/tmp`, not a code regression within `code-kb`. Remediation item P2-7 addresses how integration tests can be hardened against this failure mode.

---

## 3. Explicit Evaluation of Compliance Against the 7 Core Invariants in `AGENTS.md`

`AGENTS.md` defines 7 Core Invariants that govern the architecture, interface contracts, and performance boundaries of `code-kb`. Below is the exhaustive assessment of each invariant against the post-v0.5.1 codebase:

### Invariant 1: Zero Workspace Parameters in Tool Schemas
- **Contract Mandate**: NEVER expose `workspace`, `workspace_id`, `repo_path`, or `root_dir` in any MCP tool schema. Workspace binding must occur via CWD, MCP roots handshake (`initialize`), or path inspection.
- **Status**: **COMPLIANT**
- **Verifiable Code Evidence**:
  - `crates/code-kb-cli/src/mcp/server.rs:112-337` registers 10 tools: `codebase_outline`, `file_skeleton`, `find_symbol`, `search_symbols`, `get_symbol_body`, `get_context_slice`, `find_references`, `find_structural_facts`, `blast_radius`, `replace_symbol_body`.
  - Inspection of all 10 `input_schema` properties confirms 0 workspace parameters.
  - Automated CI assertion in `crates/code-kb-cli/tests/mcp_test.rs:163-175` iterates over all registered tools and asserts `workspace`, `workspace_id`, `repo_path`, and `root_dir` are absent.
  - Dynamic rebind on candidate paths (`server.rs:386-410`) and worktree fast-path auto-copy (`server.rs:423-470`) operate transparently without requiring prompt parameters.

### Invariant 2: Zero In-Memory Heap Objects for Repositories
- **Contract Mandate**: Do not hydrate repository symbol graphs or file lists into RAM. Retained memory must stay `< 15 MB`. All queries must execute as direct, indexed SQLite queries with `open_read_only`.
- **Status**: **COMPLIANT**
- **Verifiable Code Evidence**:
  - `crates/code-kb-core/src/db.rs:26-32` configures SQLite connections with `PRAGMA query_only = ON;`, `PRAGMA cache_size = -4000;` (~4 MB page cache limit), and `PRAGMA mmap_size = 268435456;`.
  - `crates/code-kb-core/src/sync.rs:294-297` utilizes a temporary in-memory SQLite table `_seen` during cold-start reconciliation to track indexed paths rather than accumulating vectors in the Rust heap.
  - SQLite connections are transiently scoped and closed immediately after tool execution.
  - *Outline Memory Bounds (P1-8)*: `load_scoped_outline_symbols` in `queries.rs:146` strictly bounds heavy `Symbol` records via SQLite `:max_slashes`. File paths loaded for directory synthesis in `ops.rs:194` consume < 3.5 MB transient RAM for 50,000 files, safely preserving Invariant 2 while avoiding outline directory truncation.

### Invariant 3: Single-Turn Atomic Edits
- **Contract Mandate**: `replace_symbol_body` must perform pre-flight tree-sitter syntax validation, optional `body_hash` concurrency verification, atomic file replacement, and immediate SQLite re-indexing in a single turn. No two-step preview-and-confirm handshakes.
- **Status**: **COMPLIANT (with 1 P2 concurrency & cleanup fix)**
- **Verifiable Code Evidence**:
  - `crates/code-kb-core/src/edit.rs:67-238` implements the complete sequence:
    1. Pre-flight tree-sitter syntax validation (`edit.rs:156`, `syntax::validate_syntax`).
    2. Optimistic concurrency verification against `expected_body_hash` (`edit.rs:117-135`).
    3. File permissions preservation (`edit.rs:184-186`).
    4. Atomic replacement via `tempfile::Builder::new().tempfile_in(target_dir).persist(&abs_path)` (`edit.rs:169-190`).
    5. Synchronous SQLite re-indexing via `sync::update_file` (`edit.rs:194-206`).
    6. Automated file rollback to original bytes if indexing fails (`edit.rs:207-227`).
  - *Identified Cleanup (P2-3)*: Delete lines 120–127 in `edit.rs` to remove the tautological check (`disk_body_hash == current_sha256`) and enforce strict SHA-256 equality (`expected == current_sha256`), eliminating a concurrency vulnerability.

### Invariant 4: Token-Dense Progressive Disclosure
- **Contract Mandate**: Always return the most compact representation that answers the query. Strip implementation bodies in `file_skeleton`. Include only immediate caller/callee signatures, related types, and test locations in `get_context_slice`.
- **Status**: **COMPLIANT (with 1 P1 callee bug & 1 P3 enum finding)**
- **Verifiable Code Evidence**:
  - `crates/code-kb-core/src/formatters.rs:113-125` replaces function bodies with comments indicating stripped lines (`// ... (N lines hidden) ...`).
  - `crates/code-kb-core/src/formatters.rs:81-94` truncates doc comments to 3 lines with `[+N doc lines]`.
  - `format_symbol_body` and `format_context_slice` format dense, deterministic markdown outputs embedding `body_hash=<sha256>`.
  - *Identified Flaw (P1-5)*: `get_context_slice` in `ops.rs:119-126` silently drops callees whose names are overloaded across multiple types (e.g. `new`, `parse`) due to an unconstrained `get_symbol_by_name` lookup.
  - *Identified Flaw (P3-10)*: Enum variant constructors crowd out function signatures in `ContextSlice` callee listings.

### Invariant 5: Windows Compatibility
- **Contract Mandate**: Windows is a first-class target. Strip Windows verbatim prefixes (`\\?\C:\...`) using `dunce::simplified`. Relative paths and JSON outputs must use explicit forward slashes `/`. SQLite connections and file handles must be closed before file rename or deletion. Path identity verified via canonical paths, not raw string equality.
- **Status**: **COMPLIANT (with 1 P1 path identity bug)**
- **Verifiable Code Evidence**:
  - `dunce::simplified` is systematically applied across path normalization (`workspace.rs:68, 153, 243, 250, 310`).
  - `to_forward_slash` ensures forward slashes in SQLite and outputs (`workspace.rs:72-75`).
  - CRLF line endings are preserved and normalized during atomic edits (`edit.rs:137-145`).
  - Matrix release workflow builds and packages native `x86_64-pc-windows-msvc` binaries (`.github/workflows/release-binaries.yml`).
  - *Identified Violation (P1-3)*: `Workspace::find_workspace_root` returns an uncanonicalized `PathBuf`. When compared via `target_root != self.workspace.canonical_root` in `server.rs:404`, drive-letter case mismatches (`c:\` vs `C:\`) or symlink differences cause repeated rebind loops on Windows.

### Invariant 6: Zero-Friction Tool Ergonomics
- **Contract Mandate**: Parameter aliases accepted (`file`/`path`, `symbol`/`name`, `body`/`code`, `q`/`name`). Optional parameters provide safe defaults (`direction="callers"`). Scoped search supported. Language-agnostic callee filtering. Self-cleaning workspaces. Cross-platform agent hooks.
- **Status**: **PARTIALLY COMPLIANT**
- **Verifiable Code Evidence**:
  - Aliases supported across MCP handlers (`server.rs:389-400, 505-550, 653-665, 815-848`).
  - Default direction is `"callers"` (`server.rs:720`).
  - `find_structural_facts` lists categories and counts when omitted (`server.rs:757-760`).
  - External stdlib callee filtering implemented in `queries.rs:885-915` and `ops.rs:115-118`.
  - Self-cleaning databases at `<root>/.code-kb/artifact.db`.
  - Native `code-kb hook` outputs routing JSON without Node/Python dependencies (`main.rs:246-278`).
  - *Identified Flaw (P1-4)*: `blast_radius_op` in `ops.rs:269-273` uses `else if`, discarding `file` if `symbol` is provided; downstream `queries.rs:1204-1214` omits file seeds from `result.seeds` when symbol seeds exist.
  - *Identified Flaw (P2-4)*: `is_test` (MCP) and `--include-tests` (CLI) lack cross-interface alias support.
  - *Identified Flaw (P3-2)*: `README.md` documents `kind` alias for `find_structural_facts`, but server handler rejects it.

### Invariant 7: Pinned Extractor & Bundled Distribution
- **Contract Mandate**: Pin exact extractor version in `scripts/julie-pins.json` (pinned to `2.42.1`). Build guard in `crates/code-kb-cli/build.rs` verifies presence and version. Single-download distribution packages `code-kb` and `julie-extract` side-by-side. Runtime discovery checks `JULIE_EXTRACT_BIN`, sibling binary directory, `.tools`, and `PATH`.
- **Status**: **COMPLIANT**
- **Verifiable Code Evidence**:
  - `scripts/julie-pins.json` pins `julie-extract` to version `2.42.1`.
  - `crates/code-kb-cli/build.rs:90-151` validates that `julie-extract` is restored and matches version `2.42.1` (bypassed only when `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1`).
  - Runtime discovery in `crates/code-kb-core/src/sync.rs:53-108` searches in order: `JULIE_EXTRACT_BIN` env var, sibling executable directory (`current_exe().parent()`), `.tools/julie-extract`, and system `PATH`.
  - Release workflow (`.github/workflows/release-binaries.yml:109-115`) packages `code-kb` and `julie-extract` side-by-side into tar.gz/zip release archives.

---

## 4. Prioritized Review Findings

### 4.1 P0 Blockers (0 Identified)
No P0 blockers exist in `code-kb` post-v0.5.1. The repository builds cleanly, passes all 83 unit and integration tests, maintains 100% `rustfmt` formatting compliance, has 0 `clippy` warnings under `-D warnings`, and maintains exact byte-for-byte equivalence between `AGENTS.md` and `CLAUDE.md`.

---

### 4.2 P1: Performance, Bugs & Correctness (8 Findings)

---

#### Finding P1-1: SQLite FTS5 Optimizer Join Inversion (623x Slowdown)
- **File & Line Numbers**: `crates/code-kb-core/src/queries.rs:461-464` and `queries.rs:618-621`
- **Severity**: **P1 (High Performance Impact)**
- **Invariant Category**: Invariant 2 (Indexed SQLite efficiency & resource bounds)
- **Root Cause**:
  In `fts_search_symbols_scoped` and `find_related_tests`, the SQL query is constructed as:
  ```sql
  SELECT ...
  FROM symbols_fts
  JOIN symbols s ON s.rowid = symbols_fts.rowid
  WHERE symbols_fts MATCH ?1 AND s.is_test = 0 AND s.test_container = 0
  ORDER BY rank_score ASC LIMIT 20
  ```
  SQLite's cost optimizer lacks cardinality statistics for the FTS5 virtual table `symbols_fts`. When it inspects `symbols s`, it discovers the B-tree index `idx_symbols_test_container (test_container)`. Because the query contains `s.test_container = 0`, SQLite assumes the B-tree index on `s` is cheaper than scanning the virtual table. Consequently, it **inverts the join order**:
  1. It performs a B-tree search on `symbols` across all rows where `test_container = 0` (3,373 out of 3,382 rows in this repository, or 99.7% of the entire table).
  2. For every single row in `symbols`, it evaluates `symbols_fts MATCH ?1` on the virtual table rowid.
  3. This completely defeats the FTS5 inverted index!
- **Reproducible Evidence & Benchmark**:
  Direct `EXPLAIN QUERY PLAN` on `.code-kb/artifact.db`:
  ```text
  QUERY PLAN
  |--SEARCH s USING INDEX idx_symbols_test_container (test_container=?)
  |--SCAN symbols_fts VIRTUAL TABLE INDEX 0:=M3
  `--USE TEMP B-TREE FOR ORDER BY
  ```
  Python benchmark running 100 queries against `.code-kb/artifact.db`:
  - Plain `JOIN`: `8.0229s` (~80.2ms per query).
  - `CROSS JOIN`: `0.0129s` (~0.13ms per query).
  - **Measured Speedup: 623.23x**.
- **Actionable Remediation Specification**:
  In SQLite, the `CROSS JOIN` operator forces the query planner to preserve the exact table order specified in the `FROM` clause, disabling the optimizer's join order inversion.
  In `crates/code-kb-core/src/queries.rs`:
  ```rust
  // Line 461-464:
  -            FROM symbols_fts
  -            JOIN symbols s ON s.rowid = symbols_fts.rowid
  +            FROM symbols_fts
  +            CROSS JOIN symbols s ON s.rowid = symbols_fts.rowid

  // Line 618-621:
  -         FROM symbols_fts
  -         JOIN symbols s ON s.rowid = symbols_fts.rowid
  +         FROM symbols_fts
  +         CROSS JOIN symbols s ON s.rowid = symbols_fts.rowid
  ```

---

#### Finding P1-2: Recursive CTE in `blast_radius` Full Table Scan (48x Slowdown)
- **File & Line Numbers**: `crates/code-kb-core/src/queries.rs:1309-1316`
- **Severity**: **P1 (High Performance Impact)**
- **Invariant Category**: Invariant 2 (Zero in-memory heap, indexed SQLite performance)
- **Root Cause**:
  In `compute_blast_radius`, the recursive CTE `impact_walk` computes multi-hop dependencies. The final select query executes:
  ```sql
  SELECT s.symbol_id, s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container, MIN(iw.depth) as min_depth
  FROM impact_walk iw
  JOIN symbols s ON iw.symbol_id = s.symbol_id
  WHERE iw.depth > 0
  GROUP BY s.symbol_id, s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container
  ORDER BY min_depth ASC, s.path ASC, s.name ASC LIMIT 200
  ```
  Because `impact_walk` is an ephemeral recursive CTE table, SQLite has zero statistics on its row count. It estimates that `symbols` (a fixed physical table) is small and `impact_walk` might be large. Thus, SQLite chooses to scan `symbols` via primary key and construct a transient `AUTOMATIC PARTIAL COVERING INDEX` on `iw` in RAM on every query.
- **Reproducible Evidence & Benchmark**:
  `EXPLAIN QUERY PLAN` on plain `JOIN`:
  ```text
  |--SCAN s USING INDEX sqlite_autoindex_symbols_1
  |--SEARCH iw USING AUTOMATIC PARTIAL COVERING INDEX (symbol_id=?)
  |--USE TEMP B-TREE FOR GROUP BY
  `--USE TEMP B-TREE FOR ORDER BY
  ```
  `EXPLAIN QUERY PLAN` on `CROSS JOIN`:
  ```text
  |--SCAN iw
  |--SEARCH s USING INDEX sqlite_autoindex_symbols_1 (symbol_id=?)
  |--USE TEMP B-TREE FOR GROUP BY
  `--USE TEMP B-TREE FOR ORDER BY
  ```
  Benchmark across 50 runs:
  - Plain `JOIN`: `0.0391s`.
  - `CROSS JOIN`: `0.0008s`.
  - **Measured Speedup: 47.93x**.
- **Actionable Remediation Specification**:
  Change `JOIN symbols s` to `CROSS JOIN symbols s` in `queries.rs:1310-1311`:
  ```rust
  -            FROM impact_walk iw
  -            JOIN symbols s ON iw.symbol_id = s.symbol_id
  +            FROM impact_walk iw
  +            CROSS JOIN symbols s ON iw.symbol_id = s.symbol_id
  ```

---

#### Finding P1-3: `Workspace::find_workspace_root` Uncanonicalized Path Causing Redundant Rebinds
- **File & Line Numbers**: `crates/code-kb-core/src/workspace.rs:177, 202, 226` and `crates/code-kb-cli/src/mcp/server.rs:403-406`
- **Severity**: **P1 (Correctness & Invariant 5 Violation)**
- **Invariant Category**: Invariant 5 (Windows compatibility & path identity)
- **Root Cause**:
  `Workspace::new` canonicalizes `root` and stores it in `self.workspace.canonical_root` via `normalize_path(&dunce::canonicalize(&root)...)`.
  However, `Workspace::find_workspace_root(abs_candidate)` walks up directory components and returns `probe`, `curr`, or `start_dir` directly as an uncanonicalized `PathBuf`.
  In `server.rs:403-406`:
  ```rust
  if let Ok(target_root) = Workspace::find_workspace_root(&abs_candidate) {
      if target_root != self.workspace.canonical_root {
          let _ = self.bind_workspace(&target_root);
      }
  }
  ```
  On Windows (where drive letter casing can vary, e.g. `c:\foo` vs `C:\foo`) or on filesystems with symlinked workspace directories, `target_root != self.workspace.canonical_root` evaluates to `true` on **every single tool call**. This invokes `bind_workspace`, re-discovering the workspace, dropping the telemetry database handle, and re-opening SQLite repeatedly.
- **Reproducible Evidence & Logical Proof**:
  Invariant 5 explicitly states:
  *"Verify path identity using canonical paths and dunce::simplified, not by raw case-sensitive string matching."*
  Comparing `target_root != self.workspace.canonical_root` directly compares an uncanonicalized `PathBuf` with a canonicalized `PathBuf`.
- **Actionable Remediation Specification**:
  Canonicalize `target_root` inside `Workspace::find_workspace_root` before returning, and simplify the comparison in `server.rs`:
  ```rust
  // In crates/code-kb-core/src/workspace.rs:
  // Canonicalize final result in find_workspace_root:
  pub fn find_workspace_root(start: &Path) -> Result<PathBuf, WorkspaceError> {
      ...
      let raw_root = if probe.join(".code-kb").exists() || probe.join(".git").exists() {
          probe
      } else if ... {
          curr
      } else {
          start_dir
      };
      let canon = dunce::canonicalize(&raw_root).unwrap_or(raw_root);
      Ok(normalize_path(&canon))
  }

  // In crates/code-kb-cli/src/mcp/server.rs:404:
  if let Ok(target_root) = Workspace::find_workspace_root(&abs_candidate) {
      let target_norm = code_kb_core::normalize_path(&dunce::canonicalize(&target_root).unwrap_or(target_root));
      if target_norm != self.workspace.canonical_root {
          let _ = self.bind_workspace(&target_norm);
      }
  }
  ```

---

#### Finding P1-4: `blast_radius_op` and Downstream Queries Discard `file` Seeds when `symbol` is Provided
- **File & Line Numbers**: `crates/code-kb-core/src/ops.rs:269-274` and `crates/code-kb-core/src/queries.rs:1204-1221`
- **Severity**: **P1 (Logic Bug / Parameter Dropping)**
- **Invariant Category**: Invariant 6 (Zero-friction tool ergonomics & blast radius prediction)
- **Root Cause**:
  1. **Seed Collection in `ops.rs:269-274`**:
     ```rust
     if let Some(s) = clean_symbol {
         seed_symbols.push(s);
     } else if let Some(ref f) = clean_file {
         seed_paths.push(f.as_str());
     } else {
         // zero arguments: discover git status
     ```
     Both MCP (`blast_radius(symbol, file)`) and CLI (`code-kb blast-radius [symbol] --file <file>`) allow the user to provide both a seed symbol and a seed file. The underlying engine `queries::compute_blast_radius(conn, &seed_symbols, &seed_paths, ...)` accepts both slices simultaneously. However, because line 271 uses `else if`, whenever `clean_symbol` is present, `clean_file` is **silently ignored and discarded**.
  2. **Downstream Seed Reporting in `queries.rs:1204-1221`**:
     Even when both slices are passed to `queries::compute_blast_radius`, lines 1204–1214 populate `result.seeds` and set `seed_type` using another mutually exclusive `if / else if` check:
     ```rust
     let seed_type = if !seed_symbols.is_empty() {
         for s in seed_symbols {
             seeds.push(s.to_string());
         }
         "symbol".to_string()
     } else if !seed_paths.is_empty() {
         for p in seed_paths {
             seeds.push(p.to_string());
         }
         "file".to_string()
     } else { ... };
     ```
     If both `seed_symbols` and `seed_paths` are present, `result.seeds` captures only the symbol seeds, sets `seed_type = "symbol"`, and discards all file paths from the returned seed list, even though the query where-clauses below handle both.
- **Reproducible Evidence**:
  Calling `code-kb blast-radius Workspace --file crates/code-kb-core/src/workspace.rs` results in `seed_paths` being omitted in `ops.rs`, and downstream in `queries.rs:1204-1214`, `result.seeds` only reports `["Workspace"]` with `seed_type = "symbol"`.
- **Actionable Remediation Specification**:
  1. In `crates/code-kb-core/src/ops.rs:269-274`, decouple the two `if` checks so both seeds are collected:
     ```rust
     // crates/code-kb-core/src/ops.rs:269-274:
     -    if let Some(s) = clean_symbol {
     -        seed_symbols.push(s);
     -    } else if let Some(ref f) = clean_file {
     -        seed_paths.push(f.as_str());
     -    } else {
     +    if let Some(s) = clean_symbol {
     +        seed_symbols.push(s);
     +    }
     +    if let Some(ref f) = clean_file {
     +        seed_paths.push(f.as_str());
     +    }
     +    if seed_symbols.is_empty() && seed_paths.is_empty() {
     ```
  2. In `crates/code-kb-core/src/queries.rs:1204-1221`, support mixed seed inputs by collecting both symbols and file paths into `seeds` and assigning `"mixed"`:
     ```rust
     // crates/code-kb-core/src/queries.rs:1204-1221:
     let seed_type = if !seed_symbols.is_empty() && !seed_paths.is_empty() {
         for s in seed_symbols {
             seeds.push(s.to_string());
         }
         for p in seed_paths {
             seeds.push(p.to_string());
         }
         "mixed".to_string()
     } else if !seed_symbols.is_empty() {
         for s in seed_symbols {
             seeds.push(s.to_string());
         }
         "symbol".to_string()
     } else if !seed_paths.is_empty() {
         for p in seed_paths {
             seeds.push(p.to_string());
         }
         "file".to_string()
     } else {
         return Ok(BlastRadiusResult {
             seed_type: "none".to_string(),
             seeds: Vec::new(),
             likely_tests: Vec::new(),
             impacted_symbols: Vec::new(),
         });
     };
     ```

---

#### Finding P1-5: Context Slice Callee Resolution Silently Drops Ambiguous Methods via N+1 Queries
- **File & Line Numbers**: `crates/code-kb-core/src/ops.rs:119-126` and `crates/code-kb-core/src/queries.rs:964-980`
- **Severity**: **P1 (Data Loss / Incomplete Context Slice)**
- **Invariant Category**: Invariant 4 (Token-dense progressive disclosure)
- **Root Cause**:
  In `queries::find_references_for_symbol` (`queries.rs:975`), the SQLite query joins `symbols s_to ON r.to_symbol_id = s_to.symbol_id`. At this point, the exact callee `symbol_id`, `signature`, `path`, and `start_line` are fully resolved. However, the query maps this into `ReferenceSite`, discarding everything except `to_symbol_name = s_to.name`.
  Then, in `ops.rs:119-126`:
  ```rust
  for c in callees {
      if let Ok(Some(s)) = queries::get_symbol_by_name(conn, &c.to_symbol_name, None) {
          let sig = s.signature.unwrap_or(s.name);
          let entry = format!("{sig} ({}:{})", s.path, s.start_line);
          if !callee_signatures.contains(&entry) {
              callee_signatures.push(entry);
          }
      }
  }
  ```
  `get_symbol_by_name` performs a workspace-wide search for that symbol name with no path filter. Whenever `c.to_symbol_name` is a commonly overloaded method (e.g. `new`, `parse`, `format`, `run`, `validate`, `len`), `get_symbol_by_name` returns `Err(QueryError::AmbiguousSymbol)`.
  Because `if let Ok(Some(s))` matches only on `Ok`, **the ambiguous callee is silently discarded!**
  Furthermore, executing `get_symbol_by_name` in a loop performs up to 20 redundant queries (N+1 query problem).
- **Reproducible Evidence**:
  Calling `code-kb slice replace_symbol_body` on a function that calls `Workspace::new` or `String::new`: the call to `new` is omitted from `Dependencies (Signatures)` because `new` exists in 5+ files across the repository.
- **Actionable Remediation Specification**:
  Update `ReferenceSite` or add a specialized query that returns the already-joined signature and location directly from `s_to`:
  ```sql
  SELECT s_to.name, s_to.signature, s_to.path, s_to.start_line, s_to.kind
  FROM relationships r
  JOIN symbols s_from ON r.from_symbol_id = s_from.symbol_id
  JOIN symbols s_to ON r.to_symbol_id = s_to.symbol_id
  WHERE s_from.name = ?1 AND (?3 IS NULL OR r.from_symbol_id = ?3)
  LIMIT ?2
  ```
  Populate `callee_signatures` directly from the single query result in O(1), completely eliminating `get_symbol_by_name` and preventing dropped ambiguous methods.

---

#### Finding P1-6: Watcher Thread Panic Risk on Root Errors & Synchronous Blocking Callback
- **File & Line Numbers**: `crates/code-kb-core/src/watcher.rs:60-61, 101-104, 114-128`
- **Severity**: **P1 (Reliability & Thread Safety)**
- **Invariant Category**: Invariant 6 (Filesystem watcher resiliency)
- **Root Cause**:
  1. **Thread Panic on Root Matcher Failure**:
     ```rust
     let mut ignore_matcher = ignore_builder
         .build_matchers()
         .pop()
         .expect("workspace root creates one ignore matcher");
     ```
     If the workspace directory permissions change, a worktree is deleted during a session, or the root path becomes inaccessible, `build_matchers()` returns an empty `Vec`. Calling `.pop().expect(...)` triggers an immediate panic inside the background thread spawned by `new_debouncer`. The watcher thread crashes silently while the main MCP process remains running, permanently disabling file event monitoring.
  2. **Synchronous Blocking Callback**:
     The debouncer callback runs synchronously on the watcher thread. If `relevant_files.len() > 50`, line 106 invokes `scan_workspace(&ws_clone, &db_clone, false)` synchronously, which executes `julie-extract scan` (taking 1–4 seconds). During this interval, the thread cannot process incoming events, leading to kernel inotify buffer overflows.
  3. **Sequential Process Spawning & Single-File CLI Constraint**:
     For batches of 10–50 files, lines 114–128 execute up to 50 sequential calls to `update_file`, each spawning an external child process `julie-extract update` with individual process creation and SQLite write transaction overhead.
     Crucially, `.tools/julie-extract update` strictly requires a single `--file <FILE>` argument (it does not accept multiple files or a batch flag). Only `julie-extract scan` operates across multiple files.
- **Actionable Remediation Specification**:
  1. Replace `.expect(...)` with safe error logging and fallback:
     ```rust
     let mut ignore_matcher = match ignore_builder.build_matchers().pop() {
         Some(m) => m,
         None => {
             warn!("Failed to initialize ignore matcher for workspace root; skipping tick");
             return;
         }
     };
     ```
  2. For storm recovery (> 50 files) or multi-file batches, eliminate debouncer thread blocking and respect the CLI constraint:
     - Because `julie-extract update` takes only a single `--file <FILE>` argument, multi-file batches cannot be dispatched via a CLI batch flag.
     - Multi-file updates must either:
       a. Trigger `scan_workspace(&ws_clone, &db_clone, false)` (which invokes `julie-extract scan` once to process all modified workspace files in a single unified process), OR
       b. Offload individual `update_file` invocations to an asynchronous background worker channel/queue so the `notify-debouncer-full` event loop is never blocked.

---

#### Finding P1-7: Unconditional Full-File Read & Hash in `reconcile_offline_edits`
- **File & Line Numbers**: `crates/code-kb-core/src/sync.rs:360-379`
- **Severity**: **P1 (Cold Start I/O & Startup Latency)**
- **Invariant Category**: Invariant 2 (Zero in-memory heap & fast startup)
- **Root Cause**:
  During startup offline reconciliation, `reconcile_offline_edits` inspects all files on disk. Lines 366-377 read:
  ```rust
  let hash_matches = match std::fs::read(path) {
      Ok(content) => compute_content_hash_matches(&content, &stored_hash),
      Err(e) => { ... true }
  };

  if indexed_bytes != bytes || !hash_matches {
      report.modified.push(rel_str);
  }
  ```
  `indexed_bytes` is the size from SQLite; `bytes` is the metadata file size from disk. If `indexed_bytes != bytes`, the file is **guaranteed to be modified**. However, line 366 unconditionally reads the entire file into heap RAM (`std::fs::read(path)`) and computes its SHA-256 hash *before* checking `indexed_bytes != bytes`!
- **Contrast with `ensure_fresh_file` (`sync.rs:248-253`)**:
  `ensure_fresh_file` correctly implements short-circuiting:
  ```rust
  if f.content_bytes != disk_bytes {
      true
  } else {
      let disk_content = std::fs::read(&abs_path)?;
      !compute_content_hash_matches(&disk_content, &f.content_hash)
  }
  ```
- **Actionable Remediation Specification**:
  Short-circuit the disk read in `reconcile_offline_edits`:
  ```rust
  // crates/code-kb-core/src/sync.rs:366-378:
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

  if is_modified {
      report.modified.push(rel_str);
  }
  ```

---

#### Finding P1-8: `codebase_outline_op` Directory Synthesis Architecture & Memory Bounds
- **File & Line Numbers**: `crates/code-kb-core/src/ops.rs:192-221` and `crates/code-kb-core/src/formatters.rs:173-205`
- **Severity**: **P1 (Memory Bounds & Architectural Clarification)**
- **Invariant Category**: Invariant 2 (Retained memory < 15 MB)
- **Root Cause & Architectural Context**:
  1. **Directory Synthesis Mechanism in `add_path_to_outline`**:
     In `code-kb`'s SQLite schema, there is no `directories` table; directory nodes in `OutlineNode` are synthesized on the fly from file path prefixes inside `add_path_to_outline` (`crates/code-kb-core/src/formatters.rs:173-205`). As each file path is processed, `add_path_to_outline` splits the path on `/` and inserts intermediate directory entries into `curr.subdirs` up to `max_depth`. For example, processing `crates/code-kb-core/src/lib.rs` creates the `crates/` directory entry when `max_depth >= 1`.
  2. **Severe Functional Regression of Depth-Limiting `SELECT path FROM files`**:
     It was previously proposed to push the slash limit down into the files query via:
     `AND (length(path) - length(replace(path, '/', '')) <= :max_slashes)`
     However, doing so introduces a **critical functional regression**:
     In a repository where top-level directories do not contain direct root-level files (such as `.github/`, `crates/`, `docs/`, `scripts/`, `skills/`, `.claude-plugin/`), no file with 0 slashes exists in those directory trees. For `code-kb outline --depth 1` (`max_slashes = 0`), filtering `SELECT path FROM files` by `max_slashes` returns only files residing directly in the repository root (e.g. `Cargo.toml`, `README.md`). Because no file path starting with `crates/` is returned, `add_path_to_outline` never synthesizes the `crates/` directory node! Running `code-kb outline --depth 1` would render **zero directories**, completely hiding the repository structure.
  3. **Memory Bounding Reality**:
     - The memory-intensive records in `code-kb` are `Symbol` structs (which contain name, kind, signature, path, and doc comments). In `codebase_outline_op`, `load_scoped_outline_symbols` (`queries.rs:146`) **already strictly bounds** memory by filtering symbols in SQLite using `(length(path) - length(replace(path, '/', '')) <= :max_slashes)`.
     - Raw file path strings in `SELECT path FROM files` are extremely lightweight: in a large repository with 50,000 files, 50,000 relative path strings average ~70 bytes each, consuming less than 3.5 MB of transient heap RAM during outline construction. This transient memory is immediately deallocated when `codebase_outline_op` returns, remaining well within Invariant 2's `< 15 MB` retained memory ceiling.
- **Actionable Remediation Specification**:
  1. Do **NOT** apply slash depth filtering to `SELECT path FROM files`. All file paths must be retained to allow `add_path_to_outline` to correctly synthesize directory hierarchy at any requested depth.
  2. Retain the slash depth filter exclusively in `load_scoped_outline_symbols` (`queries.rs:146`), where it successfully bounds heavy `Symbol` allocations.
  3. If strict file path bounding is ever required in the future for million-file monorepos, it must be achieved by querying distinct directory prefixes (`SELECT DISTINCT ...`) rather than filtering out paths of deeper files.

---

### 4.3 P2: Dead Code, Technical Debt & Edge Case Flaws (12 Findings)

---

#### Finding P2-1: Malformed Diagnostic Error String on `SymbolNotFound` with Suggestions
- **File & Line Numbers**: `crates/code-kb-core/src/queries.rs:14-15, 855-857` and `crates/code-kb-core/src/ops.rs:16-17, 74-76`
- **Severity**: **P2 (Diagnostic Formatting Defect)**
- **Invariant Category**: Invariant 6 (Zero-friction tool ergonomics)
- **Root Cause**:
  `QueryError::SymbolNotFound` is declared as:
  ```rust
  #[error("Symbol '{0}' not found")]
  SymbolNotFound(String),
  ```
  When near-match suggestions are formatted in `queries.rs:855-857`:
  ```rust
  Err(QueryError::SymbolNotFound(format!(
      "{symbol_name}'. Did you mean one of:\n{list}"
  )))
  ```
  This passes the entire formatted suggestion block into `{0}`. When rendered, the error message becomes:
  `"Symbol 'my_symbol'. Did you mean one of:\n  - ...' not found"`
  Notice the misplaced opening quote and the trailing `' not found` at the very end. The exact same error exists in `ops.rs:75`.
- **Actionable Remediation Specification**:
  Define a dedicated error variant:
  ```rust
  #[error("Symbol '{0}' not found. Did you mean one of:\n{1}")]
  SymbolNotFoundWithSuggestions(String, String),
  ```
  or format cleanly without the trailing `' not found` appendage.

---

#### Finding P2-2: `replace_symbol_body` Maps Workspace Path Errors to `SymbolNotFound`
- **File & Line Numbers**: `crates/code-kb-core/src/edit.rs:77-78`
- **Severity**: **P2 (Error Taxonomy Defect)**
- **Invariant Category**: Invariant 3 (Single-turn atomic edits)
- **Root Cause**:
  In `crates/code-kb-core/src/edit.rs`:
  ```rust
  let (effective_abs, rel_path) = workspace
      .resolve_path(Path::new(file_path))
      .map_err(|e| EditError::SymbolNotFound(symbol_name.to_string(), e.to_string()))?;
  ```
  If `workspace.resolve_path` fails (e.g. because the path is outside the workspace root or path traversal is attempted), the error is converted into `EditError::SymbolNotFound`. The resulting error message is:
  `"Symbol 'my_fn' not found in 'Path '/...' is outside workspace root '...''"`, falsely claiming the symbol is missing rather than reporting an unauthorized or invalid path.
- **Actionable Remediation Specification**:
  Add `Workspace(#[from] WorkspaceError)` to `EditError`:
  ```rust
  // In crates/code-kb-core/src/edit.rs:
  #[derive(Debug, Error)]
  pub enum EditError {
      ...
      #[error("Workspace path error: {0}")]
      Workspace(#[from] WorkspaceError),
  ```
  Replace line 78 with `.map_err(EditError::Workspace)?;`.

---

#### Finding P2-3: Tautological Optimistic Lock Fallback in `replace_symbol_body` & Concurrency Vulnerability
- **File & Line Numbers**: `crates/code-kb-core/src/edit.rs:120-127`
- **Severity**: **P2 (Dead Code & Concurrency Vulnerability)**
- **Invariant Category**: Invariant 3 (Single-turn atomic edits)
- **Root Cause**:
  1. **Tautological Identity Check**:
     In `crates/code-kb-core/src/edit.rs:114-127`:
     ```rust
     114: let current_sha256 = hash_content(existing_body);
     ...
     117: if let Some(expected) = expected_body_hash {
     118:     let mut matches = expected == current_sha256;
     122:     if !matches && symbol.body_hash.as_deref().is_some_and(|h| h == expected) {
     123:         let disk_body_hash = hash_content(existing_body);
     124:         if disk_body_hash == current_sha256 {
     125:             matches = true;
     126:         }
     127:     }
     ```
     `disk_body_hash` and `current_sha256` are identical SHA-256 hashes of the exact same byte slice `existing_body`. Thus, line 124 (`disk_body_hash == current_sha256`) is an identity check (`X == X`) that evaluates to `true` 100% of the time.
  2. **Concurrency Vulnerability of Fallback**:
     If lines 123–126 are simplified to `if !matches && symbol.body_hash.as_deref().is_some_and(|h| h == expected) { matches = true; }`, a severe concurrency vulnerability is introduced: if a file on disk is modified after indexing, `expected == current_sha256` will be false, but the edit would still proceed because `symbol.body_hash == expected` in SQLite was not yet re-indexed. This completely defeats optimistic concurrency locking.
  3. **Hash Representation Alignment**:
     In `code-kb`, both `format_symbol_body` (`formatters.rs:250`) and `format_context_slice` (`formatters.rs:272`) emit SHA-256 hashes generated by `hash_content` (`body_hash=<64-char-sha256>`). Callers pass this exact SHA-256 hash as `expected_body_hash`. The 32-character index hash from `julie-extract` stored in SQLite `symbols.body_hash` is never emitted to agents in tool output.
- **Actionable Remediation Specification**:
  Delete lines 120–127 entirely in `crates/code-kb-core/src/edit.rs` and enforce strict equality `expected == current_sha256`:
  ```rust
  // crates/code-kb-core/src/edit.rs:116-126:
  // Verify optimistic lock if caller specified expected_body_hash
  if let Some(expected) = expected_body_hash {
      if expected != current_sha256 {
          return Err(EditError::HashMismatch(
              expected.to_string(),
              current_sha256,
          ));
      }
  }
  ```
  This removes the dead tautological check, closes the concurrency loophole, and ensures strict optimistic concurrency locking.

---

#### Finding P2-4: Parameter Name Inconsistency: `is_test` (MCP) vs `--include-tests` (CLI)
- **File & Line Numbers**: `crates/code-kb-cli/src/mcp/server.rs:163-166, 556-559, 626-628` and `crates/code-kb-cli/src/main.rs:109-110`
- **Severity**: **P2 (CLI & MCP Asymmetry)**
- **Invariant Category**: Invariant 6 (Zero-friction tool ergonomics)
- **Root Cause**:
  - In `find_symbol` and `search_symbols`, the MCP schema exposes `is_test`.
  - The MCP server handler in `server.rs:557, 627` checks `arguments.get("is_test")`. Passing `include_tests` over MCP is ignored.
  - The CLI `symbol` and `search` commands expose `--include-tests`. Passing `--is-test` on the CLI results in a Clap argument parse error.
- **Actionable Remediation Specification**:
  - In `server.rs`: accept both `is_test` and `include_tests`:
    ```rust
    let include_tests = arguments.get("is_test")
        .or_else(|| arguments.get("include_tests"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    ```
  - In `main.rs`: add `#[arg(long, alias = "is-test")]` to `include_tests`.

---

#### Finding P2-5: Missing CI and Pre-flight Sync Check for Duplicate `SKILL.md` Files
- **File & Line Numbers**: `skills/code-kb/SKILL.md`, `.claude-plugin/skills/code-kb/SKILL.md`, and `.github/workflows/ci.yml:28-30`
- **Severity**: **P2 (CI Blindspot / Sync Contract)**
- **Invariant Category**: Invariant 7 & Sync Contract
- **Root Cause**:
  Two duplicate `SKILL.md` files exist: `skills/code-kb/SKILL.md` and `.claude-plugin/skills/code-kb/SKILL.md`. While both files currently match byte-for-byte, `.github/workflows/ci.yml` and `scripts/release-preflight.sh` only assert `cmp AGENTS.md CLAUDE.md`. There is no automated test or CI check preventing the two `SKILL.md` files from drifting out of sync.
- **Actionable Remediation Specification**:
  Add a synchronization check to `.github/workflows/ci.yml` and `scripts/release-preflight.sh`:
  ```bash
  cmp skills/code-kb/SKILL.md .claude-plugin/skills/code-kb/SKILL.md
  ```

---

#### Finding P2-6: Agent Metadata Directories (`.agents`, `.razorback`) Omitted from `is_hard_excluded`
- **File & Line Numbers**: `crates/code-kb-core/src/workspace.rs:81-108` and `/home/murphy/source/code-kb/.gitignore:1-29`
- **Severity**: **P2 (Operational Hygiene & Watcher Noise)**
- **Invariant Category**: Invariant 6 (Filesystem watcher resilience)
- **Root Cause**:
  `workspace.rs:81-108` hard-excludes directories like `.git`, `.code-kb`, `.memories`, `.worktrees`, and `.claude`.
  However, neither `.agents` nor `.razorback` is included in `is_hard_excluded`. Furthermore, while `.razorback/` is in `.gitignore`, `.agents/` is omitted from `.gitignore`.
  When coding agents run inside the workspace, writing metadata to `.agents/` triggers file watcher events, leading to unnecessary re-indexing cycles.
- **Actionable Remediation Specification**:
  1. Add `".agents" | ".razorback"` to `is_hard_excluded` in `workspace.rs:84`.
  2. Add `.agents/` to `.gitignore`.

---

#### Finding P2-7: Integration Test Suite Dependency on Unconstrained `/tmp`
- **File & Line Numbers**: `crates/code-kb-cli/tests/cli_test.rs:4` and `crates/code-kb-cli/tests/mcp_test.rs:7`
- **Severity**: **P2 (Test Suite Fragility)**
- **Invariant Category**: Invariant 5 (Platform resilience)
- **Root Cause**:
  `setup_test_repo()` in `cli_test.rs` and `mcp_test.rs` calls `tempfile::tempdir().unwrap()`. On multi-user or shared CI Linux systems where `/tmp` has a tight tmpfs quota, all SQLite tests panic with `EDQUOT` (os error 122).
- **Actionable Remediation Specification**:
  Implement a safe tempdir helper that checks `CARGO_TARGET_TMPDIR` or local workspace directory fallback:
  ```rust
  pub fn safe_tempdir() -> tempfile::TempDir {
      if let Ok(target_tmp) = std::env::var("CARGO_TARGET_TMPDIR") {
          tempfile::TempDir::new_in(target_tmp).unwrap()
      } else {
          tempfile::tempdir().unwrap()
      }
  }
  ```

---

#### Finding P2-8: Dead Code in `file_stem` Trimming and SQL Preparation in Loop
- **File & Line Numbers**: `crates/code-kb-core/src/queries.rs:1367-1375, 1401-1405`
- **Severity**: **P2 (Dead Code & Micro-Performance)**
- **Invariant Category**: Invariant 3 (Clean, minimal implementation)
- **Root Cause**:
  1. `Path::file_stem()` already strips file extensions (`.rs`, `.ts`, `.py`, `.go`). Trimming them again via `.trim_end_matches(".rs")` in lines 1369-1372 is dead code.
  2. In lines 1401-1403, `conn.prepare("SELECT DISTINCT path FROM files...")` is called inside `for stem in file_stems` on every loop iteration instead of being prepared once outside the loop.
- **Actionable Remediation Specification**:
  Remove the redundant `.trim_end_matches` calls and hoist `conn.prepare` above the loop.

---

#### Finding P2-9: Dead Function `get_log_file` Marked `#[allow(dead_code)]`
- **File & Line Numbers**: `crates/code-kb-cli/src/logging.rs:14-17`
- **Severity**: **P2 (Dead Code)**
- **Invariant Category**: Invariant 3 (Minimal surface area)
- **Root Cause**:
  `get_log_file` is annotated with `#[allow(dead_code)]` and is never referenced anywhere in `code-kb-cli` or `code-kb-core`.
- **Actionable Remediation Specification**:
  Delete lines 14-17 from `crates/code-kb-cli/src/logging.rs`.

---

#### Finding P2-10: Unused Public Export `fts_search_symbols`
- **File & Line Numbers**: `crates/code-kb-core/src/queries.rs:357-365` and `crates/code-kb-core/src/lib.rs:33`
- **Severity**: **P2 (Dead Export / API Surface)**
- **Invariant Category**: Invariant 3 (Minimal surface area)
- **Root Cause**:
  `fts_search_symbols` is a thin wrapper over `fts_search_symbols_scoped(conn, query, kind, None, ...)`. It is not called anywhere in production code; all callers invoke `fts_search_symbols_scoped` directly.
- **Actionable Remediation Specification**:
  Deprecate or remove `fts_search_symbols` and standardize all callers and tests on `fts_search_symbols_scoped`.

---

#### Finding P2-11: Cargo.toml Dependency Duplication and Non-Workspace Version
- **File & Line Numbers**: `crates/code-kb-core/Cargo.toml:26, 36` and `crates/code-kb-cli/Cargo.toml:28`
- **Severity**: **P2 (Cargo Hygiene)**
- **Invariant Category**: Invariant 7 (Workspace single-source-of-truth)
- **Root Cause**:
  - In `crates/code-kb-core/Cargo.toml`, `tempfile = { workspace = true }` is declared under `[dependencies]` (line 26) AND under `[dev-dependencies]` (line 36).
  - In `crates/code-kb-cli/Cargo.toml`, `tempfile = "3.17"` hardcodes a version rather than inheriting `{ workspace = true }`.
- **Actionable Remediation Specification**:
  - Remove duplicate `tempfile` under `[dev-dependencies]` in `crates/code-kb-core/Cargo.toml`.
  - Replace `tempfile = "3.17"` with `tempfile = { workspace = true }` in `crates/code-kb-cli/Cargo.toml`.

---

#### Finding P2-12: Watcher Inotify Queue Flooding on Recursive Ignored Directories
- **File & Line Numbers**: `crates/code-kb-core/src/watcher.rs:133`
- **Severity**: **P2 (OS Resource Bounds)**
- **Invariant Category**: Invariant 6 (Filesystem watcher resilience)
- **Root Cause**:
  Line 133 executes `debouncer.watch(&workspace.canonical_root, RecursiveMode::Recursive)?;`.
  On Linux, inotify recursively adds watches to all directories, including `target/`, `.git/`, and `node_modules/`. During large builds (`cargo build`) or checkouts, tens of thousands of filesystem events are generated in `target/`. While `is_hard_excluded` drops them in the debouncer callback, the kernel inotify queue can overflow (`IN_Q_OVERFLOW`) before the events reach the callback.
- **Actionable Remediation Specification**:
  Document this kernel-level limitation or selectively watch top-level directories excluding `target/` and `.git/`.

---

### 4.4 P3: Documentation, CLI Polish & Minor Ergonomics (11 Findings)

---

#### Finding P3-1: `README.md` Documents Incorrect Default Depth (`3` vs `2`) and Property Name (`path` vs `file`)
- **File & Line Numbers**: `README.md:202`
- **Severity**: **P3 (Documentation Drift)**
- **Root Cause**:
  Line 202 states:
  `| blast_radius | ... | symbol (opt), path (opt), depth (opt, def: 3), limit (opt) | name, file, impact |`
  In actual code:
  1. The default depth is `2` (`server.rs:301`, `main.rs:176`).
  2. The primary MCP schema property name is `file`, not `path` (`server.rs:295-298`).
- **Actionable Remediation Specification**:
  Update `README.md:202` to:
  `symbol (opt), file (opt), depth (opt, def: 2), limit (opt)`

---

#### Finding P3-2: `README.md` Documents Ghost `kind` Alias for `find_structural_facts`
- **File & Line Numbers**: `README.md:203` and `crates/code-kb-cli/src/mcp/server.rs:747-752`
- **Severity**: **P3 (Documentation Drift)**
- **Root Cause**:
  `README.md:203` lists `kind` under Aliases for `find_structural_facts`.
  In `server.rs:747-752`, the code checks `cat`, `type`, and `pattern`, but does **not** check `kind`. If an agent passes `kind="route"`, it is ignored and the tool falls back to listing categories.
- **Actionable Remediation Specification**:
  Add `.or_else(|| arguments.get("kind"))` to `crates/code-kb-cli/src/mcp/server.rs:751`.

---

#### Finding P3-3: `docs/site/index.html` Displays Stale Version `v0.5.0`
- **File & Line Numbers**: `docs/site/index.html:19, 37, 256`
- **Severity**: **P3 (Showcase Website Drift)**
- **Root Cause**:
  Lines 19 and 37 display `v0.5.0` and `v0.5.0 Released`, and line 256 refers to `code-kb (v0.5)`. The active workspace version is `0.5.1`.
- **Actionable Remediation Specification**:
  Update version occurrences in `docs/site/index.html` from `v0.5.0` to `v0.5.1`.

---

#### Finding P3-4: `TODO.md` Contains Stale References to `v0.5.0`
- **File & Line Numbers**: `TODO.md:7, 10-12`
- **Severity**: **P3 (Documentation Drift)**
- **Root Cause**:
  Lines 10-12 state:
  `- [ ] Crates.io publication: Publish code-kb-core v0.5.0... Publish code-kb-cli v0.5.0`
  The current version is `0.5.1`.
- **Actionable Remediation Specification**:
  Update `TODO.md` to reference `v0.5.1`.

---

#### Finding P3-5: `docs/plans/011` References Deprecated `code-kb prune` Command
- **File & Line Numbers**: `docs/plans/011-miller-julie-comparative-analysis-and-lessons.md:26, 87`
- **Severity**: **P3 (Historical Plan Drift)**
- **Root Cause**:
  Plan 011 refers to `code-kb prune` for cleaning up worktrees. Plan 012 deliberately deprecated `code-kb prune` in favor of self-cleaning in-tree databases at `<root>/.code-kb/artifact.db`.
- **Actionable Remediation Specification**:
  Add an explanatory note to Plan 011 indicating that `code-kb prune` was replaced by self-cleaning in-tree databases.

---

#### Finding P3-6: Missing Local `cargo test` Integration Test for `AGENTS.md == CLAUDE.md`
- **File & Line Numbers**: `crates/code-kb-cli/tests/cli_test.rs`
- **Severity**: **P3 (Developer Ergonomics)**
- **Root Cause**:
  The sync contract is verified in CI (`ci.yml`) and `scripts/release-preflight.sh`, but no test runs during local `cargo test`.
- **Actionable Remediation Specification**:
  Add a test to `crates/code-kb-cli/tests/cli_test.rs`:
  ```rust
  #[test]
  fn test_agents_and_claude_md_sync_contract() {
      let agents = std::fs::read_to_string("../../AGENTS.md").unwrap();
      let claude = std::fs::read_to_string("../../CLAUDE.md").unwrap();
      assert_eq!(agents, claude, "AGENTS.md and CLAUDE.md must be byte-for-byte identical");
  }
  ```

---

#### Finding P3-7: `code-kb edit` Subcommand Ignores Global `--json` Flag
- **File & Line Numbers**: `crates/code-kb-cli/src/main.rs:556-568`
- **Severity**: **P3 (CLI Parity)**
- **Root Cause**:
  Every query subcommand in `main.rs` supports `if cli.json` for machine-readable output. `Command::Edit` ignores `cli.json` and unconditionally prints human-readable text.
- **Actionable Remediation Specification**:
  Add `if cli.json` handling to `Command::Edit`:
  ```rust
  if cli.json {
      println!("{}", serde_json::to_string_pretty(&res)?);
  } else {
      println!("{}", format_replace_symbol_result(&res));
  }
  ```

---

#### Finding P3-8: Schema Parameter Naming Asymmetry in `blast_radius` (`symbol`/`file`) vs Other Tools
- **File & Line Numbers**: `crates/code-kb-cli/src/mcp/server.rs:288-308`
- **Severity**: **P3 (Schema Uniformity)**
- **Root Cause**:
  `file_skeleton`, `get_symbol_body`, `get_context_slice`, and `replace_symbol_body` use primary schema names `symbol_name` and `file_path`. `blast_radius` uses `symbol` and `file`.
- **Actionable Remediation Specification**:
  Ensure both naming conventions are fully supported via aliases and documented consistently.

---

#### Finding P3-9: Positional CLI Arguments Lack Named Option Alternatives in `outline` and `facts`
- **File & Line Numbers**: `crates/code-kb-cli/src/main.rs:84-90, 182-190`
- **Severity**: **P3 (CLI Ergonomics)**
- **Root Cause**:
  `code-kb outline` only takes a positional `[PATH]`, and `code-kb facts` only takes a positional `[CATEGORY]`. Running `code-kb outline --path src` or `code-kb facts --category route` errors out.
- **Actionable Remediation Specification**:
  Add named option flags in Clap structs: `#[arg(long)] path: Option<String>` and `#[arg(long)] category: Option<String>`.

---

#### Finding P3-10: Enum Variant Constructors Crowd Out Function Signatures in `ContextSlice`
- **File & Line Numbers**: `crates/code-kb-core/src/ops.rs:119-130`
- **Severity**: **P3 (Context Quality)**
- **Root Cause**:
  When extracting callees for a function, enum variant constructors (e.g. `InvalidOffsetRange`, `HashMismatch`) consume 3–4 of the 10 available callee slots, crowding out actual function callees.
- **Actionable Remediation Specification**:
  Filter out `kind == "variant"` or prioritize functions and methods before enum variants when constructing `callee_signatures`.

---

#### Finding P3-11: Unescaped LIKE Wildcards in `compute_blast_radius` Queries
- **File & Line Numbers**: `crates/code-kb-core/src/queries.rs:1241, 1400-1402`
- **Severity**: **P3 (SQL Hygiene)**
- **Root Cause**:
  In `queries.rs:1241` and `1401`, paths and file stems are interpolated into SQL LIKE clauses (`path LIKE '%' || ? || '%'`) without calling `escape_like` or appending `ESCAPE '\\'`. Filenames containing underscores (e.g. `cli_test.rs`) treat `_` as a single-character wildcard.
- **Actionable Remediation Specification**:
  Apply `escape_like` and add `ESCAPE '\\'` to all LIKE pattern searches.

---

## 5. Cross-Cutting Architectural Analysis

### 5.1 SQLite Query Optimization & Indexing Architecture
- **Optimizer Boundary**: SQLite's query planner operates under cost heuristics. Virtual tables (such as FTS5) and recursive Common Table Expressions (such as `impact_walk`) do not provide distribution statistics to the planner.
- **The Join Reordering Hazard**: When a virtual table or recursive CTE is joined with a regular table that has indexed columns (`s.test_container`, `s.is_test`), SQLite assumes the B-tree index is cheaper and inverts the join order.
- **The Architectural Solution**: `CROSS JOIN` is SQLite's built-in syntax to force left-to-right join execution. Applying `CROSS JOIN` in `fts_search_symbols_scoped`, `find_related_tests`, and `compute_blast_radius` completely eliminates full-table scans, delivering **623x** and **48x** speedups with zero schema migrations.

### 5.2 Filesystem Watcher Concurrency & Debouncing
- **Architecture**: `code-kb` employs Tier 3 real-time synchronization using `notify-debouncer-full` with a 150ms window.
- **Hazard Analysis**:
  1. The debouncer closure executes synchronously on the event notification thread. Any long-running task (e.g. `scan_workspace` on > 50 files) blocks event delivery.
  2. Sequential process spawning (`julie-extract update`) creates significant OS process fork overhead during burst edits. Furthermore, `julie-extract update` strictly accepts a single `--file <FILE>` argument, preventing multi-file CLI batching.
  3. Recursive directory watching on Linux attaches inotify watches to `target/` and `.git/`.
- **Remediation Strategy**: Keep the callback thread lightweight, filter out ignored directories early, and handle multi-file batches via background worker queues or unified `scan_workspace` invocations.

### 5.3 Memory Bounds (< 15 MB) & Heap Retention
- **Invariant Verification**: Memory retention was verified against SQLite pragma configurations:
  - `PRAGMA query_only = ON;`
  - `PRAGMA cache_size = -4000;` (~4 MB RAM per connection).
  - `PRAGMA mmap_size = 268435456;` (256 MB zero-copy virtual memory mapping).
- **Transient Memory Guards**: `load_scoped_outline_symbols` pushes slash depth limits down into SQLite for heavy `Symbol` records (P1-8), keeping transient memory well below 15 MB (< 4 MB for 50,000 file path strings), while streaming callee signatures directly (P1-5) avoids N+1 query accumulation.

### 5.4 Documentation & Schema Parity Governance
- **The Parity Triad**: Every feature in `code-kb` spans three representations:
  1. MCP Tool Schema (`crates/code-kb-cli/src/mcp/server.rs`)
  2. CLI Command (`crates/code-kb-cli/src/main.rs`)
  3. Documentation (`README.md`, `skills/code-kb/SKILL.md`)
- **Parity Deficiencies Identified**: Small naming asymmetries (`is_test` vs `--include-tests`, `file` vs `path`) and documentation drifts (`depth: 3` vs `2`, `v0.5.0` vs `v0.5.1`) must be enforced through automated CI parity assertions.

---

## 6. Phased Remediation Roadmap

```
┌────────────────────────────────────────────────────────────────────────────────┐
│ PHASE 1: High-Impact P1 Performance, Core Logic & Test Harness Fixes           │
│ • P2-7: Implement safe_tempdir test helper (prerequisite for suite execution)  │
│ • P1-1: Apply CROSS JOIN in FTS5 search (623x speedup)                         │
│ • P1-2: Apply CROSS JOIN in blast_radius CTE (48x speedup)                     │
│ • P1-3: Canonicalize find_workspace_root path comparison                       │
│ • P1-4: Fix blast_radius_op & queries to retain both symbol and file seeds     │
│ • P1-5 & P3-10: Resolve ambiguous callees via join & filter enum constructors  │
│ • P1-7: Short-circuit file read in reconcile_offline_edits                     │
│ • P1-8: Preserve outline directory synthesis & verify Symbol memory bounding   │
└──────────────────────────────────────┬─────────────────────────────────────────┘
                                       │
┌──────────────────────────────────────▼─────────────────────────────────────────┐
│ PHASE 2: Watcher & Concurrency Resiliency                                      │
│ • P1-6: Replace watcher .expect() with safe fallback and async/scan batching   │
│ • P2-6: Add .agents and .razorback to is_hard_excluded and .gitignore          │
│ • P2-12: Document / mitigate inotify queue overflow during compilation         │
└──────────────────────────────────────┬─────────────────────────────────────────┘
                                       │
┌──────────────────────────────────────▼─────────────────────────────────────────┐
│ PHASE 3: Dead Code, Error Taxonomy & Dependency Cleanup                        │
│ • P2-1: Fix malformed SymbolNotFound suggestion error string                   │
│ • P2-2: Add Workspace error variant to EditError                               │
│ • P2-3: Enforce strict SHA-256 optimistic lock & delete dead fallback check    │
│ • P2-8: Remove redundant file_stem trimming & hoist query prepare              │
│ • P2-9: Delete dead get_log_file function                                      │
│ • P2-10: Remove unused fts_search_symbols public export                        │
│ • P2-11: Clean up Cargo.toml dependency declarations                           │
│ • P3-11: Escape LIKE wildcards in blast radius queries                         │
└──────────────────────────────────────┬─────────────────────────────────────────┘
                                       │
┌──────────────────────────────────────▼─────────────────────────────────────────┐
│ PHASE 4: Documentation, Schema & Contract Sync                                 │
│ • P2-4: Bridge is_test (MCP) and --include-tests (CLI) aliases                 │
│ • P2-5: Add CI sync check for duplicate SKILL.md files                         │
│ • P3-1: Correct blast_radius depth (2) and property name in README.md          │
│ • P3-2: Support kind alias in find_structural_facts                            │
│ • P3-3: Update docs/site/index.html to v0.5.1                                  │
│ • P3-4: Update TODO.md to v0.5.1                                               │
│ • P3-5: Annotate Plan 011 regarding code-kb prune deprecation                  │
│ • P3-6: Add local cargo test assertion for AGENTS.md == CLAUDE.md              │
│ • P3-7: Add --json support to code-kb edit CLI command                         │
│ • P3-8: Standardize blast_radius schema parameter aliases                      │
│ • P3-9: Add named options (--path, --category) to outline and facts CLI        │
└────────────────────────────────────────────────────────────────────────────────┘
```

---

## 7. Verification & Testing Protocol

### 7.1 Baseline Quality Commands
```bash
# 1. Sync contract verification
cmp AGENTS.md CLAUDE.md
diff -u AGENTS.md CLAUDE.md

# 2. Duplicate skill sync verification
cmp skills/code-kb/SKILL.md .claude-plugin/skills/code-kb/SKILL.md

# 3. Formatting and Linting
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# 4. Invariant 1 (Zero Workspace Parameters in Tool Schemas)
TMPDIR=$PWD/target/tmp cargo test --test mcp_test

# 5. Full workspace test suite
TMPDIR=$PWD/target/tmp cargo test --all-targets
```

### 7.2 Independent Benchmark Reproduction Script
To verify the SQLite join order optimizer fixes (P1-1 and P1-2), run:

```bash
python3 -c "
import sqlite3, time

conn = sqlite3.connect('.code-kb/artifact.db')

# 1. Benchmark P1-1 (FTS5 Join Inversion)
t0 = time.perf_counter()
for _ in range(100):
    conn.execute('''SELECT s.symbol_id FROM symbols_fts JOIN symbols s ON s.rowid = symbols_fts.rowid 
                    WHERE symbols_fts MATCH 'test*' AND s.is_test = 0 AND s.test_container = 0 
                    ORDER BY bm25(symbols_fts, 10.0, 5.0, 1.0) ASC LIMIT 10''').fetchall()
t1 = time.perf_counter()

for _ in range(100):
    conn.execute('''SELECT s.symbol_id FROM symbols_fts CROSS JOIN symbols s ON s.rowid = symbols_fts.rowid 
                    WHERE symbols_fts MATCH 'test*' AND s.is_test = 0 AND s.test_container = 0 
                    ORDER BY bm25(symbols_fts, 10.0, 5.0, 1.0) ASC LIMIT 10''').fetchall()
t2 = time.perf_counter()

print(f'FTS Regular JOIN: {t1 - t0:.4f}s')
print(f'FTS CROSS JOIN:   {t2 - t1:.4f}s (Speedup: {(t1 - t0)/(t2 - t1):.2f}x)')

# 2. Benchmark P1-2 (Blast Radius Recursive CTE)
sql_plain = '''WITH RECURSIVE impact_walk(symbol_id, depth) AS (
    SELECT symbol_id, 0 FROM symbols WHERE name = 'Workspace' AND kind NOT IN ('import','variable','parameter','field','property','module','namespace')
    UNION
    SELECT r.from_symbol_id, iw.depth + 1 FROM relationships r JOIN impact_walk iw ON r.to_symbol_id = iw.symbol_id WHERE iw.depth < 3
    UNION
    SELECT p.from_symbol_id, iw.depth + 1 FROM pending_relationships p JOIN symbols s_target ON p.target_terminal_name = s_target.name JOIN impact_walk iw ON s_target.symbol_id = iw.symbol_id WHERE iw.depth < 3 AND s_target.kind NOT IN ('import','variable','parameter','field','property','module','namespace')
)
SELECT s.symbol_id, s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container, MIN(iw.depth) as min_depth
FROM impact_walk iw JOIN symbols s ON iw.symbol_id = s.symbol_id
WHERE iw.depth > 0 AND s.kind NOT IN ('import','variable','parameter','field','property','module','namespace')
GROUP BY s.symbol_id, s.name, s.kind, s.path, s.start_line, s.is_test, s.test_container
ORDER BY min_depth ASC, s.path ASC, s.name ASC LIMIT 200'''

sql_cross = sql_plain.replace('FROM impact_walk iw JOIN symbols s', 'FROM impact_walk iw CROSS JOIN symbols s')

t3 = time.perf_counter()
for _ in range(50):
    conn.execute(sql_plain).fetchall()
t4 = time.perf_counter()
for _ in range(50):
    conn.execute(sql_cross).fetchall()
t5 = time.perf_counter()

print(f'Blast Radius Regular JOIN: {t4 - t3:.4f}s')
print(f'Blast Radius CROSS JOIN:   {t5 - t4:.4f}s (Speedup: {(t4 - t3)/(t5 - t4):.2f}x)')
"
```
Expected output: `CROSS JOIN` achieves > 100x speedup on FTS search and > 30x speedup on blast radius.
