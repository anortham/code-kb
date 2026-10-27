# Cross-language review fixes

> **For agentic workers:** Use razorback:subagent-driven-development. Independent tasks run in parallel with lead review and commits.

**Goal:** Implement the project review's actionable fixes while making support for every pinned Julie language an explicit, tested product requirement.

**Architecture:** Keep the seven tools, shared CLI/MCP operations, per-call project root, indexed SQLite queries, and bounded retained memory. Build common checks on extracted facts and preserve language semantics. Keep navigation, edit evidence, construction reasoning, output recovery, and release evidence independently verifiable.

**Tech Stack:** Rust, SQLite, pinned julie-extract 3.9.1, Python evaluation scripts, shell release scripts, Node plugin tests.

**Spec:** [Project direction review](../reviews/2026-10-26-project-direction.md), the owner's all-language requirement, and AGENTS.md.

**Architecture Quality:** Read-only architecture review approved proceeding with explicit language applicability, real extraction tests, conservative callback evidence, bounded result recovery, and evidence fingerprints. The main risk is mistaking an extractor capability or a name match for a complete semantic proof.

## Authorization

The owner said: "ok let's proceed with the fixes then docs/reviews/2026-10-26-project-direction.md and I want to make sure that we have cross language support and the CLAUDE.md and AGENTS.md file in this project should have made it clear that we try to support all languages". This authorizes local implementation and commits without another plan approval round.

- local_commit_authority: authorized by that implementation request.
- push_authority: missing; do not push.
- pr_authority: missing; do not publish a PR.
- Paid agent comparisons and extractor releases require separate approval. Prepare reproducible local evidence and runnable comparisons here; do not substitute them for measured paid agent outcomes.
- Reviewer choice: none for external model CLIs. The lead reviews each implementation inline.

## Global Constraints

