# Task 3 report: Python and Rust cases

## Cases and source eligibility

- Created the sqlglot and MiniJinja cases under `evals/edit-2026-10-27/cases/` with pinned full-history source clones outside trial snapshots, upstream licenses retained, fresh-trial patches, per-file protected test lists, hidden checks, exact argv commands, and generated roots.
- sqlglot pin `feb0e08aba28a29590596bb3f5761fbaeb0e98b9`: 183 production Python files / 85,099 lines. The reference changes `sqlglot/parsers/bigquery.py` and `sqlglot/generators/bigquery.py`.
- MiniJinja pin `81787c4eab656f3a4f4e92dea53f303a36f1685c`: 80 production Rust files / 38,491 lines. The reference changes `minijinja-cli/src/config.rs` and `minijinja-cli/src/command.rs`.
- Both cases are `feature` tasks. MiniJinja extends suffix detection and help while its seed deliberately removes legacy `.j2` autoescape behavior to produce a genuine hidden regression.
- Final case JSON SHA-256: sqlglot `bb1aa29213587b8c01984d6f70ba3cbc931f8fd1330c6f9fcf2a4a08b75cc13e`; MiniJinja `7f58fd59a90b9345bd78b2d96aef02f4d6636f2305366c8b60ff2c838866cf8e`. Final case-tree hashes and dependency/tool host maps are in `target/edit-evaluation/task3_python_rust/runtime-bindings.json`.

## Readiness commands and markers

- sqlglot public: `/usr/bin/env PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=/state/python-site:/workspace/project /usr/bin/python3 -m unittest tests.dialects.test_bigquery`; required output markers `Ran 64 tests`, `OK`.
- sqlglot hidden: same argv with `tests.dialects.test_edit_eval_timestamp_bucket`; required success markers `Ran 2 tests`, `OK`. Seed is expected to exit 1 with `bucket width belongs in DateBin.this`.
- sqlglot’s integrated readiness record is retained at `target/edit-evaluation/task3_python_rust/official-readiness/sqlglot/readiness.json` (created `2026-10-27T14:36:43Z`, status `ready`): baseline and reference public each passed 64 tests, the seed failed for the intended diagnostic, and the reference hidden check passed 2 tests. This record predates the final additional `OK` output marker and evidence-text update; the retained logs contain both markers. The lead’s final all-six run will bind the final case hash.
- MiniJinja public: `/usr/bin/env CARGO_HOME=/state/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/state/target CARGO_INCREMENTAL=0 RUSTC=/tools/rust/bin/rustc /tools/rust/bin/cargo test --locked --offline -p minijinja-cli --bin minijinja-cli --test test_basic -- --skip test_help`; required markers `test result: ok. 4 passed`, `test result: ok. 30 passed`, `1 filtered out`.
- MiniJinja hidden: same environment with `cargo test --locked --offline -p minijinja-cli --test edit_eval_autoescape`; required markers `test automatic_mode_uses_the_output_suffix_before_a_template_wrapper ... ok`, `test explicit_modes_override_template_wrapper_detection ... ok`, `test long_help_documents_template_wrapper_suffixes ... ok`, `test result: ok. 3 passed`. Seed is expected to exit 101 with `legacy .j2 autoescape should remain enabled`.
- Direct final MiniJinja Task2 sandbox runs passed baseline public (4 unit + 30 CLI tests), reference public (4 + 30), and reference hidden (3). Seed hidden exited 101 with the expected `.j2` diagnostic. Logs are under `target/edit-evaluation/task3_python_rust/logs/`; the final semantic help-assertion run is `minijinja-v5-reference-hidden-autoescape_suffix_acceptance/`.

## Decisions and retained failures

