---
id: improve-retrieval-first-measure-task-savings-after
title: Improve retrieval first, measure task savings afterward
status: active
created: 2026-09-26T23:16:22.894Z
updated: 2026-09-27T01:21:06.173Z
tags:
  - project-direction
  - retrieval-quality
  - priorities
  - token-savings
---

## Direction

The owner accepted the five improvement areas in the September 26 evaluation and changed their order. Complete the four retrieval and quality improvements before the complete-task token/context savings study.

1. Improve reference and impact correctness, explain matching reasons, and address difficult-query latency.
2. Bound skeleton and context output with explicit continuation.
3. Strengthen held-out retrieval correctness checks using the existing test and evaluation assets.
4. Trial separate file/content search, informed by Phoebe and Miller.
5. Measure complete-task token/context savings and correct unsupported claims.

## Constraints

Keep the AST/SQLite foundation and CLI/MCP parity. Focused correctness and performance verification belongs with each improvement. Moving the broader savings study last does not postpone those checks. Paid model replays require an explicit budget.

## Status and evidence

The approved receiver-reference slice is implemented and verified locally on fix/receiver-reference-reliability in .worktrees/receiver-reference-reliability. Verified source f804a790 fixes proven receiver mismatches and labels unresolved member references as candidates. Labeled regression cases retain all 15 required rows, remove all 14 formerly returned forbidden rows, and label all 14 unresolved rows. Linux 545 Rust + 19 plugin tests and Windows/NTFS 135 affected core + 2 protocol tests pass.

Priority 1 remains open. Paired frozen-corpus measurements show scoped Hermes references at 18.52 s baseline versus 20.50 s candidate median; blast on run_agent.py is 6.17 s versus 8.27 s. The owner explicitly flagged 20 seconds as too slow and asked for CPU/disk evidence. Three warm CPU/I/O samples per arm show about one saturated core, zero storage reads, and about 40 GiB of repeated cached-read traffic per query. Final SQL diagnosis assigns 15.81 s to pending-call checks and 4.07 s to identifier checks. The follow-up isolated two missing lookup indexes: resolved receiver type and same-name pending-call site. The implementation now creates them for new and existing databases without changing reference semantics. Initial probes reduce the 20-second query to about 2 seconds with identical rows and about 37 MiB index growth; final paired measurements and Windows verification are in progress. The Linux gate passes all 546 Rust tests, lint, formatting, and release build. Aggregation and additional symbols indexes were rejected after measurements. Complete-task token/context savings remain unmeasured.

See docs/reviews/2026-09-26-receiver-reference-reliability.md and its data JSON for this slice, and docs/reviews/2026-09-26-project-evaluation.md for the broader evaluation. Local publication authority remains absent. The earlier recommendation to run the savings study first is superseded.
