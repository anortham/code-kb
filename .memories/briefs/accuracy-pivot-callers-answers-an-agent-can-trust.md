---
id: accuracy-pivot-callers-answers-an-agent-can-trust
title: "Accuracy pivot: callers answers an agent can trust"
status: active
created: 2026-10-17T17:10:13.294Z
updated: 2026-10-27T19:37:22.732Z
tags:
  - project-direction
  - accuracy
  - find-references
  - ground-truth-gate
---

# Direction

Support all 42 pinned Julie languages, seven MCP tools with CLI parity, explicit project_root, bounded SQLite query memory, Windows, and byte-equivalent AGENTS.md/CLAUDE.md. Preserve original scored outcomes and raw evidence.

# Owner's spending instruction

The owner explicitly ordered: “we don't need any gates around budget/spending, if there is such a thing written in the repo or tools remove it”. Approved work includes model evaluation and necessary spending. Do not ask for separate spending approval, impose financial dispatch caps, require approval receipts, or resurrect old spending gates from checkpoints or archives. Technical input/readiness/isolation/result-integrity checks remain. Push, publication, release and unrelated global configuration are still outside this task.

# Completed cleanup

Both comparison tools have no financial approval checks. The edit validator needs no call_limit/budget fields and returns only technical validity/counts. The legacy runner has no authorized.json prerequisite, arbitrary-job spending cap or retry spending ceiling. A regression dispatches 73 fake jobs without a marker. Full Python suite passes 266 tests; affected modules pass 18 and 16. Current real 26-control manifest validates after updated runner/protocol hashes, with explicit prior-commit lineage. Case, schedule, model, runtime, profile, guidance and dependency inputs are unchanged. Paired guidance and stale saved next-step instructions are corrected. Original 72-run manifest and raw runtime archive are unchanged. Plan: docs/plans/2026-10-27-remove-spending-gates.md; proof: evals/edit-2026-10-27/results/controls/spending-removal/identity.json.

# Verified foundation

Source 70a2c9b passes Linux 1,904 / Windows 1,876 Rust tests, all 42 languages, Clippy, fmt and release. Runtime source is unchanged by cleanup. Windows VM is off; it may be started whenever needed and shut down after checks. Preserve unrelated guest test.log. Watch probe: 1,016 to three registrations, retained RSS 15,760 to 14,760 KiB. Six larger pinned cases have passing baselines, intended seed failures, reviewed references, fresh replay and isolated final-binary MCP controls.

# Continue

Run the already approved completed-edit evaluation after the cleanup commit, without another spending question. No new scored trials have run yet. Retain every actual attempt, grade fresh reconstructions and review source before acceptance. Use durable seeds/tools/dependencies under /home/murphy/.cache/code-kb-completed-edit/2026-10-27-fe7ede0. Worktree /home/murphy/.codex/worktrees/1e6f/code-kb; branch fix/cross-language-review.