- The protected `minijinja-cli/tests/snapshots/test_basic__long_help.snap` expects the old help sentence; the full baseline command passed 4 unit and 31 CLI tests, while the same full reference command failed only `test_help`. The selected public command keeps both existing public test targets and skips only that obsolete snapshot assertion. The hidden help test checks the three wrapper names and case-insensitivity language inside the `--autoescape` section, accepting equivalent wording.
- The original full-suite baseline/reference logs are retained as `target/edit-evaluation/task3_python_rust/logs/minijinja-v2-*`. No snapshot or upstream test was modified.
- An earlier integrated MiniJinja readiness record at `target/edit-evaluation/task3_python_rust/official-readiness/minijinja/readiness.json` is retained as `not_ready`: that runner revision mistakenly ran the newly installed hidden test in an extra `seed_public` gate. The lead removed that extra gate; direct readiness checks above use the final declared commands.
- The four protected MiniJinja license/readme symlinks stay in full snapshot protection. Per the reviewed sandbox handoff, symlink binds were omitted; their regular root targets remain read-only.
- Public dependency prep ran only in sandbox `prepare`: pytz 2026.5 and `cargo fetch --locked`. All grade commands used network-disabled `grade` mode. sqlglot import evidence resolves to `/workspace/project/sqlglot/__init__.py`.

## Verification and handoff

- `edit_evaluation.py validate-case` passed for both cases; `rustfmt --check` passed for MiniJinja’s hidden Rust test.
- Source clone statuses are clean and pinned. Cargo/rustc 1.97.1 satisfy the MiniJinja Rust 1.88 minimum.
- No host upstream setup/build/test commands, model calls, staging, or commits were used.
- Active worktree at handoff: `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, HEAD `50a1bcdafc199c7d7f3c42c21e4d94a681ad902a`. The Task 3 case directories and shared evaluation runner files remain untracked in this worktree. The lead owns the frozen all-six integrated run and final commit.

## Reference completeness follow-up

- Source review found both requests require regression tests to be added, but the initial reference patches only changed production files. Added one public regression file to each reference patch without editing the requests, hidden fixtures, canonical tests, or selected canonical commands.
- SQLGlot: `checks/test_timestamp_bucket_public.py` is installed by the reference patch at `tests/dialects/test_timestamp_bucket_public.py`. The network-disabled Task2 sandbox run passed 2 tests with `Ran 2 tests` and `OK`. Exact argv: `/usr/bin/env PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=/state/python-site:/workspace/project /usr/bin/python3 -m unittest tests.dialects.test_timestamp_bucket_public`. Logs: `target/edit-evaluation/task3_python_rust/reference-public-followup/logs/sqlglot/public-test/`.
- MiniJinja: `checks/edit_eval_autoescape_public.rs` is installed by the reference patch at `minijinja-cli/tests/edit_eval_autoescape_public.rs`. The network-disabled Task2 sandbox run passed 2 tests. Exact argv: `/usr/bin/env CARGO_HOME=/state/cargo CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/state/target CARGO_INCREMENTAL=0 RUSTC=/tools/rust/bin/rustc /tools/rust/bin/cargo test --locked --offline -p minijinja-cli --test edit_eval_autoescape_public`. Logs: `target/edit-evaluation/task3_python_rust/reference-public-followup/logs/minijinja/public-test/`.
- Full reference patches passed `patch --dry-run --batch --forward -p1` on fresh copies of the seeded source. `validate_case` passed for both cases; neither new reference destination overlaps hidden or protected paths. Test hashes, final patch hashes, final case-tree hashes, elapsed times, and log paths are recorded in `target/edit-evaluation/task3_python_rust/reference-public-followup/reference-public-test-results.json`.
- Final hashes: SQLGlot reference patch `559cf70bb80fa458e9ea7f5580a8775c69aff1c8988951b2a1248ea5c806fdb8`, case tree `57871ef9231b3d83db1b93220a57c000f9405c788369318b65a3f48098ce400d`; MiniJinja reference patch `58558ff2dccc44a6d0d5092814c0a9f661309d5bfd7d6ad1d9d791afdd8d60d1`, case tree `73103cde9693f7d790544ab704ce4271f2bc4fd7cb6a0ae94a992041c5c36c8e`.
- These were focused public-test runs on copies of the already-repaired reference snapshots. The lead's final integration run will apply the updated reference patches through the final runner.
