---
id: improve-retrieval-first-measure-task-savings-after
title: Improve retrieval first, measure task savings afterward
status: active
created: 2026-09-26T23:16:22.894Z
updated: 2026-09-27T13:54:32.454Z
tags:
  - project-direction
  - retrieval-quality
  - priorities
  - token-savings
---

## Direction

Complete the four retrieval and quality improvements before the complete-task token/context savings study, following the owner's reordered September 26 evaluation.

1. Improve reference and impact correctness, explain matching reasons, and address difficult-query latency.
2. Bound skeleton and context output with explicit continuation.
3. Strengthen held-out retrieval correctness checks using the existing test and evaluation assets.
4. Trial separate file/content search, informed by Phoebe and Miller.
5. Measure complete-task token/context savings and correct unsupported claims.

## Constraints

Keep the AST/SQLite foundation, bounded retained memory, and CLI/MCP parity. Focused correctness and performance checks belong with each improvement. Paid model replays require an explicit budget. Local commits are authorized; push, PR, and release authority remain absent.

## Status and evidence

The receiver-reference slice and its missing-index performance follow-up are implemented locally on fix/receiver-reference-reliability in .worktrees/receiver-reference-reliability. Source f804a790 fixes proven receiver mismatches and labels unresolved member references as candidates. Its labeled fixtures retain all 15 required rows, remove all 14 formerly returned forbidden rows, and label all 14 unresolved rows.

Verified source 0551d318 adds two SQLite indexes in ensure_fts_index without changing reference SQL. Five frozen-corpus pairs reduce scoped Hermes references from 20.08 s to 2.04 s median and impact on run_agent.py from 8.17 s to 6.15 s; all five workloads retain identical returned results. Reference CPU time falls 20.04 to 2.03 s, cached-read traffic 40.07 GiB to 315.10 MiB, and physical storage reads remain zero. Hermes database growth is 35.125 MiB, and a separate fresh-copy initializer takes 0.780 s. Retained RSS after all workloads is 26.69 versus 26.91 MiB. Linux 546 Rust tests plus lint/build/format and Windows 153 affected tests pass. The new performance regression test counts SQLite work rather than wall time.

Priority 1 remains open for remaining recursive inference cost and additional inference explanations; 2-second references and 6-second impact are still slow. Aggregation, UNION changes, and extra symbols indexes were measured and rejected for this fix. No complete-task token/context savings or compaction benefit has been established.

See docs/reviews/2026-09-27-reference-query-latency.md and its data JSON for current measurements, docs/reviews/2026-09-26-receiver-reference-reliability.md for the correctness slice, and docs/reviews/2026-09-26-project-evaluation.md for the broader evaluation.
