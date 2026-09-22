---
id: improve-precision-in-the-ten-read-tools
title: Improve precision in the ten read tools
status: completed
created: 2026-09-22T19:48:51.046Z
updated: 2026-09-22T22:59:02.761Z
tags:
  - read-tools
  - symbol-identity
  - impact
  - test-discovery
---

## Goal
Improve the ten read tools: truthful blast-radius truncation, consistent path-based context test discovery, and exact symbol selection from lookup/search into body/context/references/impact.

## Why
The owner approved these three improvements after the toolkit review. Native agent tools own source writes.

## Constraints
Keep ten MCP tools, zero workspace parameters, 1:1 CLI parity, SQLite-backed bounded queries, native-edit freshness, Windows path behavior, and the pinned extractor. No new editing, search index, registry, or dependency.

## Outcome
Completed and published in code-kb v2.0.0 at release commit `5403618` on `main`. The release removed the two editing tools, added exact current-index symbol IDs across read tools, disclosed blast-radius limits, and aligned context test discovery with the shared test-path policy. Exact-commit CI, release builds, archive checksums, local Linux and Windows tests, and both crates.io packages passed verification. Unresolved-call matching remains heuristic.

## References
- docs/plans/2026-09-22-read-tool-precision.md
- docs/decisions/002-exact-symbol-selection.md
- .memories/release-v2.0.0-verification.md
