---
id: code-kb-lightweight-code-intelligence-engine-mcp-s
title: "code-kb: Lightweight Code-Intelligence Engine & MCP Server"
status: active
created: 2026-09-12T22:44:18.259Z
updated: 2026-09-12T23:35:47.077Z
tags:
  - code-kb
  - mcp
  - code-intelligence
  - ast
---

# code-kb: Lightweight Code-Intelligence Engine & MCP Server

## Goal
Build `code-kb`, a lightweight (<15MB RAM, <5ms query latency) Rust code-intelligence engine and MCP server that queries `julie-extractors` AST artifacts (SQLite schema v7) to provide progressive disclosure, semantic symbol navigation, and surgical context slicing to AI coding agents.

## Why Now
AI coding agents waste 80–90% of their token budget and execution turns on brute-force grep and raw file reading. `code-kb` delivers token-dense skeletons and surgical context bundles, eliminating context thrashing.

## Constraints & Guardrails
- Pure Consumer Boundary: Do not own parsers directly; query `julie-extractors` SQLite schema v7.
- Zero In-Memory Heap Graphs: Direct SQLite WAL queries; retain memory under 15MB.
- 1:1 Session-to-Workspace Binding: No `workspace_id` parameters exposed to LLMs.
- CLI-First Architecture: 1:1 CLI command for every MCP tool.
- Single-turn atomic edits (`replace_symbol_body`) with optimistic locking and immediate re-indexing.

## Active Roadmap
1. [x] Tier 3 Background File Watcher (`notify` with 150ms debounce and git-storm circuit breaker).
2. [x] Tier 2 In-Database FTS5 Conceptual Search (BM25 over docstrings, signatures, and symbol names).
3. [x] Harness integration via `.mcp.json`.
4. [ ] Dogfooding and extraction validation on `julie-extractors` codebase (>30,000 LOC).

## References
- docs/plans/001-architecture-and-service-model.md
- docs/plans/002-workspace-scoping-and-mcp.md
- docs/plans/003-tool-catalog-and-schema.md
- docs/plans/004-file-synchronization-and-watchers.md
- docs/plans/005-startup-reconciliation.md
- docs/plans/006-lessons-from-miller.md
- docs/plans/010-master-implementation-plan.md

