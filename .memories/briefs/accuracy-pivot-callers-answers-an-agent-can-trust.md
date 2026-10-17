---
id: accuracy-pivot-callers-answers-an-agent-can-trust
title: "Accuracy pivot: callers answers an agent can trust"
status: active
created: 2026-10-17T17:10:13.294Z
updated: 2026-10-17T17:10:13.294Z
tags:
  - project-direction
  - accuracy
  - find-references
  - ground-truth-gate
---

## Direction (owner approved 2026-10-17: "Ok let's do it")

Pivot code-kb in place from token savings to accuracy and completeness. An agent must be able to trust a callers answer enough to skip the grep.

1. **Phase 1, accuracy base** (in progress): labeled callers (proven, likely, unresolved, excluded) that account for every indexed row with the name, a whole-word text count that explains every line, the `full` index level, the Rust `crate::`/crate-name path fix, and an offline SCIP-based ground-truth gate in 7 languages.
2. **Phase 2**: the reach answer (overrides, constructions, registrations, direct tests), call-site argument counts in julie, a post-edit check pushed by a hook, and tool removals.
3. **Phase 3**: a missed-site replay over session logs.
4. **Phase 4**: an agent A/B test (needs a budget approval).

## Evidence

- code-kb found 135 of 593 true call sites on 12 symbols (91% of its rows were true). Express: 1 of 313.
- `refs safe_tempdir` says 108; rg finds 209. The index holds the rest as namespaced pending rows that the resolver drops.
- 21 of 49 sampled "(0 found)" answers had real call sites.
- Raw research: `~/.code-kb/reports/2026-10-17-direction-research/`.

## Constraints

Local commits authorized in code-kb and julie-extractors; push, PR, release, and paid model runs need approval. Workflows use sonnet agents, and the lead validates each result. Plan: `docs/plans/2026-10-17-accuracy-base.md` on branch `feat/accuracy-base` (worktree `.worktrees/accuracy-base`; julie worktree `.worktrees/accuracy-base` on the same branch name).
