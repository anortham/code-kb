# Completed-edit evaluation implementation plan

> Use razorback:subagent-driven-development. Commit mode is parallel-lead-commit; the lead reviews, stages, and commits each completed slice.

**Goal:** Build and validate a reproducible completed-edit evaluation on six larger, previously unused repositories, with patches and tests as acceptance evidence.

**Architecture:** Separate source snapshots, isolated process execution, and trusted orchestration/grading. Reuse trace metrics while preserving the original read-only campaign. Preparation, agent trials, and graders have distinct mounts and credentials.

**Tech stack:** Python standard library, Git, bubblewrap, installed language toolchains, existing code-kb binaries and Codex CLI.

**Spec:** [Completed-edit protocol](../../evals/edit-2026-10-27/protocol.md).

**Architecture quality:** Read-only Astra advice incorporated: mutable trials need allowlisted mounts, environment sanitation, independent filesystem patch capture, protected inputs, and fresh grader reconstruction. The main risks are hidden-answer exposure and accepting an agent's claims instead of its patch.

## Global constraints

- Work in `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, starting at `edd897d`.
- The owner's approval to continue authorizes local work and commits; do not ask again for reversible implementation choices.
- Model evaluation is covered by the owner's approval. No separate spending approval, financial cap or receipt is required; manifest, readiness and isolation checks remain required.
- No push, publication, release, global configuration change, new product tool, or speculative engine feature.
- Preserve all original pilot artifacts and production routing. All 42 languages remain in scope; selected languages are a sample.
- Tests first. No comments in tests. Workers do not stage/commit, launch models, or modify files owned by another task.
- Download pinned public sources to ignored `target/edit-evaluation/`; do not change working copies under `/home/murphy/source` or existing evaluation caches.

## Verification strategy

**Source of truth:** AGENTS.md, CLAUDE.md, the protocol, and each upstream project's pinned manifests/test instructions.

**Worker red/green:** Single unittest ID for evaluator changes; a selected upstream check for each curated case. Record the failing behavior, command, output, SHA, and timestamp.

**Worker ceiling:** Each worker's new test module once on its final tree; each case's baseline/seed/reference commands. No Rust workspace suite or provider CLI calls.

**Lead affected-change:** New evaluator tests together, preserved comparison harness tests, actual sandbox/Codex permission probes, fresh reconstruction and grading for every reference patch.

**Branch gate:** Full Python scripts suite, plugin checks, JSON/hash validation, diff/format checks, source-control reconciliation, and review of all case readiness records. Reuse Rust/Linux/Windows runtime evidence at edd897d unless product Rust changes.

**Security scope:** No dependency-audit command is declared for this dependency-free evaluator. Run the existing outbound redactor on constructed payloads before any future dispatch. Isolation, no-secret environment, protected files, no-network graders, and malicious-patch negative tests are hard gates.

**Evidence:** Patch replay identity, baseline green/seed purposeful red/reference green, protection, cleanup, and isolation are hard gates. Tokens, time, and footprint are report-only. Fake-agent runs validate the evaluator, not coding-agent usefulness.

**Escalation:** A product defect requires a separate owned regression/fix slice and relevant runtime gates. Corpus/toolchain failures are retained and repaired before freezing tasks; they cannot be waived into scored trials.

**Verification ledger:** Record scope, invariant, command, source identity, result, timestamp, and artifact location. Reuse unchanged passing scopes. Fix failed IDs before repeating a wide command.

## Parallel execution contract

| Task | Batch | File ownership | Serialization required | Dependency reason |
| --- | --- | --- | --- | --- |
| 1. Patch snapshots | A | `scripts/edit_snapshot.py`, `scripts/test_edit_snapshot.py` | No | Independent filesystem contract. |
| 2. Execution isolation | A | `scripts/edit_sandbox.py`, `scripts/test_edit_sandbox.py` | No | Independent subprocess/mount contract. |
| 3. Python/Rust cases | A then B | `evals/edit-2026-10-27/cases/sqlglot/`, `cases/minijinja/` | Yes for execution | Source inspection and checks first; Task2 required before upstream execution. |
| 4. Go/TypeScript cases | A then B | `evals/edit-2026-10-27/cases/hcl/`, `cases/date-fns/` | Yes for execution | Source inspection and checks first; Task2 required before upstream execution. |
| 5. Java/C# cases | A then B | `evals/edit-2026-10-27/cases/commons-lang/`, `cases/nodatime/` | Yes for execution | Source inspection and checks first; Task2 required before upstream execution. |
| 6. Local orchestration | After A | Fresh Luna owns `scripts/edit_evaluation.py` and `scripts/test_edit_evaluation.py`; lead owns protocol/corpus/manifest/readiness/report/memory files | Yes | Integrates reviewed Tasks1/2 and completed cases. |

### Task 1: Capture and replay exact patches

**Files:** Create `scripts/edit_snapshot.py` and `scripts/test_edit_snapshot.py`.
**Interfaces:** Snapshot/capture/apply signatures and schema semantics in the protocol.
**Contract inputs:** Protected/excluded paths are trusted relative paths; source and bundle data are untrusted.
**File ownership:** `scripts/edit_snapshot.py`, `scripts/test_edit_snapshot.py`.
**Serialization required:** No.
**Dependency reason:** Independent filesystem contract.

**What to build:** Independent manifest comparison and replayable content bundles. Capture changes that Git would miss and preserve evidence for invalid attempts.
**Approach:** Stream hashes, use content-addressed blobs, validate every input before replay, and never follow escaping links.

- [x] Additions, deletions, ignored/untracked files, modes, directories and links round-trip exactly.
- [x] Tampered blobs, traversal, escaping links, special files, limits, protected changes and wrong seeds fail safely with retained evidence.
- [x] New test module passes; report API evidence and hand verified diff to lead.

### Task 2: Isolate preparation, agents and graders

**Files:** Create `scripts/edit_sandbox.py` and `scripts/test_edit_sandbox.py`.
**Interfaces:** The protocol's `run` CLI; document the matching Python API signature before case execution.
**Contract inputs:** Fixed container paths; no writable host-root or orchestration mount; trusted logs remain outside child access.
**File ownership:** `scripts/edit_sandbox.py`, `scripts/test_edit_sandbox.py`.
**Serialization required:** No.
**Dependency reason:** Independent subprocess/mount contract.

**What to build:** Real bubblewrap execution with explicit environment/mounts, protected inputs, resource deadlines, and process cleanup. Supply the preparation/grade path first so corpus workers can validate safely.
**Approach:** Start from an empty filesystem, expose system/tool/dependency inputs, and use disposable state. Verify agent-specific Codex profile denial using the installed sandbox command, without calling a model.

- [x] Real child commands can edit permitted source and use prepared dependencies while host canaries, grader inputs, logs and protected writes are inaccessible.
- [x] Graders have no provider credentials/network; preparation networking is explicit; agent shell access respects the verified writable profile and private-profile denial.
- [x] Timeout and descendant cleanup retain outputs/partial edits; malformed paths/configuration and unsupported modes fail closed.
- [x] New module and actual isolation probes pass; hand exact API/CLI and verified diff to lead.

### Task 3: Curate Python and Rust cases

**Files:** Create `case.json`, `request.md`, `seed.patch`, `reference.patch`, and checks under `evals/edit-2026-10-27/cases/sqlglot/` and `cases/minijinja/`.
**Interfaces:** Protocol case schema and Task2 execution CLI/API.
**Contract inputs:** Exact corpus.json pins, upstream instructions/licenses, full source snapshots, no prior-corpus overlap.
**File ownership:** `evals/edit-2026-10-27/cases/sqlglot/`, `cases/minijinja/`.
**Serialization required:** Yes for execution.
**Dependency reason:** Task2 sandbox must be reviewed before upstream commands run. Reading/fetching/writing checks can proceed independently.

**What to build:** One meaningful multi-file edit task per repository with real tests and a known repair. Report actual source files/lines, dependency setup, protected/generated paths and commands.
**Approach:** Inspect pinned source first, write checks, produce a purposeful seeded failure, then the minimal reference repair. Run no upstream setup/build script on the host.

- [x] Both repositories satisfy size/exposure rules and have concrete requests requiring at least two production files.
- [x] Canonical baseline passes; seeded input fails for the intended reason; reference repair passes hidden and selected public checks in the sandbox.
- [x] Case data has exact executable commands, safe paths, retained evidence and no placeholders; hand results to lead without model calls/commits.

### Task 4: Curate Go and TypeScript cases

**Files:** Same case contract under `evals/edit-2026-10-27/cases/hcl/` and `cases/date-fns/`.
**Interfaces:** Protocol case schema and Task2 execution CLI/API.
**Contract inputs:** Exact corpus pins, upstream instructions/licenses and full snapshots.
**File ownership:** `evals/edit-2026-10-27/cases/hcl/`, `cases/date-fns/`.
**Serialization required:** Yes for execution.
**Dependency reason:** Task2 reviewed sandbox before upstream execution; source/check preparation is independent.

**What to build:** Multi-file edits that exercise callers and preserve unrelated valid behavior. Freeze source-backed checks and reference patches without exposing them to trials.
**Approach:** Use actual package/lock/test commands; keep build output separate from immutable dependency inputs.

- [x] Both candidates meet size/exposure rules and have source-backed tasks requiring two production files.
- [x] Baseline green, purposeful seed red, reference green and relevant public regressions are retained from sandbox runs.
- [x] Exact case contract, dependency setup, paths and evidence pass worker scope and are handed to lead.

### Task 5: Curate Java and C# cases

**Files:** Same case contract under `evals/edit-2026-10-27/cases/commons-lang/` and `cases/nodatime/`.
**Interfaces:** Protocol case schema and Task2 execution CLI/API.
**Contract inputs:** Corpus pins, actual SDK/build-tool requirements and upstream test instructions/licenses.
**File ownership:** `evals/edit-2026-10-27/cases/commons-lang/`, `cases/nodatime/`.
**Serialization required:** Yes for execution.
**Dependency reason:** Task2 reviewed sandbox before upstream execution; reading/check preparation is independent.

**What to build:** Two runnable multi-file edit cases with explicit Java/.NET coverage. Keep task validity separate from toolchain/dependency failures.
**Approach:** Obtain tools/dependencies only in disposable preparation state, then prove offline grading. Choose supported installed SDK targets from actual project configuration.

- [x] Both source pins meet size/exposure rules and have concrete multi-file requests.
- [x] Baseline, seeded failure and repaired acceptance/public checks run with reviewed tools and retained logs.
- [x] No global installs/configuration changes; complete source-backed cases and readiness evidence go to lead.

### Task 6: Integrate local trials and freeze readiness

**Files:** Create `scripts/edit_evaluation.py`, `scripts/test_edit_evaluation.py`, campaign manifest/schema/readiness/report files under `evals/edit-2026-10-27/`; update this plan, project direction and memory.
**Interfaces:** Reviewed snapshot/sandbox APIs, case contract and existing trace metrics.
**Contract inputs:** All source/control/dependency identities, protected tests, immutable prior pilot and reproducible execution controls.
**File ownership:** Fresh Luna owns the two orchestration scripts; lead owns manifests, reports, docs and memory. Lead reviews and integrates the worker's implementation; no sibling-owned code changes without handoff.
**Serialization required:** Yes.
**Dependency reason:** Requires reviewed Tasks1/2 and verified cases from Tasks3–5.

**What to build:** End-to-end local rehearsal with actual patches, independent reconstruction and trusted grading. Produce a concrete frozen study ready for a separately gated model dispatch.
**Approach:** Use fake subprocess agents for successful edits and adversarial failures; preserve exact artifacts/outcomes. Retain failed readiness instead of silently weakening selection.

- [x] Real fake-agent runs cover success, partial timeout, protected tampering, untracked/deleted source, invalid output and failed grading; replay identity is enforced.
- [x] All six cases have reviewed source, references, protected tests and retained readiness; all manifests/hashes validate and answer leakage tests pass.
- [x] Proposed 24-run schedule, model/configuration and technical controls are frozen; no provider is called by preparation or tests.
- [x] Python/plugin/JSON/security scopes pass, docs/brief reflect evidence and remaining dispatch authority, and local commits/worktrees are reconciled.
