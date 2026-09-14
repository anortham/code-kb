---
id: code-kb-lightweight-code-intelligence-engine-mcp-s
title: "code-kb: Answer Accuracy, Bounded Output & Benchmark Rigor"
status: active
created: 2026-09-12T22:44:18.259Z
updated: 2026-09-14T21:47:16.977Z
tags:
  - code-kb
  - mcp
  - accuracy
  - bounded-output
  - resolution
  - benchmarking
---

## Goal
Strengthen the accuracy of existing answers, implement bounded token-dense output, tighten tool contracts, ensure cross-tool target identity consistency, and make result limits transparent without taking on architectural complexity (in-memory graphs, daemons, vector embeddings, multi-tier resolvers).

## Why Now
A toolkit review (documented in `docs/toolkit-assessment.md`) validated the 11 MCP tools and identified key gaps in target identity consistency (e.g. `find_references` lacking a file selector, `blast_radius` treating `file` only additively rather than as a symbol path filter), invisible result caps (callee/test/CTE ceilings), and structural-fact ergonomics.

## Constraints
- Pure SQLite queries: keep resolution and traversal logic in SQLite queries and CTEs; do not import in-memory symbol graphs or runtime daemons.
- Zero workspace parameters: maintain process-to-workspace 1:1 binding; never expose `workspace` or `repo_path` in MCP schemas.
- Retained memory target < 15MB: ensure SQLite and Rust heap remain lightweight.
- Exact 1:1 MCP to CLI parity: every MCP capability must be verifiable from the terminal.
- Single-turn atomic edits: pre-flight validation and atomic updates remain single-turn without two-step handshakes.

## Completed Work
1. **Accuracy & Bounded Output (docs/plans/015-accuracy-and-bounded-output-plan.md)**
   - Conservative reference resolution avoiding phantom joins.
   - Bounded impact formatting grouped by file.
   - Contract integrity and safety boundaries.
   - Benchmark rigor with child process RSS measurement.
2. **Toolkit Review Defect Fixes (docs/toolkit-assessment.md)**
   - Prioritized primary definitions over fields for qualified symbol lookups in `queries.rs` (`get_symbol_by_name_internal`).
   - Resolved seed symbols to exact `symbol_id`s in `compute_blast_radius`, enabling qualified seeds and rejecting unknown/ambiguous bare seeds.
   - Propagated file freshness errors in `code-kb --json skeleton` instead of returning stale data.
   - Aligned tool documentation and MCP schemas for `replace_symbol_body` and `blast_radius`.

## Active Roadmap & Follow-ups
1. **Phase 5: Target Identity & Disambiguation Consistency**
   - Disambiguate `blast_radius(symbol, file)`: when both `symbol` and `file` are provided, use `file` as the path filter for resolving `symbol`, enabling targeted blast radius on symbols that share names across files.
   - Add optional `file_path` to `find_references`: allow callers to isolate callers/callees for a symbol defined in a specific file.
2. **Phase 6: Result Completeness & Truncation Transparency**
   - Disclose caps and truncation in compact and structured responses:
     - Context slice: indicate when callee count exceeds 10 or test count exceeds 5.
     - Blast radius: indicate when impacted symbols hit CTE limits (200 rows) or display caps.
     - Search & lookup: include total match count or continuation indicator when truncating.
3. **Phase 7: Structural-Fact Ergonomics**
   - Add path filtering to `find_structural_facts`.
   - Improve category discovery and matching (e.g. normalize framework tags or provide alias mapping for common terms like `config`, `routes`).
4. **Phase 8: Workspace Hygiene & Commits**
   - Commit toolkit review documentation (`docs/toolkit-assessment.md`) and verified regression fixes.

## References
- docs/toolkit-assessment.md
- docs/plans/015-accuracy-and-bounded-output-plan.md
- docs/plans/013-post-v0.5.1-review-findings.md
- AGENTS.md
