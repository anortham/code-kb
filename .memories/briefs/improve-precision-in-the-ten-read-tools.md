---
id: improve-precision-in-the-ten-read-tools
title: Improve precision in the ten read tools
status: active
created: 2026-09-22T19:48:51.046Z
updated: 2026-09-22T21:50:12.750Z
tags:
  - read-tools
  - symbol-identity
  - impact
  - test-discovery
---

## Goal
Improve the ten read tools: truthful blast-radius truncation, consistent path-based context test discovery, and exact symbol selection from lookup/search into body/context/references/impact.

## Why now
The owner approved these three improvements after the toolkit review. Native agent tools own source writes.

## Constraints
Keep ten MCP tools, zero workspace parameters, 1:1 CLI parity, SQLite-backed bounded queries, native-edit freshness, Windows path behavior, and the pinned extractor. No new editing, search index, registry, dependency, or release scope.

## Success criteria
Impact reports its output and discovery caps; context and impact share test-path recognition; same-file overloads can be selected by current symbol ID without name fallback. Unresolved-call matching remains heuristic.

## Status
Implemented and locally verified through source commit 74baf19 on refactor/read-only-tools in .worktrees/read-only-tools. Linux passed 394 Rust tests; the Windows NTFS guest passed 388 Rust tests after restoring checksum-verified julie-extract 3.3.1. Main still lacks the read-only removal and precision work. No push, merge, release, or publication was authorized.

## References
- docs/plans/2026-09-22-read-tool-precision.md
- docs/decisions/002-exact-symbol-selection.md
- docs/toolkit-assessment.md
