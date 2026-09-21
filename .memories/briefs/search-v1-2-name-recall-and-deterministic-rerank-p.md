---
id: search-v1-2-name-recall-and-deterministic-rerank-p
title: "Search v1.2: name recall and deterministic rerank (plan 020)"
status: completed
created: 2026-09-20T18:42:01.489Z
updated: 2026-09-21T00:50:44.980Z
tags:
  - search
  - v1.2
  - plan-020
---

## Direction
Implement docs/plans/020-search-v1.2-recall-and-rerank.md: trigram FTS5 table on symbol names, transactional FTS migration that surfaces failures, candidate admission that protects each recall source, a Rust rerank with a const weight table, `--explain` output, an in-repo synthetic corpus gate, then release v1.2.0.

## Constraints
- SQLite + FTS5 only. No embeddings, no Tantivy, no in-memory index, no daemon.
- Warm search p50 under 25 ms; rerank under 1 ms at 200 candidates; retained RSS stays about 25 MB.
- Evaluation sets live outside the checkout at ~/.code-kb/search-eval/ (development, sealed acceptance, regression). Tune on development only; run acceptance once.
- Release (push, tag, crates publish) is an approval boundary.

## Work state
- Worktree: /home/murphy/source/code-kb/.worktrees/feat-search-v1.2, branch feat/search-v1.2-recall-rerank, BASE a42bd4d.
- SDD ledger: .razorback/sdd/020-search-v1.2-recall-and-rerank-204433d7fab2/progress.md (git-ignored).
- Task order: 1 (eval sets) ∥ 2 (corpus gate); then 3 migration (creates both FTS tables), 4 trigram admission, 5 rerank + explain, 6 eval run + plan 019 tables, 7 release.