- Active worktree: `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, starting at `aeebe72`.
- All code-kb calls use that worktree as `project_root`; all Goldfish calls use it as `workspace`.
- AGENTS.md and CLAUDE.md remain byte-for-byte equivalent.
- All 42 languages in the pinned extractor's machine-readable capability report are in scope. Missing facts are outstanding gaps, not semantic non-applicability.
- Tests must use real extraction at the production references level. Synthetic rows alone do not establish language coverage.
- A callback reference establishes a dependency, not a broken invocation. Weak evidence cannot become a proven Stop blocker.
- Do not weaken existing precision, uncertainty, test-path, or overload behavior to increase recall.
- No repository graph or retained file list in memory; SQL remains the repository query engine. Preserve Windows paths and handle lifetimes.
- No new general MCP tool, embedding runtime, watcher daemon, registry, or compiler-scale subsystem.
- No narration comments; public API docs remain short and factual. Tests have no comments.

## Verification Strategy

**Project source of truth:** AGENTS.md, CLAUDE.md, .github/workflows/ci.yml.

**Worker red/green scope:** Exact new test IDs using `cargo test -p code-kb-core --test <owned-test> <test-id> -- --exact`, CLI equivalent for recovery tests, or Python unittest IDs for evidence scripts. Read current helpers first. New conformance cases that already pass characterize the real integration; each behavior fix requires a demonstrated failing case first.

**Worker ceiling:** Each worker's dedicated test binary and directly changed module's existing unit tests once on the final tree. Workers do not run the workspace suite or release build.

**Worker gate invariant:** Tests prove the task's stated observable behavior, including valid unchanged uses and misleading same-name evidence. A failing gate is investigated and fixed within ownership; notify the lead of cross-task dependencies.

**Lead affected-change scope:** Existing edit-check/arity/reference/reach tests after their coherent batch; CLI/MCP recovery tests after output changes; Python harness suite after script changes. Reuse recorded worker evidence on unchanged code.

**Branch gate:** `cargo test --workspace --locked --no-fail-fast`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, `cargo build --release`, `python3 -B -m unittest discover -s scripts -p 'test_*.py'`, `node --test tests/plugin/*.test.cjs`, guidance synchronization, whitespace checks, and `check_edit`.

**Security scope:** none declared; no new dependency or external transmission is planned.

**Replay/metric evidence:** Real-extractor conformance and regression assertions are hard gates. Fresh local retrieval/replay timing, coverage, and memory measurements are report-only unless an existing gate declares otherwise. Historical corpora remain regression/tuning sets. Do not claim an agent outcome improvement without an agent comparison.

**Escalation triggers:** Changed reference classification requires related reference-report regressions; changed retained state requires live MCP RSS measurement; missing upstream facts require a concrete repro and dependency resolution rather than a false completion claim.

**Assigned verification failure:** Investigate and fix exact failed IDs; update the lead if resolution crosses ownership. Preserve the gate's intended behavior.

**Verification ledger:** `.razorback/sdd/<plan-key>/progress.md` records invariant, command, scope, SHA or worktree state, result, and time. Reuse passing output for unchanged scopes; broad failure logs are read before any rerun.

## Parallel Execution Contract

Commit mode: `parallel-lead-commit` for every task. Workers do not stage, commit, checkpoint, or spawn agents.

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| 1: Pinned-language conformance | A | `evals/languages/**`, `crates/code-kb-core/tests/language_conformance_test.rs`, `docs/language-support.md` | No | None - safe parallel batch. |
| 2: Argument semantics | A | `crates/code-kb-core/src/edit_check/signatures.rs`, `crates/code-kb-core/src/reference_report/arity.rs`, `crates/code-kb-core/tests/edit_check_arguments_test.rs` | No | None - safe parallel batch. |
| 3: Construction semantics | A | `crates/code-kb-core/src/edit_check/fields.rs`, `crates/code-kb-core/src/reference_report/sections.rs`, `crates/code-kb-core/tests/edit_check_fields_test.rs` | No | None - safe parallel batch; preserve FieldChange's existing interface. |
| 4: Removed members and callable values | A | `crates/code-kb-core/src/edit_check.rs`, `crates/code-kb-core/src/reference_report.rs`, `crates/code-kb-core/tests/edit_check_members_test.rs`, optional new `crates/code-kb-core/src/edit_check/members.rs` | No | None - safe parallel batch; this task is the single orchestration owner for explicit interface handoffs from tasks 2 and 3. |
| 5: Actionable output recovery | A | `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-core/src/test_commands.rs`, `crates/code-kb-core/src/queries.rs`, `crates/code-kb-core/src/ops.rs`, `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/src/mcp/protocol.rs`, `crates/code-kb-cli/tests/report_recovery_test.rs`, optional new `crates/code-kb-core/src/report_pages.rs` | No | None - safe parallel batch; new core exports are integrated by the lead. |
| 6: Evaluation and release evidence | A | `scripts/release-preflight.sh`, `scripts/release-dogfood.sh`, `scripts/reach_gate.py`, `scripts/test_reach_gate.py`, `scripts/test_release_evidence.py`, `scripts/release-evidence-inputs.json`, optional new `scripts/release_evidence.py`, `docs/RELEASING.md`, `docs/plans/2026-09-23-retrieval-value-and-optional-routing.md`, dedicated fresh local evaluation fixtures under `evals/review-2026-10-26/**` | No | None - safe parallel batch. |
| 8: Extractor fact compatibility | A | `crates/code-kb-core/src/db.rs`, `crates/code-kb-core/src/sync.rs`, `crates/code-kb-core/tests/extractor_compat_test.rs` | No | None - safe parallel batch; preserve existing public lifecycle APIs. |
| 7: Integration and documentation | C | `AGENTS.md`, `CLAUDE.md`, `README.md`, `docs/edit-check.md`, `docs/argument-counts.md`, `docs/reach-sections.md`, review/plan files, `.memories/**`, `crates/code-kb-core/src/lib.rs`, `crates/code-kb-core/tests/edit_check_test.rs`, `crates/code-kb-cli/tests/mcp_test.rs`, `.github/workflows/ci.yml` | Yes | Finalize the evidence and inventory after all implementation tasks. |

### Task 1: Pinned-language conformance

**Files:** Create `evals/languages/**`, `crates/code-kb-core/tests/language_conformance_test.rs`, and `docs/language-support.md`.

**Interfaces:** Consume the actual `julie-extract languages --json` report, existing scan/update APIs, and shared query/slicing operations. Produce a fixture inventory that detects pin drift and documents capabilities separately from observed behavior.

**Contract inputs:** All 42 current language IDs, including gomod/gosum/qmldir, and each format's real syntax. Gosum intentionally has no symbols; that is not a failed symbol search fixture.

**File ownership:** `evals/languages/**`, `crates/code-kb-core/tests/language_conformance_test.rs`, `docs/language-support.md`.

**Serialization required:** No.

**Dependency reason:** None - safe parallel batch.

**What to build:** Real-extractor navigation and refresh coverage for every pinned language, plus a truthful capability inventory. Exercise lookup, skeleton, bodies/context, references/impact, and edit checks where the fixture's constructs and extractor facts make each applicable. Classify missing applicable facts or consumer behavior as gaps, never as N/A.

**Approach:** Scan controlled fixtures, assert independently chosen symbols/locations or format-specific facts, edit content, and verify refreshed results. Derive the language-set assertion from the installed pinned extractor, not a duplicate unchecked constant. Send production defects to the lead with a real repro for ownership assignment.

**Acceptance criteria:**
- [x] Every current language has a meaningful real-extraction fixture and refresh assertion; a new pinned language without a fixture fails coverage.
- [x] Capability output distinguishes observed coverage, limited coverage, consumer gaps, missing facts, and semantic N/A, with reasons and test pointers.
- [x] Shared-operation behavior is verified through applicable executable and structured-format cases, including no-symbol behavior.
- [x] Dedicated tests pass; verified diff and evidence are handed to the lead.

### Task 2: Argument semantics across applicable languages

**Files:** Modify `edit_check/signatures.rs` and `reference_report/arity.rs`; create `tests/edit_check_arguments_test.rs`, all under `crates/code-kb-core/`.

**Interfaces:** Preserve Shape comparison and existing arity entry-point signatures. Consume extracted parameters/signatures and explicit call-site source only where existing architecture requires it.

**Contract inputs:** Named-argument rename and accepted argument-range semantics are separate capabilities. Existing strict languages and intentionally permissive JavaScript behavior must remain correct.

**File ownership:** `crates/code-kb-core/src/edit_check/signatures.rs`, `crates/code-kb-core/src/reference_report/arity.rs`, `crates/code-kb-core/tests/edit_check_arguments_test.rs`.

**Serialization required:** No.

**Dependency reason:** None - safe parallel batch.

**What to build:** Close the applicable named-argument and accepted-range consumer gaps identified by real extracted examples across language families. Preserve default, optional, variadic, receiver, annotation, external-label, and overload semantics; a syntax allowlist addition alone is insufficient.

**Approach:** Verify syntax from primary language documentation or local compiler behavior when uncertain. Add failing public edit-check/reference cases before each fix. Share lexical/fact readers where semantics agree and retain explicit differences where they do not.

**Acceptance criteria:**
- [x] Named-argument renames identify stale named uses in all applicable fixture languages and accept positional-only/local-name changes when valid.
- [x] Accepted ranges cover applicable extracted formal parameters without excluding valid optional/variadic/named calls or permissive-language calls.
- [x] Missing extraction evidence is reproduced and reported distinctly; no guessed proof or fabricated argument count.
- [x] Dedicated tests and relevant module unit tests pass; verified diff and evidence are handed to the lead.

### Task 3: Construction semantics across applicable types

**Files:** Modify `edit_check/fields.rs` and `reference_report/sections.rs`; create `tests/edit_check_fields_test.rs`, all under `crates/code-kb-core/`.

**Interfaces:** Preserve FieldChange and its existing construction-check entry points. A connection-aware private helper may add indexed constructor context and owner declaration lines when the extractor omits defaults; orchestration changes go through the lead/task 4 owner.

**Contract inputs:** Field-name change detection already works more broadly than construction reasoning. Constructor synthesis and custom constructors have different rules.

**File ownership:** `crates/code-kb-core/src/edit_check/fields.rs`, `crates/code-kb-core/src/reference_report/sections.rs`, `crates/code-kb-core/tests/edit_check_fields_test.rs`.

**Serialization required:** No.

**Dependency reason:** None - safe parallel batch; preserve FieldChange's existing interface.

**What to build:** Extend affected-construction reasoning beyond the existing Rust/Go/Python-dataclass/Java-record owners wherever analogous language semantics and Julie evidence permit it. Include records/data/case classes, synthesized memberwise initialization, and aggregate/designated initialization as applicable; do not assume every added field changes a constructor.

**Approach:** Compare real scanned before/after programs and verify construction sites and accepted unchanged cases. Resolve required/default/explicit-constructor differences from actual source facts and language semantics. Coordinate orchestration needs rather than editing shared files.

**Acceptance criteria:**
- [x] Applicable construction gaps have positive and negative real-extractor regressions across type/language families.
- [x] Valid defaults, custom constructors, unrelated same-name types, and non-breaking added members do not become proven failures.
- [x] Existing four owner families keep their tested behavior; unproven cases remain explicitly limited.
- [x] Dedicated tests and relevant module unit tests pass; verified diff and evidence are handed to the lead.

### Task 4: Removed member reads and callable value dependencies

**Files:** Modify `crates/code-kb-core/src/edit_check.rs`, `crates/code-kb-core/src/reference_report.rs`; create `crates/code-kb-core/tests/edit_check_members_test.rs` and an optional private `edit_check/members.rs` helper.

**Interfaces:** Consume finalized Shape/arity/FieldChange behavior and the shared reference evidence model. Preserve existing removal, overload, and Stop proof rules.

**Contract inputs:** Removed functions already flow through every_site, including extracted variable_ref rows. Signature/parameter changes currently filter those rows out. Field changes currently report constructions and omit reads.

**File ownership:** `crates/code-kb-core/src/edit_check.rs`, `crates/code-kb-core/src/reference_report.rs`, `crates/code-kb-core/tests/edit_check_members_test.rs`, optional new `crates/code-kb-core/src/edit_check/members.rs`.

**Serialization required:** No.

**Dependency reason:** None - safe parallel batch; this task is the single orchestration owner for explicit interface handoffs from tasks 2 and 3. Final verification consumes their completed semantics.

**What to build:** Report uses of removed members across applicable languages and surface changed-callable value/callback dependencies with appropriate uncertainty. Handle declarations versus writes and inherited definitions so a removal still satisfied by an ancestor is not a proven break.

**Approach:** Use existing member_access/variable_ref/type facts and shared receiver/scope evidence. Do not infer a callback invocation's argument count or claim that retaining a callback reference is necessarily invalid after a signature change.

**Acceptance criteria:**
- [x] Removed member reads and callable removals are found in real fixtures across applicable languages, with same-name/shadow/inheritance negatives.
- [x] Changed callable value dependencies are visible without becoming unsupported proven call failures or Stop blocks.
- [x] Dynamic attributes use actual extracted owners/evidence; missing fact contracts are concrete outstanding dependencies.
- [x] Dedicated tests and relevant existing edit/reference tests pass; verified diff and evidence are handed to the lead.

### Task 5: Actionable output recovery and test fallback

**Files:** Modify core `formatters.rs`, `test_commands.rs`, `queries.rs`, and `ops.rs`, CLI `main.rs` and `mcp/{server,protocol}.rs`; create `crates/code-kb-cli/tests/report_recovery_test.rs` and an optional core `report_pages.rs` helper.

**Interfaces:** Existing seven MCP tools and exact CLI counterparts. Add compatible optional recovery controls to existing reports only when needed, keeping default summaries compact. New core exports are integrated by the lead.

**Contract inputs:** Counts must remain complete, pages bounded and deterministically ordered, project roots/filters fixed by each call. Each page sees the current index; document restart behavior after changes. Never retain a report/session repository graph in RAM.

**File ownership:** `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-core/src/test_commands.rs`, `crates/code-kb-core/src/queries.rs`, `crates/code-kb-core/src/ops.rs`, `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/src/mcp/protocol.rs`, `crates/code-kb-cli/tests/report_recovery_test.rs`, optional new `crates/code-kb-core/src/report_pages.rs`.

**Serialization required:** No.

**Dependency reason:** None - safe parallel batch; new core exports are integrated by the lead.

**What to build:** Make folded reference/edit/impact details retrievable through MCP with equivalent CLI controls. Give incomplete test coverage a verified broader command when runner/config evidence exists; otherwise name the broader scope and missing evidence.

**Approach:** Reuse shared report operations and formatters. Keep additive schemas, stable result ordering, explicit bounds/zero/invalid/past-end behavior, and current-index paging semantics. Detect commands from repository configuration, not source-language guesses; quote paths for the intended shell and preserve working-directory context.

**Acceptance criteria:**
- [x] An agent can retrieve previously folded report rows without switching to CLI JSON; bounds, count completeness, filters, ordering, and CLI/MCP equivalence are tested.
- [x] Default output remains compact and compatible; removed tool names and root invariants remain unchanged.
- [x] Broader commands execute relevant tests in controlled real runner/config fixtures; unsupported runners produce honest scope advice.
- [x] Dedicated tests pass; verified diff and evidence are handed to the lead.

### Task 6: Trustworthy evaluation and release evidence

**Files:** Modify `scripts/release-preflight.sh`, `scripts/release-dogfood.sh`, `scripts/reach_gate.py`, `scripts/test_reach_gate.py`, `docs/RELEASING.md`, and `docs/plans/2026-09-23-retrieval-value-and-optional-routing.md`; create `scripts/test_release_evidence.py`, optional `scripts/release_evidence.py`, and fresh fixtures under `evals/review-2026-10-26/`.

**Interfaces:** Existing release reports/preflight and Python harness CI. Produce source-backed fingerprints and reproducible local regression evidence without paid model runs.

**Contract inputs:** An ancestor PASS alone cannot validate semantic changes. Fingerprints cover relevant source, extractor identity/level, fixtures, grading logic, and test-command configuration, while allowing explicitly defined docs/version-only changes. Preserve the documented one-full-dogfood-round policy: later source fixes require explicit fix/reproduction evidence; an arbitrary later semantic commit is not covered merely because the original round is an ancestor.

**File ownership:** `scripts/release-preflight.sh`, `scripts/release-dogfood.sh`, `scripts/reach_gate.py`, `scripts/test_reach_gate.py`, `scripts/test_release_evidence.py`, `scripts/release-evidence-inputs.json`, optional new `scripts/release_evidence.py`, `docs/RELEASING.md`, `docs/plans/2026-09-23-retrieval-value-and-optional-routing.md`, dedicated fresh local evaluation fixtures under `evals/review-2026-10-26/**`.

**Serialization required:** No.

**Dependency reason:** None - safe parallel batch.

**What to build:** Bind release evidence to its semantic inputs, modernize the controlled comparison plan, and add fresh source-backed local task cases across languages for the review's retrieval/test-reach gaps. Keep historical regressions distinct from new held-out cases and measured agent outcomes.

**Approach:** Share one fingerprint implementation rather than duplicating hashing in shell and Python. Exercise actual report production/validation with controlled Git histories and changed files. Reuse existing benchmark/replay interfaces only after confirming their current shapes.

**Acceptance criteria:**
- [x] Relevant semantic/fixture/grader/extractor changes invalidate old PASS evidence; permitted docs/version-only changes do not.
- [x] Missing/malformed/future/mismatched evidence fails with an actionable reason; current valid evidence passes.
- [x] Fresh local cross-language task cases and an executable updated comparison procedure are recorded without claiming a paid agent comparison ran.
- [x] Dedicated Python tests pass; verified diff and evidence are handed to the lead.

### Task 7: Integration, guidance, and final verification

**Files:** Lead-owned guidance, user docs, review/plan, memory, core exports, and CI from the ownership table.

**Interfaces:** Integrate reviewed worker outputs and document actual capability coverage and limitations.

**Contract inputs:** The implementation request, all previous task evidence, and the complete supported-language inventory.

**File ownership:** `AGENTS.md`, `CLAUDE.md`, `README.md`, `docs/edit-check.md`, `docs/argument-counts.md`, `docs/reach-sections.md`, review/plan files, `.memories/**`, `crates/code-kb-core/src/lib.rs`, `crates/code-kb-core/tests/edit_check_test.rs`, `crates/code-kb-cli/tests/mcp_test.rs`, `.github/workflows/ci.yml`.

**Serialization required:** Yes.

**Dependency reason:** Finalize the evidence and inventory after all implementation tasks.

**What to build:** Explicit all-language guidance, accurate feature documentation, reconciled coverage inventory, and a verified integrated branch. Fix all actionable consumer defects discovered in the assigned scope; do not turn a missing applicable feature into a completed inventory row.

**Approach:** Review each task against its actual diff and evidence, integrate shared interfaces, run the narrow affected scopes and one final branch gate, measure live MCP memory, and reconcile every involved worktree before committing/reporting.

**Acceptance criteria:**
- [x] Both guidance files explicitly require all pinned languages and stay byte-identical; docs match verified behavior.
- [x] Every identified applicable gap is fixed or has a genuine externally blocked dependency with a concrete repro and required action.
- [x] Branch gates and post-edit checks pass, and live memory/output evidence is recorded without overstating agent outcomes.
- [x] Intentional local commits include plan/docs/memory; all task state is reconciled and no push/release occurs.


### Task 8: Extractor fact compatibility

**Files:** Modify `crates/code-kb-core/src/db.rs` and `crates/code-kb-core/src/sync.rs`; create `crates/code-kb-core/tests/extractor_compat_test.rs`.

**Interfaces:** Preserve existing public scan/update/query APIs and symbol IDs. Apply narrow normalization to already-extracted facts through the shared index lifecycle; source spans and written signatures remain intact.

**Contract inputs:** Real CSS `@function --anchor_css() { result: 41; }` is extracted by pinned Julie as a namespace named `@function --anchor_css()`, while its call identifier is `--anchor_css`. The declaration signature carries an unambiguous function name. Current CSS syntax is grounded in https://drafts.csswg.org/css-mixins-1/ . Ordinary selectors/custom properties remain unaffected.

**File ownership:** `crates/code-kb-core/src/db.rs`, `crates/code-kb-core/src/sync.rs`, `crates/code-kb-core/tests/extractor_compat_test.rs`.

**Serialization required:** No.

**Dependency reason:** None - safe parallel batch; preserve existing public lifecycle APIs.

**What to build:** Normalize the pinned extractor's explicit custom-function declaration facts so lookup and reference consumers agree on names and callable kinds. Apply the same correction to existing indexes, new scans, worktree copies, and incremental updates without a new extractor release or a source-text parser.

**Approach:** Add a failing real-extractor public lookup/reference/body case first. Reuse the existing lifecycle post-processing shape used for implementation owner linking, giving any broader shared helper an accurate name. Normalize only explicit matching CSS declarations, preserve IDs/spans/signatures, make the operation idempotent and bounded, and avoid a separate retained repository collection. Coordinate reference classifier changes with task 4 and skeleton rendering with task 5.

**Acceptance criteria:**
- [x] Explicit custom functions are looked up by their callable name, and extracted call identifiers can reach their matching declarations.
- [x] Source body spans and ordinary selectors/properties/at-rules remain correct; ambiguous/unsupported forms are not guessed.
- [x] Existing-index and incremental update tests prove correction is repeatable and survives rewrites without stale names.
- [x] Dedicated tests pass; verified diff and evidence are handed to the lead.

## Execution result

- Runtime implementation: `2104bad09903a3708dd8c620cd75eb379ad7d286`; Windows test-fixture portability: `de67a398db6b1e92d3bb244d6f758fce789abd17`.
- Linux: 1,818 Rust, 172 Python, and 20 plugin tests passed; 391 post-lint edit tests and two final fixture checks passed. Clippy, formatting, release build, whitespace, guidance synchronization, and post-edit review passed.
- Windows NTFS: 1,794 Rust tests across 34 suites passed, including all 42-language conformance cases. Plugin tests: 12 passed, eight platform-specific skips. The 59 previously failing argument cases and both real runner fixtures passed before the full rerun. Windows Clippy with denied warnings, formatting, and the locked release build also passed.
- Same-root live MCP retained RSS: 23.64 MiB before, 24.46 MiB after, for 20 calls per fresh process. This is local memory evidence, not an agent outcome.
- Remaining dependencies are recorded with reproductions in `docs/language-support.md`: missing CSS call/formal facts, lossy Unicode declaration spelling, and a dialect-safe SQL argument contract. Curried F# and framework invocation remain conservative. Eight controlled comparison tasks are prepared; no paid model run, push, PR publication, or release was performed.
