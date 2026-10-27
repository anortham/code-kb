# Task 6 report: local edit-evaluation orchestration

Prepared 2026-10-27 15:01:28 UTC. The checks below ran in this worktree during this session; the command runner did not expose their exact start time. Their command output is retained here.

## Result

Implemented the local case, readiness, fake-agent, replay-grading, manifest-validation, and outcome-summary runner. No provider dispatch was added. Upstream source is exported from the exact pinned commit and is only executed inside the reviewed sandbox. Synthetic repositories are used for runner tests; no scored corpus was run by this worker.

Readiness writes `OUT/readiness.json`. A ready record points to `work/seed-project` and `evidence/reference-bundle`, relative to `OUT`, for lead-owned fresh replay grading. `source_review` remains `pending` until source review.

## Interfaces

Python entry points:

```python
validate_case(case_dir, *, corpus_path=DEFAULT_CORPUS, verify_assets=True)
export_source(repository, commit, destination)
prepare_case_readiness(case_dir, source_root, output_root, *, tools=None, dependencies=None, corpus_path=DEFAULT_CORPUS)
run_fake_agent(case_dir, source_root, attempt_dir, argv, *, tools=None, dependencies=None, timeout=60, corpus_path=DEFAULT_CORPUS)
replay_and_grade(case_dir, seed_project, bundle_dir, grade_dir, *, tools=None, dependencies=None, trial_completed=True, corpus_path=DEFAULT_CORPUS)
validate_manifest(manifest_path, *, corpus_path=DEFAULT_CORPUS, artifact_paths=None)
summarize_campaign(records)
main(arguments=None)
```

The readiness record exposes a seed-only Git tree and a captured reference bundle. The lead can grade it with:

```bash
python3 scripts/edit_evaluation.py grade-bundle \
  --case-dir CASE \
  --seed-project OUT/work/seed-project \
  --bundle OUT/evidence/reference-bundle \
  --output GRADE \
  --corpus CORPUS
```

Repeat `--dependency NAME=ABS_PATH` and `--tool NAME=ABS_PATH` for declared runtime inputs. Manifest validation accepts repeated `--artifact NAME=ABS_PATH`; every SHA control requires a path. Runner, snapshot, sandbox, protocol, and corpus overrides must resolve to the active source files, and all override paths must be absolute, non-symlink files or directories with matching hashes.

The CLI commands are `validate-case`, `export-source`, `readiness`, `fake-agent`, `grade-bundle`, `validate-manifest`, and `summarize`.

## Red/green evidence

The following focused commands first reproduced contract gaps, then passed after the fixes:

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_frozen_manifest_binds_six_ready_cases_controls_and_ordered_cells
ERROR: missing control artifact paths: codex, corpus, extractor, guidance, protocol, runner, sandbox, snapshot
```

The manifest test passed after requiring and validating each external artifact binding, including active-source identity and stale-hash checks.

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_readiness_is_public_green_seed_red_reference_green
FAIL: readiness status was not_ready; seed hidden failure was rejected because the successful-run marker "OK" was absent from the intentional failing run
```

The readiness test passed after checking the seed failure by its expected nonzero status and diagnostic, while enforcing output markers on passing commands. It also replays and grades the captured reference bundle from a fresh tree.

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationCaseTests.test_case_rejects_generated_roots_that_hide_protected_paths
FAIL: EvaluationError not raised
```

The test passed after rejecting generated-root exclusions that overlap protected inputs, including `.git` and canonical tests.

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationCaseTests.test_behavioral_commands_require_literal_output_markers
FAIL: EvaluationError not raised
```

The test passed after requiring `expected_output` to be an array of nonblank strings.

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_frozen_manifest_binds_six_ready_cases_controls_and_ordered_cells
FAIL: EvaluationError not raised
```

That red assertion covered a symlink supplied as the binary artifact path. The test passed after rejecting symlink and relative overrides.

## Final verification

Commands:

```text
python3 -m py_compile scripts/edit_evaluation.py scripts/test_edit_evaluation.py
exit 0

python3 scripts/edit_evaluation.py --help
exit 0; listed all seven CLI commands

python3 -m unittest scripts.test_edit_evaluation
..............
----------------------------------------------------------------------
Ran 14 tests in 4.781s

