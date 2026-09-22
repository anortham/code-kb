---
id: improve-precision-in-the-ten-read-tools
title: Improve precision in the ten read tools
status: active
created: 2026-09-22T19:48:51.046Z
updated: 2026-09-22T19:48:51.046Z
tags:
  - read-tools
  - symbol-identity
  - impact
  - test-discovery
---

## Goal
Improve the existing read tools: truthful blast-radius truncation, consistent path-based context test discovery, and exact symbol selection from lookup/search into body/context/references/impact.

## Why now
The owner approved these three improvements after the telemetry and toolkit review. Both editing tools were removed in e761398; native agent tools own source writes.

## Constraints
Keep ten MCP tools, zero workspace parameters, 1:1 CLI parity, SQLite-backed bounded queries, native-edit freshness, Windows path behavior, and the pinned extractor. Reuse current symbol IDs and test-path rules. No new editing, search index, registry, dependency, or release scope.

## Success criteria
Agents can tell when impact output is incomplete, context recognizes the same test paths as impact, and same-file overloads can be selected explicitly without name fallback. Exact target selection must not overstate the precision of unresolved call edges.

## Status
Direction approved; owner requested a plan only. Plan details are for review and implementation has not begun. Continue in .worktrees/read-only-tools on refactor/read-only-tools; main does not yet contain removal. Push/release authority is absent.

## References
- docs/plans/2026-09-22-read-tool-precision.md
- docs/toolkit-assessment.md
