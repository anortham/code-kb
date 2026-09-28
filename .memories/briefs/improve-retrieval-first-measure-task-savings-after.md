---
id: improve-retrieval-first-measure-task-savings-after
title: Improve retrieval first, measure task savings afterward
status: active
created: 2026-09-26T23:16:22.894Z
updated: 2026-09-27T14:42:48.280Z
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

The receiver-reference slice and two performance follow-ups are implemented locally on fix/receiver-reference-reliability in .worktrees/receiver-reference-reliability. Source f804a790 fixes proven receiver mismatches and labels unresolved member references as candidates. Its labeled fixtures retain all 15 required rows, remove all 14 formerly returned forbidden rows, and label all 14 unresolved rows.

Source 0551d318 adds receiver-type and duplicate-site indexes without changing reference SQL. Five frozen-corpus pairs reduce scoped Hermes references from 20.08 s to 2.04 s median and impact from 8.17 s to 6.15 s. Reference CPU falls 20.04 to 2.03 s; cached-read traffic falls 40.07 GiB to 315.10 MiB. This change adds 35.125 MiB to Hermes and takes 0.780 s for one separate fresh-copy initializer run.

Source fe057e683 adds one partial symbols(path, name) index for import rows. The latest five-pair run reduces Hermes impact from 6.50 s to 3.80 s median (41.6%), with observed p95 7.27 to 4.21 s. All five workloads retain identical results, including first responses and warmups. Impact CPU falls 6.39 to 3.76 s and read-call bytes 955.58 to 387.71 MiB. Physical reads vary and total 111.03 versus 131.58 MiB; do not claim a storage-I/O improvement or fully cached timings. Retained Hermes RSS is 26.80 versus 27.06 MiB. The partial index adds 7.8125 MiB; its incremental initializer takes 411.81 ms in one separate probe. Linux 547 tests plus lint/build/format and Windows 154 affected tests pass. Regression tests count SQLite work rather than wall time. No reference SQL rewrite, cache, or repository graph was added.

## Remaining scope

Priority 1 remains open for inference explanations and further difficult-query latency work; the approved import-index performance slice is complete. Impact remains several seconds and reaches its existing discovery ceiling. A builder-aggregation prototype was not adopted. No complete-task token/context savings or compaction benefit has been established.

See docs/reviews/2026-09-27-impact-query-latency.md and its data JSON for current results, docs/reviews/2026-09-27-reference-query-latency.md for the preceding index fix, docs/reviews/2026-09-26-receiver-reference-reliability.md for correctness, and docs/reviews/2026-09-26-project-evaluation.md for the broader evaluation.
