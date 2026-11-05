# Toolkit Efficiency Implementation Plan

> **For agentic workers:** Use `razorback:subagent-driven-development` for independent tasks, or tasks whose separate context has a clear benefit, when delegation is available and permitted. Otherwise use `razorback:executing-plans`: the current agent runs the plan.

**Goal:** Reduce code-kb context and instruction overhead only where fresh traces show a repeatable cost, while preserving accepted-edit quality and the code-kb contract.

**Architecture:** Keep the existing seven MCP tools, SQLite index, and one-call context flow. Use the current campaign runner for frozen comparisons among native tools, current code-kb, and an evidence-selected candidate; change only the selected context or code-kb instruction surface.

**Tech Stack:** Rust, MCP, Python evaluation harness, Codex CLI.

**Spec:** [Design contract](#design-contract)

**Architecture Quality:** Keep context behavior inside existing query and formatter boundaries. Main risk: reducing answer size or calls can hide callers, tests, conventions, uncertainty, or indexing cost that an agent needs.

**Status:** Completed and verified on `toolkit-efficiency` in `/home/murphy/.codex/worktrees/dd7b/code-kb`, retained locally without publication. The candidate failed both frozen efficiency gates and was removed. All 36 patches were independently accepted. K4 retains two source-backed skill clarifications. The full branch gate passes; see `evals/edit-2026-11-03-toolkit-efficiency/results/report.md` and `evals/edit-2026-11-03-toolkit-efficiency/verification.md`. Reviewer choice: none.

**Authority:** The implementation request authorizes this plan, necessary model trials, and local commits. Repository `AGENTS.md` authorizes necessary model-evaluation spend; no separate budget gate applies. The user explicitly prohibits pushing or releasing. Publication and global installation changes remain ungranted.

**Reviewed baseline:** `/home/murphy/source/code-kb`, `main`, `8640bcd8cf952c842ab9a7352a568987f4caab5b`, version 2.12.0. The previous review passed 20 launcher tests but interrupted the Rust workspace suite; it is not full-suite verification. Optional coordinator: `/home/murphy/source/goldfish/docs/plans/2026-11-03-ai-toolkit-coordination-plan.md`.

## Global Constraints

- The plan author and implementation executor preserve the seven MCP tools, required `project_root`, automatic index refresh, explicit coverage and uncertainty warnings, and the 42-language contract.
- Preserve retrieval completeness: callers, related tests, conventions, dependencies, open ends, budget notices, and drill-down paths remain available when the requested answer needs them.
- Preserve the Stop hook's intentional fail-open, one-shot behavior for an unchanged diff. It is not a defect to make blocking.
- Write a failing targeted test before production changes. Do not weaken existing tests or acceptance checks.
- Do not add tools, a retrieval stack, telemetry, dynamic/compiler analysis, new dependencies, or language-scope expansion.
- Keep `evals/edit-2026-10-27/` and `evals/edit-2026-10-31-one-call/` immutable. Their six cases are development/regression evidence only, not confirmation data.
- Do not claim savings from response-size comparisons alone. Preserve unavailable measurements as unavailable; never infer missing token or timing fields.
- Keep code-kb ownership local: Razorback guidance changes belong to Razorback, and cross-tool smoke testing belongs to the coordinator.

## Design Contract

This section is the binding spec for the plan.

- Optimize an observed repeated context or instruction cost, not a hypothesized bottleneck. K1 freezes the source-backed trace finding and the exact code-kb surface eligible for K2 before any implementation edit.
- Confirmation uses six previously untouched tasks spanning configuration, framework behavior, and language families, twice per arm. With a candidate, run native tools, frozen current code-kb, and candidate code-kb: 12 matched sets, 36 trials. With no justified candidate, compare native tools and current code-kb only: 12 pairs, 24 trials. Fix model, runtime, effort, inputs, seeds where supported, sandbox, grading, and counterbalanced run order before exposure. Record unsupported controls instead of pretending they exist.
- Acceptance is primary. A candidate must not regress independently accepted patch counts versus current code-kb, overall or within any task, and must introduce no protected-input violation. Retain every source-review rejection in the results. A claim of equivalent-or-better quality than native tools additionally needs acceptance at least as high as the native arm; do not infer universal quality from this small sample.
- An optimization is eligible to retain when it lowers both total and uncached input tokens by at least 5% versus frozen current code-kb, with index-inclusive agent time no more than 5% worse and the quality gate satisfied. A separate claim of whole-task savings versus native tools requires those same efficiency thresholds against the native arm. A useful code-kb improvement can pass the former without passing the latter. Report cached input, output tokens, shell/MCP calls, and missing measurements separately. Five percent is a proposed practical materiality bar, not statistical significance.
- Missing acceptance, token, or index-inclusive timing data makes the efficiency result inconclusive. A lower output size, fewer tool calls, or a favorable subset cannot substitute for the full gate.
- Freeze the sample size, comparisons, metrics, thresholds, and stop rule in K1. Do not rerun completed trials to improve scores. Record setup failures before model exposure; repair only the setup and run only unexposed slots. Do not repair a completed model attempt.
- The confirmation sample is small and cannot establish performance across all 42 languages. A negative result or no implementation change is valid; do not ship an ineffective performance diff or make a savings claim below its corresponding frozen threshold. One candidate and one confirmation campaign are in scope; do not tune on confirmation results or launch another campaign without revising the design first.
- Keep the current tool and index contracts stable. If evidence points to changing a schema, reference semantics, index freshness, language scope, or Stop behavior, stop and revise this design with the owner before proceeding.

## Verification Strategy

**Project source of truth:** `AGENTS.md`, `.github/workflows/ci.yml`, and the existing completed-edit protocol in `evals/edit-2026-10-27/` and `evals/edit-2026-10-31-one-call/`.

**Worker red/green scope:** Run the exact affected Rust test target(s) selected through `code-kb` `get_symbol_context`. If an approved evaluation-harness change becomes necessary, use its exact test file first, such as `python3 -m unittest discover -s scripts -p 'test_comparison_metrics.py'` for metric parsing; the lead owns the full script suite.

**Worker ceiling:** The affected core/CLI test binaries, evaluation-script tests when changed, and `cargo fmt --all -- --check`. No worker runs confirmation model trials outside K3.

**Worker gate invariant:** Context completeness and warnings remain intact; metric parsing preserves missing values; frozen artifacts and historical results remain unchanged.

**Lead affected-change scope:** All test targets for touched Rust crates, all script unit tests if campaign or metric code changes, and checks that code-kb guidance mirrors stay synchronized.

**Branch gate:** `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --locked`; `python3 -m unittest discover -s scripts -p 'test_*.py'`; `node --test tests/plugin/*.test.cjs`; and `cmp AGENTS.md CLAUDE.md` when either file changes. Build with `cargo build --release` before selecting any campaign runtime; record its absolute executable path, hash, version, and source commit. Never substitute a stale PATH binary for the candidate.

**Security scope:** none declared.

**Replay/metric evidence:** K1 freezes the hard acceptance and efficiency gates. The six historical tasks are development evidence only. K3 reports every paired result and every missing measurement; no broader claim follows from this sample.

**Escalation triggers:** Any change to tool schemas, `project_root`, index refresh, reference labels, the 42-language scope, Stop hook behavior, or campaign trust/sandbox boundaries requires a design update before implementation.

**Assigned verification failure:** Diagnose and repair within the owned scope while a safe plan-consistent path remains. Stop and report only an actual blocker or a design/ownership mismatch; do not weaken tests or change the gate to make the task pass.

**Verification ledger:** Record invariant, command or campaign ID, scope, commit SHA, result, and timestamp. For K3 also record manifest hash, runtime/model identity, matched slot IDs, acceptance, input/cached/uncached/output tokens, shell/MCP counts, index-inclusive time, and missing fields.

## Parallel Execution Contract

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| K1: Freeze trace finding and protocol | None - serial | `evals/edit-2026-11-03-toolkit-efficiency/` | Yes | K2 and K3 depend on K1's frozen target, cases, metrics, thresholds, and schedule. |
| K2: Implement the measured reduction | None - serial | Evidence-selected subset of `crates/code-kb-core/src/ops.rs`, `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-cli/src/mcp/server.rs`, and `skills/code-kb/SKILL.md`, plus their affected tests | Yes | K2 starts only after K1 fixes the exact surface; K3 measures the resulting candidate. |
| K3: Run paired confirmation | None - serial | `evals/edit-2026-11-03-toolkit-efficiency/results/` and `target/toolkit-efficiency/` | Yes | It uses the K1 frozen manifest and the K2 candidate build. |
| K4: Reconcile code-kb documentation | None - serial | `README.md`, `skills/code-kb/SKILL.md`, `docs/language-support.md`, `docs/edit-check.md`, `AGENTS.md`, and `CLAUDE.md`, only when evidence shows a mismatch | Yes | Documentation must match the measured K2/K3 behavior; final combined smoke belongs to Razorback. |

Commit mode for future implementation work: `parallel-lead-commit`. Each worker hands the verified diff to the lead for review. The coordinating lead may commit planning artifacts locally before implementation.

## Tasks

### Task K1: Freeze the trace finding and confirmation protocol

**Files:**
- Read: `evals/edit-2026-10-27/results/report.md`
- Read: `evals/edit-2026-10-31-one-call/report.md`
- Read: `evals/edit-2026-10-31-one-call/results/metrics.json`
- Read: `evals/edit-2026-10-31-one-call/protocol-v3.md`
- Read: `scripts/completed_edit_campaign.py`
- Read: `scripts/comparison_metrics.py`
- Create: `evals/edit-2026-11-03-toolkit-efficiency/`

**Interfaces:**
- Consumes: immutable historical reports and traces, current source, and the existing campaign/metric contracts.
- Produces: a frozen protocol, manifest, untouched confirmation cases, and one named source/test seam for K2.

**Contract inputs:** The Design contract; the existing `run_campaign` frozen inputs and 1-based `--schedule-index` runner; `summarize_trace` keeps missing usage unavailable.

**File ownership:** Own all new artifacts below `evals/edit-2026-11-03-toolkit-efficiency/`; do not edit either historical evaluation tree or implementation files.

**Serialization required:** Yes.

**Dependency reason:** K2 and K3 require K1's frozen source finding, task set, metrics, thresholds, and schedule.

**What to build:** Inspect paired traces for repeated context, instructions, and calls, then identify the smallest source-backed reduction opportunity. Freeze the confirmation protocol and six untouched tasks covering configuration, framework behavior, and language families.

**Approach:** Reconcile the 2.11.x native comparison and the 2.12.0 one-call follow-up, including acceptance and all available token and timing fields. Treat historical cases as development data only. Use the existing campaign format and record source/test ownership selected with current `code-kb` navigation. Do not select a change from response size alone.

**Acceptance criteria:**
- [x] The protocol names the trace evidence and a concrete repeated-cost finding, or records that no code-kb optimization is justified.
- [x] The protocol freezes six untouched cases, two repeats per arm, 12 matched three-arm sets (or two-arm pairs with no candidate), controls, run order, acceptance rubric, both 5% comparisons, and the stop rule before K3 starts.
- [x] Use the existing two-arm campaign format for native-versus-current and native-versus-candidate schedules if necessary; reuse each identical native control result once through immutable slot references in a combined report rather than rerunning it. Verify protocol/runner compatibility before exposure. Do not invent a new runner arm field or change the runner's trust boundaries to accommodate this plan.
- [x] Every case has independently gradable acceptance and protected-input checks; historical case files remain byte-identical.
- [x] The task identifies exact K2 source and regression-test paths from current code-kb evidence; if no seam is justified, K2 is skipped and baseline behavior is retained.
- [x] K1 artifacts pass their narrow validation and are handed to the lead per `parallel-lead-commit`.

### Task K2: Implement only the measured context reduction

**Files:**
- Modify only K1-selected files from: `crates/code-kb-core/src/ops.rs`, `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `skills/code-kb/SKILL.md`
- Test: K1-selected existing regression-test file(s) under `crates/code-kb-core/tests/` or `crates/code-kb-cli/tests/`

**Interfaces:**
- Consumes: K1's frozen finding and exact source/test ownership.
- Produces: one minimal code-kb candidate build with a failing-before/fixed-after regression test.

**Contract inputs:** The Design contract and K1's source-backed finding; current seven-tool schemas and the `get_context_slices_selected_op` contract.

**File ownership:** K2 owns only the exact source/test paths frozen by K1; no other task edits those paths.

**Serialization required:** Yes.

**Dependency reason:** K1 must identify a measured repeated cost before production code changes; K3 needs the candidate build.

**What to build:** Make the smallest change that reduces the observed code-kb context or repeated-instruction cost. Keep tool behavior and the quality evidence needed for the edit intact.

**Approach:** Use `get_symbol_context` and `find_references` on the selected seam before editing. Add the targeted failing test first. Do not assume a batching, truncation, caching, summary, or prompt rewrite is the right algorithm; implement only what the frozen traces support.

**Acceptance criteria:**
- [x] The regression test fails before the implementation and passes after it.
- [x] All seven MCP tools, explicit root, automatic refresh, caller/test/convention information, uncertainty and coverage notices, and budget/drill-down warnings remain available as required.
- [x] No unrelated public behavior, language contract, or Stop hook behavior changes.
- [x] `cargo build --release` produces the candidate and its path/hash/version/commit are frozen alongside a separately preserved current-code-kb runtime before K3.
- [x] Worker-scope verification passes; hand the verified diff to the lead per `parallel-lead-commit`.

### Task K3: Run the fresh paired confirmation campaign

**Files:**
- Read: `evals/edit-2026-11-03-toolkit-efficiency/protocol.md`
- Read: `evals/edit-2026-11-03-toolkit-efficiency/manifest.json`
- Create: `evals/edit-2026-11-03-toolkit-efficiency/results/`
- Create: `target/toolkit-efficiency/`

**Interfaces:**
- Consumes: the frozen K1 manifest and K2 candidate, or the current baseline when K1 found no justified change.
- Produces: immutable attempt records, paired metrics, and a report with a pass, fail, or inconclusive result.

**Contract inputs:** Existing `scripts/completed_edit_campaign.py` options: `--manifest`, `--artifact-paths`, `--runtime-bindings`, `--output`, and repeated 1-based `--schedule-index`; K1's fixed protocol.

**File ownership:** Own only the new results directory and the ignored `target/toolkit-efficiency/` output; never rewrite frozen inputs or completed attempt records.

**Serialization required:** Yes.

**Dependency reason:** The source candidate and frozen confirmation inputs must be complete before the campaign runs.

**What to build:** Run the 12 frozen matched sets once: native/current/candidate when a candidate exists, native/current otherwise. Score accepted edits first, then report total, uncached, and cached input tokens, output tokens, shell/MCP calls, index-inclusive agent time, and missing data. Compare candidate versus current and candidate versus native separately.

**Approach:** Use the existing runner, frozen artifacts, and output-isolated campaign path. Keep completed attempts unchanged. Repair only setup failures that occurred before model exposure and run only the affected unexposed slots. Do not tune against the confirmation cases or extend the sample after seeing results.

**Acceptance criteria:**
- [x] The report contains all 12 matched sets or marks the affected comparison inconclusive; it does not silently discard missing measurements or failed slots. It identifies every runtime by executable hash/version/source commit.
- [x] Acceptance and protected-input/source-review results are primary; all token and timing measures are reported separately.
- [x] The candidate-improvement and native-savings gates are reported separately at the Design contract thresholds; a fail/inconclusive outcome makes no corresponding savings claim. A candidate failing the retention gate is not shipped as an optimization.
- [x] No completed historical or confirmation attempt is rerun or rewritten; hand the evidence to the lead per `parallel-lead-commit`.

### Task K4: Align code-kb guidance with verified behavior

**Files:**
- Read: `evals/edit-2026-11-03-toolkit-efficiency/results/`
- Read: `skills/code-kb/SKILL.md`
- Read: `README.md`
- Read: `AGENTS.md`
- Read: `CLAUDE.md`
- Read: `docs/language-support.md`
- Read: `docs/edit-check.md`
- Modify only if a mismatch is found: the code-kb-owned documentation above
- Test only if a documentation/runtime mismatch requires a regression: its owning existing test file

**Interfaces:**
- Consumes: K2 implementation and K3's final paired report, including negative or inconclusive findings.
- Produces: source-aligned code-kb guidance or an audit record that no guidance change is warranted.

**Contract inputs:** The Design contract; `AGENTS.md` and `CLAUDE.md` byte-sync requirement; K3's exact measured result.

**File ownership:** K4 owns only the listed code-kb documentation and tests justified by a source/docs mismatch. Razorback owns Razorback guidance.

**Serialization required:** Yes.

**Dependency reason:** Guidance must describe verified behavior, not a pre-evaluation hypothesis.

**What to build:** Reconcile code-kb guidance with the verified tool output and measured limits. Preserve useful quality guidance and avoid promising savings unless K3 passes.

**Approach:** Compare every changed instruction with current source and K3 evidence, including configuration/framework reference gaps, broader-test guidance, and the advisory fail-open one-shot edit check. If no mismatch exists, leave guidance unchanged. Do not edit Razorback materials or broaden into compiler/dynamic analysis.

**Acceptance criteria:**
- [x] Any changed claim is source-backed and agrees with the paired results; a negative result contains no efficiency claim.
- [x] `AGENTS.md` and `CLAUDE.md` remain byte-identical if either changes.
- [x] Add a regression test only when an actual documentation/runtime mismatch was found; otherwise record the no-change audit.
- [x] The owning tests pass and the result is handed to the lead for the coordinator's combined smoke per `parallel-lead-commit`.

## Starting a separate implementation session

Read this plan and current `AGENTS.md`, then verify root, branch, commit, dirty state, and existing worktrees. Select an intentional implementation worktree and carry the plan into it. A fresh checkout restores its pinned extractor using `bash scripts/restore-julie-extract.sh` (Windows: `pwsh scripts/restore-julie-extract.ps1`) before building. Run K1, then the justified K2 candidate, K3, and K4 in order. Record exact selected source/runtime artifacts for Razorback's combined gate. Do not leave discarded candidate changes or required memory artifacts stranded; reconcile them locally without destructive git operations. No push or release is authorized by this plan request.
