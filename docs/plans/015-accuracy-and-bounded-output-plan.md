# Plan 015: Answer Accuracy, Bounded Output, and Benchmark Rigor

## Context & Motivation
A comparative architectural audit between `code-kb`, `Julie`, and `Miller` confirmed that `code-kb`'s SQLite-first, zero-in-memory-graph architecture provides excellent performance (<5ms queries, small binary footprint, isolated per-worktree storage). However, several queries promise more certainty than the current implementation provides:
1. Pending-call queries match terminal names (`p.target_terminal_name = s.name`) across the repository, causing calls like `Vec::new()` to return `McpServer::new` and `Workspace::new` as dependencies.
2. Compact output formats (`blast_radius`) dump unbounded flat lists repeating full file paths.
3. The context slice and syntax validation boundaries contain mismatches and silent fallbacks.
4. The benchmark script uses character-count token heuristics, marks substring matches as "Top Hit", and hardcodes comparative tables without measuring RSS.

The goal is to adopt: *"Correctly completed coding tasks with minimal total agent effort"*, strengthening answer accuracy without taking on the runtime daemons, vector embeddings, or complex resolver layers of Julie and Miller.

---

## 4-Phase Roadmap

### Phase 1: Conservative Reference Resolution (Highest Priority)
- **Goal:** Eliminate phantom caller and callee dependencies caused by unqualified terminal name matching in `pending_relationships`.
- **Changes in `crates/code-kb-core/src/queries.rs`:**
  - `find_callee_signatures`: Inspect `p.target_namespace_json`. If a namespace is recorded (e.g. `["Vec"]`, `["Command"]`), do not join on `s_to.name = p.target_terminal_name` unless the namespace matches a known symbol (struct, enum, module, trait) in the workspace.
  - `find_callee_references`: Do not report terminal matches when the pending relationship specifies an unresolvable external namespace.
  - `find_caller_references`: When querying callers of top-level functions or methods, match `p.target_namespace_json` or caller scope to avoid attributing calls to methods of unrelated types (e.g. arbitrary `new()` or `run()` calls) to the target symbol.
  - `impact_walk` (Blast Radius CTE): Constrain branch 2 (`pending_relationships`) so it does not join generic identifier names across unrelated namespaces.
- **Verification:**
  - `code-kb slice find_callee_signatures` must no longer list `McpServer::new` or `Workspace::new` as dependencies.
  - Add unit and integration tests covering calls with standard library and external namespaces.

### Phase 2: Bounded Output & Formatting
- **Goal:** Prevent token bloat in compact agent responses while preserving full counts and machine-readable data.
- **Changes in `crates/code-kb-core/src/formatters.rs`:**
  - `format_blast_radius`:
    - Group impacted symbols by file instead of repeating file paths per row.
    - Group likely tests by file and cap compact display at 15–20 tests.
    - Preserve full count in section headers (e.g., `Likely Tests to Run (45 found - showing top 15)`).
    - Note omitted items and reference `--json` for complete lists.
    - Hide low-signal rows (`import`, `module`) in compact downstream lists.
- **Verification:**
  - Validate output byte reduction on broad impact targets without loss of signal.

### Phase 3: Contract Integrity & Safety Boundaries
- **Goal:** Ensure tool contracts match actual guarantees and remove silent failure modes.
- **Changes:**
  - `get_context_slice_op` in `crates/code-kb-core/src/ops.rs`:
    - Align with `AGENTS.md` Invariant 4: Decide on caller inclusion and clarify type representations.
    - Replace `.unwrap_or_default()` on SQLite queries with error propagation or explicit status in `ContextSlice`.
  - `validate_syntax` in `crates/code-kb-core/src/syntax.rs`:
    - Make the boundary explicit: when editing files in languages without registered Tree-sitter grammars, emit an explicit note/flag indicating syntax checking was bypassed rather than implying full validation.
  - `codebase_outline_op` in `crates/code-kb-core/src/ops.rs`:
    - Bound memory consumption when building hierarchical trees for large repositories.

### Phase 4: Benchmark Rigor & Measurement
- **Goal:** Turn `scripts/benchmark_quality.py` into a rigorous, reproducible evaluation harness.
- **Changes in `scripts/benchmark_quality.py`:**
  - Replace `len(text) // 4` with proper token estimation or tokenizers where available.
  - Replace substring existence (`match in output`) with true rank-1 / top-k search precision checks.
  - Measure actual peak RSS via platform facilities (`/usr/bin/time -v` / `getrusage`).
  - Remove hardcoded Julie/Miller tables in favor of reproducible automated measurements.
