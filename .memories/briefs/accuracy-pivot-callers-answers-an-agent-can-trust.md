---
id: accuracy-pivot-callers-answers-an-agent-can-trust
title: "Accuracy pivot: callers answers an agent can trust"
status: active
created: 2026-10-17T17:10:13.294Z
updated: 2026-10-26T20:38:02.733Z
tags:
  - project-direction
  - accuracy
  - find-references
  - ground-truth-gate
---

## Direction

Keep the owner-approved accuracy and completeness pivot: coding agents need source-backed answers with explicit uncertainty and missing coverage. Progressive disclosure and small responses support that goal; compression is not evidence of cheaper or more successful completed work.

## Current status, 2026-10-26

Main has shipped v2.9.1 with julie-extract 3.9.1. The phase 1–3 features are implemented: labeled callers and text accounting, seven MCP tools, reach sections, argument evidence, check_edit and Stop hooks, and offline edit/session replays. The index uses the references level, not full. Test-reach warnings and the level-by-level walk shipped in v2.9.1.

Accuracy and usefulness still have measured limits. Existing edit and planted-bug holdout sets were used for fixes and are now regression/tuning evidence. Runtime/framework test entry points, removed Python attributes, and functions passed as values remain coverage gaps. A controlled agent-task comparison on the current release remains outstanding; the two-worker live check was diagnostic, not a controlled outcome baseline.

## Completed project review

The full assessment is in docs/reviews/2026-10-26-project-direction.md. It recommends retaining the lean tools, a fresh held-out task comparison alongside a small edit-check capability trial, and actionable incomplete-result handling. These are recommendations, not approval for new product scope.

The review fixes are verified locally: failed refreshes now return errors and retry rather than answering from stale indexes; unknown tools are rejected before preparation; lookup miss suggestions respect kind filters; the benchmark parses current output and fails on malformed results; stale graders and CI coverage are repaired. Legacy integration fixtures were made compatible with real updates. Verification passed: 1,663 Rust tests, 149 Python tests, 20 plugin tests, release build, Clippy, formatting, and guidance synchronization. All work stayed in the original main checkout; no push or release was performed.

## Constraints

Keep the per-call absolute project_root contract, per-project SQLite index, bounded retained memory, pinned extractor, Windows support, and native host editing/testing. No registry, dashboard, embedding runtime, or compiler rewrite without a demonstrated task need. Pushes, releases, and paid model comparisons require explicit approval.

## Evidence and plans

- docs/bug-replay-baseline.md
- docs/replay-baseline.md
- docs/reviews/2026-10-22-v2.9.1-dogfood.md
- docs/plans/2026-09-23-retrieval-value-and-optional-routing.md
- docs/reviews/2026-10-26-project-direction.md