OK
```

Source digests:

```text
scripts/edit_evaluation.py      4084b9ef57bc49c4c56a6725bcdd785acd5ffab5387d64d8c808028be6aa7c1d
scripts/test_edit_evaluation.py 5bce288d87bc9baa091cb0461573710872296d65f74c8f84cb6868eadda74b13
```

The module tests cover case validation, baseline/seed/reference readiness, expected-output markers, fresh reference replay, fake-agent multi-file edits, ignored additions and deletions, protected writes, invalid symlink evidence, timeouts and descendant cleanup, failed grading, replay mismatch, summary metrics, and frozen manifest controls/schedule.

## Source state

```text
Worktree: /home/murphy/.codex/worktrees/1e6f/code-kb
Branch: fix/cross-language-review
HEAD: 7d7073a8b73a91fb98bf98d002d2247c5fc815b0
```

The worktree was dirty and nothing was staged or committed. Untracked entries at report time were this worker's two scripts, sibling-owned `evals/edit-2026-10-27/cases/`, and lead-owned `docs/plans/2026-10-27-filtered-watcher-registration.md`. The separate `/home/murphy/source/code-kb` checkout was on `main`, clean, and two commits ahead of `origin/main`.

No model or provider calls were made. No full Rust workspace suite or scored-corpus run was performed by this worker.

## Fix round: ANSI-colored test output

Prepared 2026-10-27 15:06 UTC after the lead reported that date-fns baseline tests had passed but were marked as not executed. The retained stdout log was `target/edit-evaluation/lead/integration-v1/date-fns/readiness/evidence/baseline-public-00-business_day_regressions/stdout.log`. Its Vitest summary contains ANSI SGR sequences around `Test Files` and the pass counts; the test and marker checks now strip ANSI escape formatting in memory. Sandbox stdout and stderr files remain byte-for-byte raw.

The regression uses the actual captured Vitest summary lines (`2 passed`, `34 passed`, `1 skipped`) and verifies the raw log remains unchanged. It also verifies that a colored `0 passed` summary does not count as test execution.

Red/green evidence:

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationOutputTests.test_colored_vitest_summary_is_detected_but_empty_run_is_not
FAIL: test_execution_observed was False for the captured ANSI-colored Vitest header
```

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationOutputTests.test_colored_vitest_summary_is_detected_but_empty_run_is_not
.
----------------------------------------------------------------------
Ran 1 test in 0.001s

OK
```

Final verification for this fix round:

```text
python3 -m py_compile scripts/edit_evaluation.py scripts/test_edit_evaluation.py
exit 0

python3 -m unittest scripts.test_edit_evaluation
...............
----------------------------------------------------------------------
Ran 15 tests in 4.784s

OK
```

The original Task 6 verification above remains intact with its original runner digest `4084b9ef57bc49c4c56a6725bcdd785acd5ffab5387d64d8c808028be6aa7c1d`. After the ANSI fix, current digests are:

```text
scripts/edit_evaluation.py      1da59b7504876f4a17295b3041dfca7232c1000be414f603215e3ec0d8f93428
scripts/test_edit_evaluation.py ab54901e919612580f456b1ba3434e9af254001615b527decc06e4669fd6f8e1
```

The final run used `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, HEAD `7d7073a8b73a91fb98bf98d002d2247c5fc815b0`. The worktree remains dirty with the two owned scripts and sibling/lead changes: `crates/code-kb-core/tests/watcher_registration_test.rs`, `docs/plans/2026-10-27-filtered-watcher-registration.md`, and `evals/edit-2026-10-27/cases/`. No staging or commit was performed.

## Fix round: source-review acceptance gate

Prepared 2026-10-27 15:36 UTC after final review found that behavioral passes were reported as successful before source review. Raw replay grades now set `success: false`; a completed behavioral pass has `acceptance_status: "pending_source_review"`, incomplete runs use `"incomplete"`, and failed behavior remains `"failed"`. `grade-bundle` exits successfully when replay and behavioral checks pass, even though acceptance is still pending.

Campaign summaries expose `behavioral_pass_count`, `pending_review_count`, and `accepted_count` separately. `success_count` now counts accepted work only, requiring source review passed, acceptance status accepted, every supplied completion and validity indicator true, unchanged protected paths, and a behavioral pass. The per-record summary includes `acceptance_status`; a stale `success: true` cannot override pending or failed review.

