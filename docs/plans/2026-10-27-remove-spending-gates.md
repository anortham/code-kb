# Remove spending approval requirements

**Goal:** Evaluation tools and project guidance must not require budget/spending approval, approval receipts, or financial dispatch caps.
**Spec:** Owner's instruction on 2026-10-27: “we don't need any gates around budget/spending, if there is such a thing written in the repo or tools remove it”.
**Architecture quality:** No architecture impact; remove approval policy from validation and execution, preserving reproducibility and isolation.
**Authority:** Implementation and local commits are already authorized. No additional approval round. No push, publication or release.
**Commit mode:** parallel-lead-commit. Reviewer: none; lead reviews each diff.

## Global constraints

- Reuse `/home/murphy/.codex/worktrees/1e6f/code-kb`, `fix/cross-language-review`, starting at `ab7a73c`.
- Preserve all original scored outcomes and raw historical evidence; archived approval records are history, never current prerequisites.
- Preserve fixed experimental schedules, input hashes, readiness, protection, isolation and honest outcome accounting.
- Keep AGENTS.md and CLAUDE.md byte-equivalent and keep all 42 languages in scope.
- Workers do not stage, commit, launch models or edit sibling-owned files.

## Verification strategy

**Worker:** Test-first focused regression, then the affected Python test module once on final source.
**Affected change:** CLI manifest validation without spending metadata; runner reaches its normal preparation/dispatch path without authorized.json using a fake provider.
**Branch:** Full Python scripts suite once after both implementations; JSON/hash checks, original-pilot identity, guidance sync, diff and worktree checks. Rust/plugin scopes are unchanged and reused.
**Security scope:** No dependency additions or outbound provider dispatch in this removal slice; retain all isolation/protection/credential controls. No dependency audit declared.

## Parallel execution contract

| Task | Batch | Ownership | Serialization | Dependency |
| --- | --- | --- | --- | --- |
| Legacy runner | A | scripts/comparison_campaign.py, scripts/test_comparison_campaign.py | No | Independent tool |
| Edit validator and guidance | A | scripts/edit_evaluation.py, scripts/test_edit_evaluation.py, docs, evals/edit-2026-10-27 active metadata, paired guidance, active memory | No | Independent tool; lead integrates |

## Task 1: Legacy comparison runner

**Files / ownership:** `scripts/comparison_campaign.py`, `scripts/test_comparison_campaign.py`.
**Interfaces:** Existing CLI and trial/campaign entry points.
**Contract inputs:** Frozen 72-trial design, input identities, trial state and outcome records.
**Serialization required:** No. **Dependency reason:** None; independent files.
**What to build:** A valid run needs no spending authorization file and has no financial approval/cap branch.
**Approach:** Remove authorization metadata from execution; retain independent technical checks and retry accounting.

- [x] Valid trial execution needs no authorized.json, budget receipt or spending permission.
- [x] Input mismatch, duplicate/in-progress state and accounting remain correct; no financial cap is substituted.
- [x] Focused red/green and final module pass; hand diff/evidence to lead.

## Task 2: Edit validator, active guidance and integration

**Files / ownership:** Lead owns `scripts/edit_evaluation.py`, `scripts/test_edit_evaluation.py`, relevant docs and active metadata, paired AGENTS/CLAUDE, active memory.
**Interfaces:** Manifest validation API/CLI and current instructions.
**Contract inputs:** Existing six cases, readiness, control hashes and 24-cell schedule.
**Serialization required:** No; final integration follows Task 1. **Dependency reason:** Module tests can run independently; shared final suite waits for both.
**What to build:** Validate reproducibility without spending fields; remove stale approval instructions and record the owner's policy durably.
**Approach:** Test a valid manifest with no financial metadata, remove validation branches/authorization output, update hashes with explicit lineage for changed controls.

- [x] No budget fields, receipts, call spending caps or dispatch-authorization output are required.
- [x] Active docs/guidance/memory direct agents to continue authorized work without separate spending approval.
- [x] Current manifest and changed control hashes validate; old raw evidence remains unchanged and clearly historical.
- [x] Full affected verification passes, plans/memory are current, and intentional changes are committed with clean worktree state.
