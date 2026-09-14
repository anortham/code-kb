---
id: code-kb-lightweight-code-intelligence-engine-mcp-s
title: "code-kb: Answer Accuracy, Bounded Output & Benchmark Rigor"
status: active
created: 2026-09-12T22:44:18.259Z
updated: 2026-09-14T13:19:07.793Z
tags:
  - code-kb
  - mcp
  - accuracy
  - bounded-output
  - resolution
  - benchmarking
---

## Goal
Strengthen the accuracy of existing answers, implement bounded token-dense output, tighten tool contracts, and establish rigorous benchmarking for `code-kb` without taking on the architectural complexity (in-memory graphs, daemons, vector embeddings, multi-tier resolvers) that burdened Julie and Miller.

## Why Now
A comparative audit against Julie and Miller identified key areas where answers promise more certainty than the implementation delivers: unqualified pending-call joins return phantom dependencies (e.g. `Vec::new()` matching `McpServer::new`), unbounded impact formatting burns agent context, and context slice / syntax safety contracts contain silent fallbacks.

## Constraints
- Pure SQLite queries: keep resolution logic in SQLite queries and CTEs; do not import Miller's multi-tiered resolver architecture or Julie's in-memory graph traversals.
- Zero workspace parameters: maintain process-to-workspace 1:1 binding.
- Memory target < 15MB: ensure SQLite and Rust heap remain lightweight.
- Preserved JSON payloads: bounded output optimizations apply to compact text formats while machine-readable JSON remains complete.

## 4-Phase Roadmap (All Completed)
Following `docs/plans/015-accuracy-and-bounded-output-plan.md`:
1. **Phase 1: Conservative Reference Resolution (COMPLETE)**
   - Tightened `pending_relationships` joins in `queries.rs` using `target_namespace_json`, `target_receiver`, and local scope.
   - Eliminated phantom caller/callee dependencies (e.g. `Vec::new` -> `McpServer::new`) across `find_callee_signatures`, `find_callee_references`, `find_caller_references`, and `impact_walk`.
2. **Phase 2: Bounded Output & Formatting (COMPLETE)**
   - Grouped `blast_radius` compact output by file.
   - Capped likely-test targets (top 20) and impacted symbols (top 50) while preserving total count headers and `--json` reference.
   - Suppressed low-signal `import`/`module` rows in compact downstream lists.
3. **Phase 3: Contract Integrity & Safety Boundaries (COMPLETE)**
   - Updated `validate_syntax` to return `Result<bool, SyntaxError>` and added `syntax_checked` to `EditResult`.
   - Replaced `.unwrap_or_default()` in `get_context_slice_op` with `?` error propagation.
   - Added table existence check in `find_type_facts` to gracefully handle partial/mock databases.
   - Bounded tree generation in `codebase_outline_op` to 1,000 files with truncation notice.
   - Synced Invariant 4 byte-for-byte between `AGENTS.md` and `CLAUDE.md`.
4. **Phase 4: Benchmark Rigor & Measurement (COMPLETE)**
   - Implemented exact peak RSS measurement via `os.wait4` (child process rusage).
   - Replaced substring checks with rigorous Rank-1 and Top-5 search precision evaluations.
   - Replaced character heuristic with accurate token estimation.
   - Removed unverified competitor claims in favor of automated measurements.

## Success Criteria (All Met)
- `code-kb slice` and `code-kb refs` do not return false-positive workspace symbols for calls with external namespaces (verified live on `find_callee_signatures`).
- Compact blast radius output groups symbols by file, caps at 20 tests / 50 symbols, and reduces output tokens without dropping full JSON data.
- Benchmark script reports measured peak RSS (14.2–28.07 MB), warm query latencies (5.58 ms median), and verified search rankings (83.3% Top-1, 100% Top-5).
- All 159 tests pass across workspace; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are 100% clean.

## References
- docs/plans/015-accuracy-and-bounded-output-plan.md
- docs/plans/012-review-findings-and-fix-plan.md
- docs/plans/006-lessons-from-miller.md
