---
id: accuracy-pivot-callers-answers-an-agent-can-trust
title: "Accuracy pivot: callers answers an agent can trust"
status: active
created: 2026-10-17T17:10:13.294Z
updated: 2026-10-26T21:24:56.588Z
tags:
  - project-direction
  - accuracy
  - find-references
  - ground-truth-gate
---

## Direction

Keep the owner-approved accuracy and completeness pivot: coding agents need source-backed answers with explicit uncertainty and missing coverage. Progressive disclosure and small responses support that goal; compression is not evidence of cheaper or more successful completed work.

The owner explicitly requires support for every language julie-extractors supports. Shared extraction and navigation do not establish parity of advanced checks. Track applicable capabilities per language, implement common behavior over extractor facts, and test language-specific semantics. Do not narrow the roadmap to Python because recent dogfood used Flask.

## Current status, 2026-10-26

Main has shipped v2.9.1 with julie-extract 3.9.1. The phase 1–3 features are implemented: labeled callers and text accounting, seven MCP tools, reach sections, argument evidence, check_edit and Stop hooks, and offline edit/session replays. The index uses the references level, not full. Test-reach warnings and the level-by-level walk shipped in v2.9.1.

Accuracy and usefulness still have measured limits. Existing edit and planted-bug holdout sets were used for fixes and are now regression/tuning evidence. Runtime/framework test entry points, removed Python attributes, and functions passed as values remain coverage gaps. A controlled agent-task comparison on the current release remains outstanding; the two-worker live check was diagnostic, not a controlled outcome baseline.

## Completed project review

The full assessment is in docs/reviews/2026-10-26-project-direction.md. It recommends retaining the lean tools, an explicit language-by-capability inventory and conformance coverage, fresh held-out tasks across language families, and actionable incomplete-result handling. The initial Python-first edit-check recommendation was corrected after the owner's clarification: removed-member and callback handling must be evaluated across applicable languages. The owner authorized implementing the review fixes on 2026-10-26, with explicit all-language coverage and matching AGENTS.md/CLAUDE.md guidance. The active implementation plan is docs/plans/2026-10-26-cross-language-review-fixes.md.

The review fixes are verified locally: failed refreshes now return errors and retry rather than answering from stale indexes; unknown tools are rejected before preparation; lookup miss suggestions respect kind filters; the benchmark parses current output and fails on malformed results; stale graders and CI coverage are repaired. Legacy integration fixtures were made compatible with real updates. Verification passed: 1,663 Rust tests, 149 Python tests, 20 plugin tests, release build, Clippy, formatting, and guidance synchronization. All work stayed in the original main checkout; no push or release was performed.

## Active implementation

Worktree: /home/murphy/.codex/worktrees/1e6f/code-kb, branch fix/cross-language-review, based on aeebe72. All task work remains there. Batch A: real-extractor conformance for all 42 languages, argument semantics, construction semantics, bounded report recovery/test fallback, and release-evidence validation. Removed member reads and callable value dependencies follow finalized argument/construction behavior. The plan records source-backed ownership, verification, and prior authorization. Missing applicable extractor facts remain gaps; coverage labels cannot certify them away. No paid agent runs, push, PR, or release is authorized.

## Constraints

Keep the per-call absolute project_root contract, per-project SQLite index, bounded retained memory, pinned extractor, Windows support, and native host editing/testing. No registry, dashboard, embedding runtime, or compiler rewrite without a demonstrated task need. Pushes, releases, and paid model comparisons require explicit approval.

## Evidence and plans

- docs/bug-replay-baseline.md
- docs/replay-baseline.md
- docs/reviews/2026-10-22-v2.9.1-dogfood.md
- docs/plans/2026-09-23-retrieval-value-and-optional-routing.md
- docs/reviews/2026-10-26-project-direction.md
