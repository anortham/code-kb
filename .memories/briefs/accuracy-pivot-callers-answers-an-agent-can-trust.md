---
id: accuracy-pivot-callers-answers-an-agent-can-trust
title: "Accuracy pivot: callers answers an agent can trust"
status: active
created: 2026-10-17T17:10:13.294Z
updated: 2026-10-28T01:08:17.449Z
tags:
  - project-direction
  - accuracy
  - find-references
  - ground-truth-gate
---

## Goal

Give coding agents reliable navigation and edit evidence across all 42 pinned Julie language IDs. Preserve seven MCP tools with CLI parity, explicit project_root, bounded SQLite memory, first-class Windows and synchronized AGENTS/CLAUDE.

## Current verified state

The requested Claude review reported one medium watcher issue. Lead verification confirmed and fixed a narrower queued-error case for registered directories, preserving the single shared retry and fail-closed handling. Fix source 8b4ca9b passes all 1,910 Linux Rust tests and 103 affected Windows tests, including all 42 language fixtures. Clippy, formatting and locked release builds pass on both. VM is confirmed off. Claude's coverage was selective, not an exhaustive signoff. See docs/reviews/2026-10-27-claude-branch-review.md. Earlier full Windows verification at 89991f0 passed 1,878 tests; do not present that old full run as a fresh full run of the later fix.

## Completed-edit evidence

The unchanged study has 24 actual trials and 22 accepted patches: native 10/12 and code-kb 12/12. Both native failures are C# purity omissions. Code-kb uses more total and uncached input and takes a mean 66.67 seconds longer across eleven complete pairs. Slot 15 timing remains unknown. Preserve originals, corrected grades, reviews and separate archives. See evals/edit-2026-10-27/results/report.md.

## Recommended direction

Reduce repeated navigation and context overhead while preserving accepted edits. Use these six cases for development/regression after tuning; reserve untouched configuration/framework tasks across language families for confirmation. No new experiment is dispatched, no task-wide savings claim or routing change is established.

## Authority and operation

All financial approval gates/caps/receipts are removed from project tools and installed Codex/Claude guidance. Necessary evaluation/spending and local commits are authorized; no push, release or global routing change. Commit memories. Worktree remains /home/murphy/.codex/worktrees/1e6f/code-kb, branch fix/cross-language-review; main is untouched. Avoid rebinding the old installed MCP here: it recursively watches ignored task trees and hits limits. Use the freshly built CLI with explicit --root for navigation and check_edit.
