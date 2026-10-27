---
id: accuracy-pivot-callers-answers-an-agent-can-trust
title: "Accuracy pivot: callers answers an agent can trust"
status: active
created: 2026-10-17T17:10:13.294Z
updated: 2026-10-27T00:40:40.637Z
tags:
  - project-direction
  - accuracy
  - find-references
  - ground-truth-gate
---

## Direction

Code-kb gives coding agents source-backed answers with explicit uncertainty and missing coverage. Progressive disclosure supports useful work; compact output alone does not prove lower task cost or higher success. Keep the seven-tool surface, exact CLI counterparts, per-call absolute project_root, SQLite queries, bounded retained memory, and Windows support.

Every language supported by the pinned Julie extractor is in scope. Build common behavior on extracted facts, add language-specific semantics where needed, and test accepted valid code alongside stale uses. Missing facts or consumer behavior are gaps, never language N/A. AGENTS.md and CLAUDE.md must remain identical and state this requirement.

## Review implementation

The owner approved docs/reviews/2026-10-26-project-direction.md fixes. Initial repairs are committed on main at aeebe72: stale-index failures are surfaced/retried, unknown tools rejected before preparation, lookup suggestions filtered correctly, and benchmarks/graders/CI repaired.

The broader slice is implemented in /home/murphy/.codex/worktrees/1e6f/code-kb on fix/cross-language-review, starting at aeebe72; runtime commit2104bad and test-portability commitde67a39 are local and verified on Linux. The plan is docs/plans/2026-10-26-cross-language-review-fixes.md. Real-extractor fixtures cover all42 pinned Julie3.9.1 IDs and detect language-set drift. Advanced argument, construction, removed-member, callable-value, CSS/HTML navigation, bounded paging, broader test-command fallback, and release-evidence binding have real regressions. Callback values remain advisory without an invocation contract. Separate dataclass constructor deltas avoid confusing ClassVar/unannotated class members with constructor fields.

Linux verification passes:1818 Rust tests,172 Python tests,20 plugin tests, workspace Clippy, formatting, release build, whitespace, and guidance synchronization. Final lint-equivalent private helper refactors passed391 focused checks; two final portability fixture checks also passed. Live same-root20-call MCP comparison retained23.64MiB before/24.46MiB after. Windows NTFS full gate passes1794 Rust tests across34 suites on de67a398db6b1e92d3bb244d6f758fce789abd17, including all42-language conformance. Both real npm fixtures and59 repaired argument cases passed first. Plugin tests passed12 with8 declared platform skips. Windows Clippy, formatting, and the locked release build also pass; the native executable started successfully. Initial Windows runtime/conformance suites passed, with only fixture setup/launcher failures corrected. The VM is shut off. All planned local implementation and verification are complete; final evidence is recorded in the review, plan, and this brief. No push/release/paid model session is authorized.

## Remaining evidence and limits

Docs/language-support.md records observed42-language fixture coverage separately from Julie capability flags and advanced grammar coverage. Julie still omits a CSS call identifier in a concrete Unicode-suffix case, loses U+2000 declaration spelling, and lacks CSS formal contracts and dialect-safe SQL argument facts. F# currying and dynamic/framework invocation remain conservative. These are documented gaps, not empty-result safety proofs.

Eight fresh retrieval/test-reach tasks and separate hidden oracles are prepared under evals/review-2026-10-26 with an updated controlled comparison procedure. No paid comparison ran. Historical edit/planted-bug corpora are regression/tuning evidence. Next strategic evidence is an approved controlled agent-task comparison and reusable upstream facts; do not claim task-success, time, or token-cost improvement from local checks alone.

## Boundaries

No dashboard, registry, embedding runtime, compiler rewrite, new general MCP tools, external messaging, push, publishing, or release without demonstrated need and applicable authorization. Keep task work in its active worktree; commit .memories artifacts. No paid runs without budget approval.
