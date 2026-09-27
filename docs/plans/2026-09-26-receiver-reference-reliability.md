# Receiver reference reliability implementation plan

> Execute with `razorback:executing-plans`. These tasks share query rules and run in order. Use a read-only reviewer for the plan and final diff when useful.

**Goal:** Stop attributing a method reference to the wrong receiver, identify unresolved member references as candidates, and diagnose the existing expensive reference and impact queries.

**Architecture:** Keep resolution in the SQLite query layer. Reuse the receiver evidence already used by pending calls when filtering member-access identifiers. Keep the existing query APIs and `ReferenceSite` shape; return uncertainty through the existing `kind` label.

**Tech stack:** Rust, rusqlite, SQLite schema v7, julie-extract 3.6.3, existing Rust integration tests and Python benchmark helpers.

**Spec:** [September 26 evaluation, priority 1](../reviews/2026-09-26-project-evaluation.md#five-improvements-in-priority-order), narrowed by the owner's request to start with receiver disambiguation. The behavior contract below specifies this first slice. The full priority also includes other inference explanations and latency improvements; this slice does not complete it.

**Architecture quality:** The affected implementation is `crates/code-kb-core/src/queries.rs`. Caller-facing interfaces remain `find_references_scoped`, `find_references_for_symbol`, `get_context_slice_selected_op`, and the existing blast-radius operations. Tests exercise those interfaces and CLI/MCP results. Share only the receiver predicates needed by both query paths; do not split the whole module or introduce a generic resolver. The main risk is treating a failed heuristic as proof of a different type, losing valid callers. Risk is medium.

## Worktree and authority

- Worktree: `/home/murphy/source/code-kb/.worktrees/receiver-reference-reliability`.
- Branch: `fix/receiver-reference-reliability`.
- Baseline: `7ceb734be947aced7cedf1ea179fbce3c9987c88`.
- Planning started with this worktree clean. The main checkout was also clean.
- `implementation_authority`: authorized by the owner's "approved" response to this plan.
- `local_commit_authority`: authorized for implementation, verification, documentation, and memory artifacts within this plan.
- `push_authority`, `pr_authority`, and release authority: missing. Do not publish as part of local execution.
- External reviewer choice for execution: `none`, unless the approval message selects one.

## Current behavior and evidence

| Existing code | What matters for this change |
|---|---|
| `pending_target_predicate` in `queries.rs` | Checks receiver types, local builders, fixtures, imports, inheritance, and nearest overrides for pending calls. A false result alone does not prove incompatibility. |
| `find_direct_references` in `queries.rs` | Collects relationships, pending calls, and identifiers. Its ordinary member-access checks do not share the local-variable and fixture inference. |
| `merge_same_site` in `queries.rs` | Includes `kind` in its deduplication key. Candidate labels must not create duplicate rows for one piece of evidence. |
| `find_related_tests` in `queries.rs` | Uses caller rows, plus separate name and full-text heuristics. A candidate reference must not silently become a confirmed reference-based test association. |
| `compute_blast_radius_scoped_with_ids` and `implicit_entry_tests` | Primarily follow relationships and pending calls. They do not generally traverse member-access identifiers. Preserve that distinction. |
| `ReferenceSite` in `models.rs`; `format_references` in `formatters.rs` | Text and JSON already expose `kind`, including `handler (candidate)`. No new output field is needed for this slice. |

The motivating Flask case is `FlaskCliRunner.invoke` in `src/flask/testing.py`.
The fixture `runner` in `tests/test_cli.py` constructs imported
`click.testing.CliRunner`. An instance of that base class does not call the
subclass override merely because both have a method named `invoke`.
Reproduce the reported false match before changing the implementation.

Existing tests already cover local builders, fixture builders, inheritance,
same-name methods, QML handlers, scoped selection, and constructor impact. Extend
those tests rather than building another fixture framework.

## Behavior contract

This contract applies to ordinary member-access identifiers targeting callable
class members. Preserve the separate rules for fields, properties, type usages,
QML handlers, and grouped references to a type.

| Receiver evidence | Caller result |
|---|---|
| Identifies the selected owner, or an inherited method selected by the nearest-definition rule | Keep `kind: "member_access"`. |
| Identifies another type whose applicable method is a different definition | Omit the row for this target. |
| Identifies a base-class instance while the selected target is a subclass override | Omit the row for this target. |
| Insufficient or conflicting evidence | Keep `kind: "member_access (candidate)"`. |

- Use symbol identity, source module, lexical scope, and fixture scope where available. Equal class names in different modules are not interchangeable.
- Match `self`, `this`, `cls`, and `super` using existing inheritance rules, including nearer overrides. A subclass can use a base method; a base instance cannot use a subclass override.
- A missing type fact, an unindexed import, or a builder whose return type is unknown is unresolved. An arbitrary imported factory is not proof of a foreign receiver type. The Flask case has additional evidence: the imported constructor also names the subclass's declared base.
- If rebinding or multiple possible builders prevent selecting a receiver, retain a candidate. Do not select whichever assignment happens to appear first in the database.
- Preserve resolved relationship precedence and same-site suppression. A supported relationship must not gain a second candidate row from its identifier.
- Apply receiver filtering before the output limit. Within identifier results, supported rows precede candidates, then existing source ordering breaks ties. Keep scope filters, symbol-ID selection, zero limits, and cap notices working.
- Candidate-only identifier evidence remains visible in references. It must not create a call edge in impact traversal or enter context's related-test list through the reference stage. Existing independent name/full-text test suggestions remain unchanged.
- Callees and context's callee signatures continue using the shared pending-call rules. Preserve `include_external`; an unknown receiver must not become a confidently resolved target there.

## Global constraints

- Keep direct SQLite queries and bounded result handling. Do not hydrate repository graphs or file lists into memory.
- Keep MCP/CLI parity, required `project_root`, path validation, and Windows path handling.
- Preserve extractor 3.6.3 and schema v7. No new dependency, index migration, cache, watcher, or extractor change is planned.
- Keep `AGENTS.md` and `CLAUDE.md` byte-for-byte equivalent when documenting the changed reference rules.
- Keep production edits focused on receiver matching and its consumers. Broader inference provenance and module extraction need separate evidence and scope.
- Diagnose performance before proposing an optimization. This plan produces the diagnosis and a concrete follow-up recommendation, not an unmeasured SQL rewrite.
- The complete-task token/context study remains priority 5. These local correctness and latency checks do not start that study.
- Do not modify the original Flask or Hermes source trees. Flask currently has unrelated `.julie/`, `.julieignore`, and `.miller/` files. Preserve them.

## Verification strategy

**Project source of truth:** `AGENTS.md`, `.github/workflows/ci.yml`, `docs/RELEASING.md`, and `scripts/julie-pins.json`.

**Preparation:** From the task worktree, run `bash scripts/restore-julie-extract.sh`. Run the existing affected test binaries once before implementation and save their output. The planning session has not run a source baseline suite.

**Worker red/green scope:** A named integration test, for example `cargo test -p code-kb-core --locked --test references_test member_access_rejects_a_different_receiver -- --exact`. New names below are test names to create, not existing tests.

**Worker ceiling:** The affected core test binaries and the named CLI/MCP tests. The lead owns the full branch gate.

**Worker gate invariant:** Known wrong receivers are absent, supported callers survive, and uncertainty remains visible. Tests assert both required and forbidden rows, not just counts.

**Lead affected-change scope:**

```sh
cargo test -p code-kb-core --locked --test references_test --test resolution_test --test disambiguation_test --test blast_radius_test --test qt_qml_test --test qt_cpp_test
cargo test -p code-kb-cli --locked --test cli_test --test mcp_test
```

**Branch gate:** Run once on the completed code changes, retaining logs. Fix a failure and rerun that failing test before repeating its broader gate.

```sh
cargo fmt --all -- --check
cmp AGENTS.md CLAUDE.md
cmp skills/code-kb/SKILL.md .claude-plugin/skills/code-kb/SKILL.md
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
node --test tests/plugin/*.test.cjs
git diff --check
```

Run the affected core tests and the new protocol tests on Windows/NTFS using the
`win-test` skill, with the task worktree as the synced source. Record the actual
commit and command. An unavailable Windows guest is a reported verification gap;
do not claim a Windows pass or push merely to obtain CI.

**Security scope:** none declared. This change adds no dependency or external service and preserves existing project-root and path validation.

**Replay/metric evidence:** Expected and forbidden receiver matches are hard gates. Report first-request latency, warm median/p95, result counts, and live server memory separately. Any repeatable slowdown or memory growth introduced by this change requires diagnosis before completion; do not claim a speedup from fewer incorrect or omitted rows.

**Escalation triggers:** A need to change the stored schema, extractor, public data shape, or unrelated inference rules is a plan mismatch. Stop that change, state the missing evidence, and revise scope. A flaky or failing assigned test requires investigation and repair, not weaker assertions.

**Verification ledger:** Record command, invariant, scope, source commit, dirty state, timestamp, and result in the final report. Reuse a passing result on unchanged code. Keep raw logs under ignored `target/receiver-reference-reliability/`.

## Parallel execution contract

Commit mode is `serial-worker-commit`. One implementer completes these tasks in
order; the shared SQL and output contract do not justify parallel edits.

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| 1. Reject proven receiver mismatches | None, serial | `crates/code-kb-core/src/queries.rs`; `crates/code-kb-core/tests/references_test.rs`; `crates/code-kb-core/tests/resolution_test.rs` | Yes | Establish receiver evidence before labeling or consuming unresolved rows. |
| 2. Expose uncertainty and verify consumers | None, serial | `crates/code-kb-core/src/queries.rs`; `crates/code-kb-core/tests/references_test.rs`; `crates/code-kb-core/tests/disambiguation_test.rs`; `crates/code-kb-cli/tests/cli_test.rs`; `crates/code-kb-cli/tests/mcp_test.rs`; `AGENTS.md`; `CLAUDE.md` | Yes | Depends on Task 1 and edits the same query path. |
| 3. Measure and diagnose difficult queries | None, serial | `docs/reviews/2026-09-26-receiver-reference-reliability.md`; `docs/reviews/2026-09-26-receiver-reference-reliability-data.json` | Yes | Measures the completed behavior against the fixed baseline. |

Each task may update its checkboxes and verification notes in this plan. Save
consequential decisions in `.memories/` before committing. Check path, branch,
commit, status, and worktree inventory before each commit and final handoff.

## Task 1: Reject proven receiver mismatches

**Files:** Modify `crates/code-kb-core/src/queries.rs`; test in `crates/code-kb-core/tests/references_test.rs` and `crates/code-kb-core/tests/resolution_test.rs`.

**Interfaces:** Consume the existing `pending_target_predicate`, indexed symbols/type facts/imports, and identifier metadata. Produce corrected rows through `find_references_scoped` and `find_references_for_symbol`, without changing their signatures or `ReferenceSite`.

**Contract inputs:** The behavior contract; `scanned_repo` and the existing fixture/local-builder tests in `references_test.rs`; `callers_use_the_receiver_variable_type_to_pick_the_method` in `resolution_test.rs`.

**File ownership:** `crates/code-kb-core/src/queries.rs`; `crates/code-kb-core/tests/references_test.rs`; `crates/code-kb-core/tests/resolution_test.rs`

**Serialization required:** Yes

**Dependency reason:** Establish receiver evidence before labeling or consuming unresolved rows.

1. Add `member_access_rejects_a_different_receiver` using a scanned Python fixture with two owners of `invoke`, a local builder, and a pytest fixture. Assert exact positive and forbidden caller sites for each selected method. Include a member access that reaches the identifier fallback, rather than testing only a pending call.
2. Add `a_base_instance_does_not_reference_a_subclass_override`, modeling an imported base constructor, its local subclass override, and both base and subclass receivers. Include the external-base case from Flask. Preserve legitimate inherited calls and nearest-override selection.
3. Run `cargo test -p code-kb-core --locked --test references_test member_access_rejects_a_different_receiver -- --exact` and `cargo test -p code-kb-core --locked --test references_test a_base_instance_does_not_reference_a_subclass_override -- --exact`. Confirm a wrong returned row causes the failure. If scanned fixtures do not produce the required identifier shape, inspect the extracted rows and use the existing controlled-row pattern from the QML tests for a focused fallback test; retain a scanned end-to-end reproduction.
4. Share the minimum receiver-evidence SQL between pending-call matching and member-access filtering. Distinguish supported, incompatible, and unresolved evidence internally; negating the current predicate is insufficient. Preserve optional-column checks and existing fixture query-plan constraints. Add cases for same-name classes in different modules and unrelated fixture scopes to the existing test files.
5. Run `cargo test -p code-kb-core --locked --test references_test --test resolution_test`. Inspect callers, callees, and constructor/inheritance assertions for unintended changes. Apply commit mode after these pass.

**Acceptance criteria:**

- [x] The baseline demonstrates the reported wrong-receiver behavior through a failing assertion.
- [x] Local, fixture, typed, and inherited supported references survive; proven mismatches disappear.
- [x] Imported base instances do not reference subclass overrides, including the Flask-shaped case.
- [x] Scope and module identity prevent cross-matching unrelated same-name classes or fixtures.
- [x] The focused tests pass and the commit SHA is recorded.

## Task 2: Expose uncertainty and verify consumers

**Files:** Modify `crates/code-kb-core/src/queries.rs`, `AGENTS.md`, and `CLAUDE.md`. Test in `crates/code-kb-core/tests/references_test.rs`, `crates/code-kb-core/tests/disambiguation_test.rs`, `crates/code-kb-cli/tests/cli_test.rs`, and `crates/code-kb-cli/tests/mcp_test.rs`.

**Interfaces:** Consume Task 1's receiver evidence. Produce `member_access (candidate)` through the existing `ReferenceSite.kind`, CLI JSON/text, and MCP text. Keep `format_references`, `ContextSlice`, and public operation signatures unchanged.

**Contract inputs:** Candidate and consumer rules above; `merge_same_site`; `find_related_tests`; existing `get_context_slice_selected_op` and blast-radius operations; existing CLI process helpers and `McpSession` in `mcp_test.rs`.

**File ownership:** `crates/code-kb-core/src/queries.rs`; `crates/code-kb-core/tests/references_test.rs`; `crates/code-kb-core/tests/disambiguation_test.rs`; `crates/code-kb-cli/tests/cli_test.rs`; `crates/code-kb-cli/tests/mcp_test.rs`; `AGENTS.md`; `CLAUDE.md`

**Serialization required:** Yes

**Dependency reason:** Depends on Task 1 and edits the same query path.

1. Add `unresolved_member_access_is_a_candidate` and `supported_member_access_precedes_candidates_at_the_limit` in `references_test.rs`. Cover an unknown parameter, an untyped imported factory, ambiguous rebinding, supported rows after rejected rows, `limit=0`, a small positive limit, a path filter, and exact symbol-ID selection. Assert that a same-site relationship plus identifier still yields one row.
2. Run the new reference tests with `cargo test -p code-kb-core --locked --test references_test member_access`. Expect failures for missing candidate labels or incorrect limit ordering, not setup failures.
3. Emit the specified candidate kind and apply classification before limits and deduplication. In `find_related_tests`, prevent candidate-only member-access evidence from entering via the reference stage. Do not change independent name/full-text heuristics or add identifier edges to blast radius.
4. Add `candidate_member_access_is_not_a_confirmed_related_test` in `disambiguation_test.rs`. Use colliding method names so the unique-name text fallback does not obscure the reference-stage assertion. Assert supported fixture-related tests remain. Add a separate unique-name case that proves an independently justified name/full-text test suggestion survives candidate filtering. Exercise callees/context with `include_external` both ways, and check that existing blast results retain valid call edges without gaining candidate-only ones.
5. Add `receiver_reference_labels_match_the_cli_contract` in `cli_test.rs` and `receiver_reference_labels_match_the_mcp_contract` in `mcp_test.rs`. Use existing process/session patterns. Assert the same required/forbidden sites and candidate label in CLI text/JSON and MCP output. These are contract checks and may pass on their first run once the core implementation is correct.
6. Update the reference rules in both agent-guidance files with the supported/incompatible/unresolved behavior and candidate-only consumer policy. Run the affected-change commands and both `cmp` checks from the verification strategy. Apply commit mode.

**Acceptance criteria:**

- [x] Unknown evidence is visible as `member_access (candidate)` in CLI and MCP results.
- [x] Supported identifier rows are not displaced by candidates or rejected rows at the limit.
- [x] No new duplicates, lost scope filters, or symbol-ID ambiguity occur.
- [x] Candidate-only evidence does not become a confirmed reference-based test association or impact edge.
- [x] Existing fields, type usages, QML handlers, constructor reachability, and external-callee behavior pass their regression checks.
- [x] Agent guidance matches byte-for-byte; the affected checks pass and the commit SHA is recorded.

## Task 3: Measure and diagnose difficult queries

**Files:** Create `docs/reviews/2026-09-26-receiver-reference-reliability.md` and `docs/reviews/2026-09-26-receiver-reference-reliability-data.json`. Raw data, temporary instrumentation, and the one-off driver belong under ignored `target/receiver-reference-reliability/`.

**Interfaces:** Consume the baseline and completed candidate binaries; reuse `PersistentMcpClient.call_tool` and `get_live_memory` from `scripts/benchmark_quality.py`. Produce a checked-in report and compact JSON with workload, binary/corpus identities, all timing samples, result counts, memory samples, and diagnosis.

**Contract inputs:** [v2.2.2 release notes](../release-notes/v2.2.2.md) and [the earlier fixture-resolution checkpoint](../../.memories/2026-09-25/155504_a7b9.md). The notes record 18-second references and 10.7-second file impact on 988k symbols. The checkpoint names references `get` and context `execute`; it does not identify the old blast file or all selection parameters.

**File ownership:** `docs/reviews/2026-09-26-receiver-reference-reliability.md`; `docs/reviews/2026-09-26-receiver-reference-reliability-data.json`

**Serialization required:** Yes

**Dependency reason:** Measures the completed behavior against the fixed baseline.

This is an evidence task. It does not require a fabricated failing test or an optimization commit.

1. Build the baseline at `7ceb734be947aced7cedf1ea179fbce3c9987c88` in an isolated source export, and the candidate in the task worktree, with `cargo build --release --locked` after restoring the pinned extractor in each. Record source commits, binary hashes, extractor version, SQLite version, and machine details. Keep baseline build output separate from the candidate.
2. Export frozen tracked corpus snapshots into the ignored artifact directory. Available source revisions are Flask `d73fa1cdcbd8b1465c151db8924ba58b1dd14e35` and Hermes `8706517544bcc3f41f3b6521725f80b349ead3c7`. Index the copies with the pinned extractor, record indexed file/symbol counts, and use equivalent fresh database copies for both binaries. The current Hermes index has 988,219 symbols but no recorded source commit; do not assume a tracked export reproduces its population or the old timings. Report differences before interpreting comparisons.
3. Freeze these tool arguments in the JSON before timing, substituting only the absolute snapshot root for each run:

   | Corpus | Tool | Arguments in addition to `project_root` |
   |---|---|---|
   | Flask | `find_references` | `symbol_name="FlaskCliRunner.invoke"`, `direction="callers"`, `limit=200` |
   | Flask | `get_symbol_context` | `symbol_name="FlaskCliRunner.invoke"` |
   | Hermes | `find_references` | `symbol_name="get"`, `direction="callers"`, `limit=20` |
   | Hermes | `get_symbol_context` | `symbol_name="execute"` |
   | Hermes | `blast_radius` | `file="run_agent.py"`, `depth=2`, `limit=20` |

   Record the selected definition identity for each ambiguous name. Add its qualified name or freshly selected ID as a paired disambiguated probe when needed. Do not reuse IDs across independently built indexes. `run_agent.py` is a new fixed workload, not a recovered historical blast target.

4. Import the existing persistent MCP client in a one-off driver. Do not run `benchmark_quality.py` unchanged on these corpora: its workload targets code-kb symbols and writes a probe file. Separate process/index startup from tool calls. Require a completed query before warm-up; record `Indexing ...` responses separately and allow at most 300 seconds for readiness. Enforce a 120-second request deadline in the driver, since the client itself has no timeout. Save the first completed request separately, discard one additional warm-up, then collect 20 measured samples per workload and binary, alternating baseline/candidate session order. Use `statistics.median` and the existing nearest-rank `percentile(samples, 0.95)` helper. Capture live RSS/PSS/anonymous memory before and after the workload, marking unavailable readings as unavailable. Store errors and timeouts as outcomes, clean up the server process, and do not report successful-sample p95 as the all-request p95 when requests fail.
5. Check the Flask caller changes against the scanned fixture and source locations. Report required and forbidden edge counts and precision/recall on the labeled cases from Tasks 1 and 2, separating supported rows from unresolved candidates. These are regression-case measurements, not project-wide quality estimates. Compare result sets alongside latency, so dropped valid edges cannot masquerade as an improvement.
6. Identify the slowest reproducible query stage. Capture its actual SQL and bound parameters with temporary local instrumentation, then run `EXPLAIN QUERY PLAN` against a read-only database copy. Separate SQL time from reconciliation and protocol time. Check for repeated scans and automatic-index construction around fixture/type resolution; preserve earlier `CROSS JOIN` and lifecycle-index constraints unless the evidence contradicts them. Remove instrumentation before branch verification.
7. Write the report with the exact one-off driver and rerun commands, raw-sample links, first-request/median/p95 timings, memory, correctness changes, the responsible query stage, and the smallest evidence-supported follow-up. If the old latency does not reproduce, say so with the corpus differences. Run the branch gate, record Windows verification or its actual blocker, validate JSON with `python3 -m json.tool docs/reviews/2026-09-26-receiver-reference-reliability-data.json`, review the final diff, and apply commit mode.

**Acceptance criteria:**

- [x] Baseline and candidate use the same declared corpus content, extractor, workload, and measurement procedure.
- [x] Readiness responses cannot enter query timings; request deadlines and percentile calculation are recorded.
- [x] The report distinguishes historical observations from new measurements and records ambiguous target selection.
- [x] Correctness, timing samples, result counts, and live retained memory are available together.
- [x] A query-plan diagnosis or a documented failure to reproduce supports the next performance step.
- [x] Temporary instrumentation is removed; branch verification and remaining gaps are recorded honestly.
- [x] Report, data, plan status, and consequential memory are committed locally; nothing is pushed or released.

## Definition of done

The known receiver mismatch is reproduced and fixed, legitimate references
remain, uncertain member references are labeled consistently, and affected
consumers obey the contract. The report gives measured correctness and latency
for this slice and identifies the next justified performance action. The plan's
checks are complete or name a genuine environmental blocker. Priority 1 remains
open for its remaining inference explanations and any diagnosed optimization.

## Execution notes

- The owner approved local execution. No external reviewer or publication authority was selected.
- Baseline affected checks passed: 124 core tests and 90 CLI/MCP tests. Logs are under `target/receiver-reference-reliability/`.
- Task 1 landed locally as `c445c8840e69628da60f9e04a275c540bd5f6eac`. Regressions failed with wrong caller rows before the fixes; its final reference/resolution scope passed 58 tests.
- Shared builder inference now respects local-variable shadowing, import modules, and nearest inherited overrides.
- The extractor supplies no receiver metadata for a bare `super().invoke` member read. Task 1 verifies the supported `super().invoke()` call; Task 2 retains a read without metadata as a candidate. This follows the unknown-evidence contract instead of inventing a receiver.
- Task 2's final affected checks passed 135 core tests and 92 CLI/MCP tests, plus both guidance-copy checks. The report's verification ledger records its source commit.
- Read-only implementation review exposed future assignments influencing earlier reads and mixed known/unknown builders being treated as supported. Both were reproduced with failing assertions and fixed before the affected rerun. Multiple bindings remain conservative candidates; this slice adds no control-flow analysis.
- Task 2 landed as `ba2f890478e13ef539269c1764de19351e3d1dba`; 545 Rust tests, 19 plugin tests, formatting, Clippy, and Windows's 135 affected core plus two new protocol tests passed on that source.
- Task 3's first SQL profile exposed an introduced regression: scoped Hermes `get` rose from 18.08 s to 48.88 s. Module-scope checks were evaluated for builders with unusable return types. A captured-query experiment using a lazy `CASE` gate reduced its pending-call stage from 44.17 s to 17.05 s with identical rows. This small correction is required by the regression check; the broader performance work remains diagnosis only. Affected tests passed after applying it; final verification and paired results follow.
- Baseline Hermes name-only `get` and `execute` probes report ambiguity. Task 3 will retain those outcomes and add explicitly scoped probes in `agent/relay_runtime.py` and `agent/relay_tools.py`, without claiming they reproduce the unknown historical selections.
- Task 3 is complete. [The report](../reviews/2026-09-26-receiver-reference-reliability.md) and its data file record verified source `f804a790b86fd715207a52feb6c52f1d704db010`, exact drivers, corpus identities, all 280 outcomes, SQL plans, and the verification ledger. Final source passed 545 Rust tests, 19 plugin tests, and Windows/NTFS's 135 affected core plus two protocol tests, formatting, Clippy, and guidance comparisons.
- The labeled regression set keeps all 15 required rows, removes all 14 formerly returned forbidden rows, and labels all 14 unresolved rows. These are regression fixtures, not a project-wide precision estimate.
- The 20-sample paired medians are 18.52→20.50 s for scoped Hermes references and 6.17→8.27 s for blast on `run_agent.py`. Correctness improves at a measured latency cost. The dominant direct SQL stages are pending matching at 15.81 s and member identifiers at 4.07 s. Reusing builder/origin/ancestor facts inside the query is the next justified experiment; this plan does not claim that optimization is complete.
- The owner flagged 20 seconds as too slow and requested disk-I/O and CPU measurements. Three warm requests per arm use about 99.7% of one logical core, read zero bytes from storage, and perform about 10.5 million read system calls returning 40.1 GiB in the candidate. Peak RSS near 278 MiB is mostly the existing 256 MiB database mapping; retained memory is reported separately. The report includes the samples and their limits.
- Final read-only evidence review checked metrics, embedded driver hashes, and claim scope without finding additional actionable issues. Only docs and memory changed after the verified production source. Priority 1 remains open for performance work and additional inference explanations; the complete-task token/context study remains priority 5.
