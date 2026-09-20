---
id: code-kb-lightweight-code-intelligence-engine-mcp-s
title: "code-kb: Answer Accuracy, Bounded Output & Benchmark Rigor"
status: active
created: 2026-09-12T22:44:18.259Z
updated: 2026-09-20T12:06:34.869Z
tags:
  - code-kb
  - mcp
  - accuracy
  - bounded-output
  - resolution
  - benchmarking
---

## Goal

Keep code-kb's existing tools accurate, compact, and inexpensive. Preserve SQLite query-time resolution, the extractor boundary, and direct CLI verification.

## Constraints

- No repository graphs, embeddings, extra daemons, or workspace parameters in MCP schemas.
- Keep single-turn validated edits and Windows support.
- The published memory figure is the measured value, about 25 MB RSS on a live server (decided 2026-09-20). There is no hard budget; measure before and after changes that could raise it.

## Status as of 2026-09-20

Target identity/path filters, structural-fact ergonomics, and prior extractor-seam repairs are present. The 2026-09-20 health review additionally fixes bounded result inputs (0–200), qualified lookup filtering, combined fact/literal limits, compact cap notices, logical empty-result telemetry, unobserved savings estimates, and release CI enforcement. Final checks pass: 247 Rust tests, 16 plugin tests, Clippy, formatting, and sync checks. Julie 3.1.1 / schema v7 / facts-level bridge checks passed; its checkout is unchanged.

## Observed follow-up candidates

These are findings, not approved implementation plans:

- Isolated server memory is about 17.7 MiB PSS; the Node launcher raises the process tree to about 67.5 MiB. A fresh-inode control found no v1.1.1-to-v1.1.3 memory regression. Preserve Windows/Node support when evaluating a smaller launcher.
- Add version-aware telemetry, percentiles, and separate startup/reconciliation/query timings. Historical result counts are unknown and old savings include synthetic baselines.
- Add persistent-MCP/large-corpus performance measurements and cross-language answer-quality cases.
- Structured responses could distinguish confirmed truncation from a conservatively reported reached limit.

## References

- docs/2026-09-20-health-review.md
- docs/plans/015-accuracy-and-bounded-output-plan.md
- docs/plans/018-julie-data-usage-review.md

The installed symlink still uses v1.1.1; current-source v1.1.3 measurements used an isolated target/health-review build. No push or release is part of this review.
