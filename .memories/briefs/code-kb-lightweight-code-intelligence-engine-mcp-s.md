---
id: code-kb-lightweight-code-intelligence-engine-mcp-s
title: "code-kb: Lightweight Code-Intelligence Engine & MCP Server"
status: active
created: 2026-09-12T22:44:18.259Z
updated: 2026-09-13T21:11:28.868Z
tags:
  - code-kb
  - mcp
  - code-intelligence
  - ast
  - stabilization
---

# code-kb: Lightweight Code-Intelligence Engine & MCP Server

## Goal
Build and stabilize `code-kb`, a lightweight (<15MB RAM, <5ms query latency) Rust code-intelligence engine and MCP server that queries `julie-extractors` AST artifacts (SQLite schema v7) to provide progressive disclosure, semantic symbol navigation, and surgical context slicing to AI coding agents.

## Why Now
Post-v0.5.0 code audit revealed critical server stability bugs (watcher inotify infinite storm loop eating CPU, MCP session bricking on invalid paths), agent output noise, and worktree isolation gaps. Resolving these stabilizes the server for unattended multi-agent workflows.

## Architectural Invariants & Decisions
- Pure Consumer Boundary: Do not own parsers directly; query `julie-extractors` SQLite schema v7.
- Zero In-Memory Heap Graphs: Direct SQLite WAL queries; retain memory under 15MB.
- Zero Workspace Parameters: 1:1 process-to-workspace binding; dynamic rebind via path inspection; never expose workspace parameters to LLMs.
- In-Tree Self-Cleaning Storage: Every repo and git worktree owns its isolated `<root>/.code-kb/artifact.db`. No centralized cache store; dead databases are automatically removed by OS when worktrees/repos are deleted.
- Worktree Fast-Path: When indexing a worktree, copy parent repo's `.code-kb/artifact.db` and run `reconcile` (<15ms) instead of full re-extraction.
- Single-turn atomic edits (`replace_symbol_body`) with syntax validation and immediate re-indexing.

## Active Plan & Roadmap
Following `docs/plans/012-review-findings-and-fix-plan.md`:
1. [ ] **Phase 1: Critical Server Stability**
   - Fix Watcher storm loop on file reads (`notify-debouncer-full`, filter `IN_OPEN`/`IN_ACCESS`).
   - Fix dynamic rebind poison on invalid paths & add worktree-aware rebind + auto-copy fast path.
   - Fix hidden files reconciliation wiping dotfiles.
   - Fix formatting & CI workflow.
2. [ ] **Phase 2: Correctness & Data Integrity**
   - Preserve file mode permissions (0755) and symlinks on edit.
   - Fix `get_symbol_by_name` `LIMIT 10` import crowd-out & `related_tests` filtering.
   - Packaging / pins fallback.
3. [ ] **Phase 3: Agent Output Quality**
   - Add signatures and `body_hash` to bodies/slices.
   - Clean up `blast_radius` seeds (remove imports/variables).
   - Fix qualified `find_references` (e.g. `McpServer::new`).
   - Clean up `codebase_outline` tags and `file_skeleton` doc dumping.
4. [ ] **Phase 4: Cleanup & Ponytail**
   - Remove unused dependencies (`syn`, `walkdir`, etc.).
   - Deprecate centralized cache & `prune` in favor of self-cleaning in-tree databases.
   - Consolidate duplicated formatters; add release profile.
5. [ ] **Phase 5: Test Hardening**
   - Require `julie-extract` in tests; strengthen assertions.
6. [ ] **Phase 6: Release Preparation**
   - Bump version to 0.5.1 and prepare release.

## References
- docs/plans/001-architecture-and-service-model.md
- docs/plans/006-lessons-from-miller.md
- docs/plans/010-master-implementation-plan.md
- docs/plans/012-review-findings-and-fix-plan.md

