---
id: code-kb-lightweight-code-intelligence-engine-mcp-s
title: "code-kb: Lightweight Code-Intelligence Engine & MCP Server"
status: active
created: 2026-09-12T22:44:18.259Z
updated: 2026-09-13T21:53:01.091Z
tags:
  - code-kb
  - mcp
  - code-intelligence
  - ast
  - stabilization
  - v0.5.1
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
Following `docs/plans/012-review-findings-and-fix-plan.md` [ALL PHASES COMPLETED]:
1. [x] **Phase 1: Critical Server Stability**
   - Fixed Watcher storm loop on file reads (`notify-debouncer-full`, filter `IN_OPEN`/`IN_ACCESS`).
   - Fixed dynamic rebind poison on invalid paths & added worktree-aware rebind + auto-copy fast path.
   - Fixed hidden files reconciliation wiping dotfiles.
   - Fixed formatting & CI workflow.
2. [x] **Phase 2: Correctness & Data Integrity**
   - Preserved file mode permissions (0755) and symlinks on edit.
   - Fixed `get_symbol_by_name` `LIMIT 10` import crowd-out & `related_tests` filtering.
   - Packaging / pins fallback and CRLF line ending normalization.
3. [x] **Phase 3: Agent Output Quality**
   - Added signatures and `body_hash` to bodies/slices.
   - Cleaned up `blast_radius` seeds (remove imports/variables/non-code).
   - Fixed qualified `find_references` (e.g. `McpServer::new`).
   - Cleaned up `codebase_outline` tags and `file_skeleton` doc dumping.
   - Kind normalization & labeled FTS fallback; persistent telemetry connection.
4. [x] **Phase 4: Cleanup & Ponytail**
   - Removed unused dependencies (`syn`, `walkdir`, `directories`, `anyhow`, `serde_json`, `thiserror`, `url`).
   - Deprecated centralized cache & `prune` in favor of self-cleaning in-tree databases.
   - Consolidate duplicated formatters; added release profile; aligned `AGENTS.md` and `CLAUDE.md`.
5. [x] **Phase 5: Test Hardening**
   - Required `julie-extract` in tests; strengthened assertions; added chained edit optimistic lock tests.
6. [x] **Phase 6: Release Preparation**
   - Bumped version to 0.5.1 across all manifests, verified all 7 pre-flight checks, verified Windows NTFS guest on Prax Windows 11 VM.

## References
- docs/plans/001-architecture-and-service-model.md
- docs/plans/006-lessons-from-miller.md
- docs/plans/010-master-implementation-plan.md
- docs/plans/012-review-findings-and-fix-plan.md