Red/green evidence:

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_fake_agent_patch_replays_ignored_addition_and_deletion_before_grading
FAIL: expected grade.success to be false while source_review is pending; it was true
```

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_summary_counts_only_accepted_reviewed_completed_valid_behavioral_passes
FAIL: expected success_count 1; got 3 from pending/failed-review records with stale success flags
```

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_timeout_keeps_partial_patch_and_kills_descendant_processes
FAIL: expected acceptance_status incomplete; got failed
```

The three focused tests passed after the fix. The summary regression walks pending review, accepted review, failed review, incomplete execution, invalid patch, and behavioral failure. It verifies stale `success: true` is ignored and reviewed, complete, valid, protected, behavioral acceptance is counted.

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_summary_counts_only_accepted_reviewed_completed_valid_behavioral_passes scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_fake_agent_patch_replays_ignored_addition_and_deletion_before_grading scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_timeout_keeps_partial_patch_and_kills_descendant_processes
...
----------------------------------------------------------------------
Ran 3 tests in 3.709s

OK
```

Final owned-module verification for this fix round:

```text
python3 -m py_compile scripts/edit_evaluation.py scripts/test_edit_evaluation.py
exit 0

python3 -m unittest scripts.test_edit_evaluation
................
----------------------------------------------------------------------
Ran 16 tests in 4.839s

OK
```

Current source digests:

```text
scripts/edit_evaluation.py      d90c460c6538af067f64f38f0afbb0f2f5728c85e52aa57547b39adc1e63ec97
scripts/test_edit_evaluation.py 7038ce37baba6df13cd44b3ae57cb523b98c907a3749b211c3f737abc64a6920
```

The verification worktree was `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, HEAD `fe7ede045be57d6e5905025e2eaf658f5d1550ac`. It was dirty with the owned orchestration scripts and concurrent lead/sibling changes in the watcher and SQLGlot/MiniJinja case files. No staging or commit was performed. This report remains ignored by `.gitignore` under `.razorback/`.

## Fix round: declared reference-added public tests

Prepared 2026-10-27 15:44 UTC after SQLGlot and MiniJinja reference patches added public test files outside `production_files`. `reference_test_files` is an optional array of safe relative paths. Validation rejects duplicates, overlapping file/directory paths, and overlaps with production files, hidden destinations, generated roots, protected paths, or Git metadata. It does not protect or reserve the new test paths.

Only the two trusted reference-patch applications in readiness enable the extra-file allowance. Each declared path must be absent from the seed and added as a regular file; only newly created parent directories required by those files are allowed. Seed patches remain restricted to `production_files`, and extra reference files remain ordinary patch content for agent grading.

Red/green evidence:

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_reference_patch_may_add_declared_public_test_but_seed_patch_may_not
FAIL: readiness was not ready; the reference patch's new public test was rejected as a protected-path change
```

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationCaseTests.test_reference_test_files_are_validated_without_becoming_protected_or_production
.
----------------------------------------------------------------------
Ran 1 test in 0.002s

OK
```

```text
python3 -m unittest scripts.test_edit_evaluation.EditEvaluationRunnerTests.test_reference_patch_may_add_declared_public_test_but_seed_patch_may_not
.
----------------------------------------------------------------------
Ran 1 test in 0.480s

OK
```

The synthetic readiness test confirms the new test is absent from the seed, present in the reference tree and captured bundle, and rejected if the seed patch tries to add it. Validation also checks duplicate and overlapping declarations. Existing public command selections are unchanged; source review uses the reference-added tests as evidence of coverage.

Final owned-module verification for this fix round:

```text
python3 -m py_compile scripts/edit_evaluation.py scripts/test_edit_evaluation.py
exit 0

python3 -m unittest scripts.test_edit_evaluation
..................
----------------------------------------------------------------------
Ran 18 tests in 5.331s

OK
```

Current source digests:

```text
scripts/edit_evaluation.py      34388ab9463a2a9cf6784014d4ea2f0c4a1dcd2d9d4124d5c0d4be0faddc8d76
scripts/test_edit_evaluation.py f8ad241ca897b5b234655042c16732128906ba39f0ffeb0fdac60b68ac439b2e
```

The verification worktree was `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, HEAD `fe7ede045be57d6e5905025e2eaf658f5d1550ac`. It was dirty with the two owned scripts and concurrent watcher, protocol, SQLGlot, and MiniJinja changes. No staging or commit was performed. The report remains ignored under `.razorback/`.
