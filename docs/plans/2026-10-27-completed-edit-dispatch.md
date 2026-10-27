# Execute the completed-edit comparison

**Goal:** Execute and source-review the frozen 24-cell native/code-kb study, retaining every attempt and independent patch grades.
**Spec:** [Completed-edit protocol](../../evals/edit-2026-10-27/protocol.md).
**Authority:** The owner approved continued work and explicitly removed separate budget/spending gates. Do not request spending approval or add financial caps/receipts. Local commits are authorized; push/release/global configuration changes are not.
**Architecture:** Thin orchestration over the reviewed snapshot, sandbox, grading and trace-metrics APIs. Preserve existing modules and the 42-language engine contract.
**Architecture quality:** Existing reviewed isolation boundary is authoritative. The launcher must not expose host/control data or mistake passing tests for source acceptance.
**Commit mode:** parallel-lead-commit; lead reviews and executes provider calls. No external reviewer chosen.

## Constraints

- Stay in `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, starting at `007e2b3`.
- Keep six cases, source pins, order, repeats, model/effort, profiles, canonical inputs and 900-second trial deadline fixed. Bind the new driver hash before scored dispatch.
- No model calls from implementation tests or workers. Root may run non-task SDK calibration through the unchanged, already tested sandbox API in parallel with launcher implementation. Scored trials wait for launcher verification and calibration.
- Source/setup/agent/grader state stays separate; no hidden tests, references, control manifests, host repos or credentials in agent commands.
- Retain partial patches and every error. Completed failures are outcomes; infrastructure retries remain separate attempts, never best-of replacements.
- Preserve all historical scored evidence. Do not change global routing from this sample.

## Verification strategy

**Worker:** Test-first local dispatch-boundary tests, then its module once. Fake external Codex only; exercise real snapshot, sandbox/grade contracts where practical.
**Lead affected-change:** Review mounts/argv/state, run one isolated non-task real calibration, validate actual frozen inputs and outgoing payloads, reconstruct every scored patch through trusted graders.
**Branch:** Full Python scripts suite after launcher integration, JSON/digest/whitespace/guidance checks, source-review receipts for every completed trial, clean source-control reconciliation. Unchanged Rust/Windows/plugin scopes are reused.
**Security scope:** Reuse existing provider-policy/outbound redactor and execution isolation. No dependency additions/audit. Credentials are read-only to trusted Codex and denied to commands; never print/copy credential content.

## Parallel execution contract

| Task | Batch | Ownership | Serialization | Dependency |
| --- | --- | --- | --- | --- |
| Launcher | A | scripts/completed_edit_campaign.py, scripts/test_completed_edit_campaign.py | No | Uses existing APIs |
| Controls and execution | A then B | Lead: docs, manifest/bindings, calibration, study outputs and source reviews | Scored calls wait for launcher | Calibration uses existing verified API |

## Task 1: Real-agent launcher

**Files / ownership:** `scripts/completed_edit_campaign.py`, `scripts/test_completed_edit_campaign.py`.
**Interfaces:** Existing `edit_sandbox.run`, `edit_snapshot` and `edit_evaluation` APIs; retain full JSONL and grading metadata.
**Contract inputs:** Frozen manifest, explicit artifact paths/runtime bindings, trusted seeded snapshots, arm profiles and saved native Codex authentication.
**Serialization:** No for implementation. **Dependency:** None.
**What to build:** Run one selected schedule cell or resume the fixed campaign without duplicate dispatch; capture/replay/grade after completion, timeout or failure.
**Approach:** Keep a host-owned state record before dispatch, clone verified durable seeds, prepare dependencies without credentials, preindex only code-kb, run pinned Codex inside agent mode, capture changes independently, grade fresh public/hidden trees, leave source review pending.

- [x] Agent argv/profile/mounts match frozen controls; native has no engine/index; no spending approval/cap branch exists.
- [x] Preserve raw traces, cleanup/timeout/partial patches, setup/index timing, metrics and pending source acceptance.
- [x] Existing/interrupted records never silently redispatch; every attempt is retained and manifest-bound.
- [x] Focused red/green and final module pass; hand source/evidence to lead.

## Task 2: Calibration, study and report

**Files / ownership:** Lead owns controls, docs, results and review receipts under `evals/edit-2026-10-27/` plus durable memory.
**Interfaces:** Launcher CLI/API and existing grader/summary APIs.
**Contract inputs:** Reviewed launcher and frozen study controls.
**Serialization:** Scored provider calls wait for Task 1 acceptance; non-task SDK calibration uses the existing tested sandbox API. **Dependency:** Actual correctness/isolation preflight.
**What to build:** The actual comparison report, not a readiness-only claim.

- [x] Real non-task calibration proves private command denial, project/state writes, expected trace and cleanup without task leakage. Three calls are retained in `results/controls/calibration/`: one missing-companion startup failure, then a passing native and code-kb call. There were no scored task exposures.
- [ ] All 24 scheduled trials complete or retain classified actual blockers/outcomes; no result replacement.
- [ ] Every patch is independently graded and source-reviewed before acceptance; compare paired acceptance/time/token outcomes honestly.
- [ ] Report, raw evidence, brief and local commits are complete and reconciled; no push/release/global routing change.

## Source-redactor correction before calibration

The real preflight scanned every frozen seed plus task/public-command/profile payloads. All six source bundles trigger the generic redactor, which corrupts non-secret annotations such as `def lookup(key: str)` into invalid source. The unchanged-input requirement applies to benchmark seeds; source redactor deltas are audit findings, not automatic evidence of leaked credentials. All six task/profile payloads are unchanged by redaction and have zero host-secret environment matches. Preserve public source provenance and hash identity, reject actual secret exposure, and retain redacted output artifacts. No task, model, profile or source is tuned in response to scored outcomes; none have run yet.

## Verified dispatch freeze

The reviewed launcher and trace parser are bound in the 32-control manifest. All 281 Python tests and 15 focused launcher tests pass. A real offline preindex completed with unchanged source, and a sandbox probe confirms the native arm cannot find an engine or index. Both arm calibrations passed after the retained missing-companion failure. Evidence is in `evals/edit-2026-10-27/results/controls/dispatch/identity.json` and `calibration/identity.json`. No scored task exposure preceded this freeze.

## Task 3: Correct evaluation defects without replacing outcomes

Original cases, manifest and model outputs remain immutable. The daemon restart stopped the controller after slot015 had a completed model answer and captured bundle. Recovery v3 grades that same bundle; agent/index times are unavailable. The resumed controller uses a byte-identical redactor in the task cache because plugin cache 0.48.4 was removed during restart. No model rerun occurred.

Compiler/test work below waits until the original timed schedule finishes. Finish extra verification and fresh readiness before the four supplemental TypeScript calls; no concurrent test/build jobs during their timed execution.

| Owner | Files and responsibility |
| --- | --- |
| Grader worker | `scripts/edit_evaluation.py`, `scripts/test_edit_evaluation.py`: summary count floors with strict coverage/status guards |
| Recovery worker | `scripts/completed_edit_campaign.py`, its tests, `scripts/comparison_campaign.py` and its tests: bind the pinned redactor, retain logs on sanitization failure, persist setup/execution metadata before grading |
| Watcher worker | `crates/code-kb-core/src/watcher.rs` and its registration tests: recover once when a discovered non-root directory disappears before registration; preserve real failures and indexing policy |
| Case worker | `corrected-cases/{hcl,date-fns,minijinja}`, `results/case-corrections/`: exact generated roots, help matcher and all-six fresh readiness after core grader is stable |
| Curators | Existing per-case source reviews and independent added-test execution in protected public replays |
| Lead | Protocol-v2, frozen-inputs/manifest-v2, corrected grades, supplemental calls, aggregate report, archives, final review/commits |

- [x] Test-first count fix accepts additional passing tests but keeps arbitrary numeric literals and named markers exact. Only full framework-scoped summaries get count floors; ambiguous bare Vitest fragments remain literal. Distinct Cargo summaries cannot satisfy two expectations with one line. NUnit discovery must be complete, passed count at least baseline, failures/skips absent. Preserve exit/timeout/empty-run rejection. Astra reviewed this plan read-only.
- [x] Versioned Mini matcher accepts accurate ASCII-letter-case wording, including both `without regard to ASCII letter case` and `ignores ASCII letter case`; all three suffixes have distinct boundaries. Missing suffix/case explanation and text outside the autoescape section still fail.
- [x] Versioned generated-root lists cover the exact canonical pnpm directory and Go suite binaries, without hiding source changes.
- [x] Recovery metadata survives a grader interruption; failed sanitization does not delete the only retained logs. Redactor selection uses the validated artifact rather than a private versioned plugin path.
- [ ] A deterministic watcher regression covers disappearance between discovery and registration. Confirm `NotFound`, retry the walk once, and preserve root/permission/repeated-churn failures. Verify the affected Rust scope and final Linux/Windows checks; keep the frozen trial engine unchanged.
- [ ] All-six fresh readiness, affected tests and full Python suite pass before a v2 freeze. Regrade all four MiniJinja and four C# patches uniformly, with no model calls.
- [ ] Execute only four previously unexposed TypeScript cells with v2 inputs; retain original setup failures and disclose amended order. Complete all source/new-test reviews, original/corrected reporting and local commits.
