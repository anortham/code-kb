# code-kb

`code-kb` is a fast, lightweight code-intelligence engine and Model Context Protocol (MCP) server designed specifically for AI coding agents. 

Backed by the rich AST fact tables produced by [`julie-extractors`](https://github.com/anortham/julie-extractors), `code-kb` provides progressive disclosure, semantic symbol navigation, and surgical context slicing—enabling agents to navigate and understand codebases with **80–90% fewer tokens** without burning context on raw file reads or text grep.

## Documentation & Architecture Plans

The design of `code-kb` incorporates critical lessons from real-world agent benchmarking and avoids the pitfalls of previous code-intelligence tools:

- [**001: Architecture & Service Model**](docs/plans/001-architecture-and-service-model.md) — Multi-project workstation scope, Git worktree deduplication, and RAG vs. AST evaluation.
- [**002: Workspace Scoping & MCP**](docs/plans/002-workspace-scoping-and-mcp.md) — 1:1 session binding, eliminating workspace registries and `workspace_id` friction.
- [**003: Tool Catalog & Schema**](docs/plans/003-tool-catalog-and-schema.md) — The 7 core read tools, token-minimized output formats, and the `replace_symbol_body` AST edit tool.
- [**004: File Synchronization & Watchers**](docs/plans/004-file-synchronization-and-watchers.md) — 3-tier sync: tool-driven updates, JIT staleness guards, and debounced background watching.
- [**005: Cold-Start Reconciliation**](docs/plans/005-startup-reconciliation.md) — Detecting and reconciling offline edits in under 50ms on startup.
- [**006: Retrospective Lessons from Miller**](docs/plans/006-lessons-from-miller.md) — Analysis of calibration data, performance ledgers, and traps to avoid.
- [**010: Master Implementation Plan**](docs/plans/010-master-implementation-plan.md) — The phased engineering roadmap from workspace scaffolding to release.

## Key Principles

1. **Sub-15MB Rust Binary:** No heavy runtime dependencies, no web dashboard, no GPU embedding models.
2. **Sub-5ms Query Latency:** Queries SQLite directly in WAL mode with zero in-memory heap bloat.
3. **CLI-First Architecture:** Every MCP tool has an exact 1:1 CLI command for instantaneous terminal testing and dogfooding.
4. **Token-Dense Progressive Disclosure:** Skeletons strip implementation bodies; context slices deliver only the target body plus callee signatures and types.
