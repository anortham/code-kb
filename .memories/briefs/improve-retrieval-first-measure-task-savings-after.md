---
id: improve-retrieval-first-measure-task-savings-after
title: Improve retrieval first, measure task savings afterward
status: active
created: 2026-09-26T23:16:22.894Z
updated: 2026-09-26T23:40:46.301Z
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

Keep the AST/SQLite foundation and CLI/MCP parity. Focused correctness and performance verification still belongs with each improvement. Moving the broader savings study last does not postpone those checks. Paid model replays still require an explicit budget.

## Status and reference

Priority order approved. The first implementation plan is ready for approval at docs/plans/2026-09-26-receiver-reference-reliability.md in worktree .worktrees/receiver-reference-reliability, branch fix/receiver-reference-reliability. It covers receiver disambiguation, uncertainty labels and affected consumers, then performance diagnosis. Implementation has not started, and this slice does not complete priority 1. See docs/reviews/2026-09-26-project-evaluation.md for the full evidence and acceptance criteria. The earlier recommendation to run the savings study first is superseded.
