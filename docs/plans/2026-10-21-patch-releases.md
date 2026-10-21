# code-kb 2.8.1 and Julie 3.9.1 releases

## Authority and workspaces

The owner requested: "ok let's go through the release process for both". This authorizes the documented release sequence, including branch pushes, PRs, merges, tags, publication, and evidence closeout. No history rewriting or paid API runs are authorized. The required harness sessions use existing Claude Max and ChatGPT subscriptions.

- code-kb: /home/murphy/source/code-kb, review/v2.8.0-followups, source fixes f6a4833. PR52 integrates these before the version bump.
- Julie: /home/murphy/source/julie-extractors, review/v3.9.0-followups, source fixes f9a4fa1f.
- Reuse these checkouts. Check every worktree and require clean intentional source before each push/tag/publication. Return primary checkouts to current clean main.

## Ownership

- Julie prep worker owns product version manifests/lockfile, C# contract marker and related marker tests, 3.9.1 notes/contract ledger/readiness evidence, and its checkpoint. It commits locally; no remote actions.
- Lead owns code-kb pin/version files, code-kb release notes, all remote integration/publication, gate interpretation, package verification and final evidence.
- Release-gate explorer is read-only. Other agents may perform bounded gate execution in assigned output directories but never modify source without explicit ownership.
- Dogfood follow-up worker A owns Python runtime member resolution in queries/reference-report helpers, focused reference-report tests, and the paired agent-doc rule. Worker B owns edit-check handling/tests for required Python keyword-only arguments, with shared arity changes only if necessary. Lead owns their integration, edit-check documentation, and exact Flask repros.
- Worker C owns a bounded file-mode blast-radius test-use supplement in ops.rs and blast-radius tests. It must reuse supported evidence and merge into nonempty results without full reports per file symbol. Lead owns formatter wording and its existing assertions.

- The TypeScript dev-case worker owns the bounded shorthand detector and reference-report
  regressions in `reference_report.rs`, `reference_report/js_bindings.rs`, and
  `reference_report_test.rs`. The lead owns the paired agent-doc rule, review,
  release gates and integration.

## Sequence and acceptance

- [x] Prepare Julie 3.9.1, schema8/report3/epoch10, corrected C# output marker; run local release/contract gates and build optimized binary.
- [x] Put that exact binary in code-kb .tools, update local pin to3.9.1, commit and run exactly one paired Claude/Codex dogfood round. Verify every wrong answer; fix before tagging, document nonblocking gaps. No repeat discovery round.
- [x] Integrate Julie through green CI; tag/publish3.9.1 only after the downstream round has no open defect.
- [x] Verify all six public archives and embedded checksums. Restore public Linux binary and put exact archive checksums into code-kb's pin.
- [x] Integrate code-kb source and published pin through PRs and green main CI. Preserve the dogfood commit as an ancestor with merge commits.
- [x] Run the full reach gate on the verified fixed source while Julie publication runs; its report commit must remain an ancestor of the bump, as required by preflight step10. Any subsequent behavior change invalidates that run. Bump only allowed version files/notes to2.8.1 after source integration. Build, run previous2.8.0-vs-candidate corpus after that bump, inspect changed outputs and call-edge samples, measure live retained memory, run preflight.
- [x] Run CI on the exact bump commit through a release PR without merging it. Atomically push that commit to main with its tag after checks pass.
- [x] Verify six code-kb release archives, embedded binary versions/checksums, fresh-plugin download/start, and publish both code-kb crates in dependency order; smoke registry and binstall installs.
- [x] Record publication evidence and current-release pointers as the documented evidence-only closeout. Verify remote tags/releases, no stranded changes, clean main in both primary checkouts.

## Verification ledger

Keep command logs in /tmp and target release directories. Reuse same-source review gate evidence where valid; version/contract changes require affected checks. CI supplies native Windows/macOS results. A shut-down local Windows VM need not be started when the documented preflight accepts CI. No release is complete until all required workflow jobs and public artifact/install checks pass.

- code-kb source PR52 passed all six required CI checks on f6a4833 (run39609828353).
- Local pin candidate87f263d uses Julie 3.9.1, binary SHA-25649d25aa51d8072c946c2a275c190068447848c604cfef11969cd180c3dbb75cb. Its archive hashes are temporary3.9.0 values and must be replaced before this branch is pushed.
- Dogfood campaign: one discovery round, lead validation plus one Claude and one Codex participant. Round20261021T134404Z uses detached Flask d73fa1c worktrees. The initial Codex process received HTTP400 before inference because gpt-6.1-sol was unavailable to its ChatGPT account. Its log is preserved; only that participant was retried with locally advertised gpt-6-sol on the same candidate and Flask commit. No second discovery round or paid API fallback.
- Nine reach-gate repositories have been materialized; every cache worktree is clean and at its pinned commit.
- The single dogfood round confirmed two further defects. An annotation-only `App.response_class` wrongly excludes three uses of Flask's assigned runtime member; runtime resolution must distinguish annotations from assigned values without changing dataclass/TypedDict fields or real overrides. A required keyword-only Python parameter leaves two broken calls out of `check_edit` because their total counts fit the new range; count evidence cannot prove the keyword is supplied. Astra reviewed both boundaries. Required checks cover annotation-only inheritance, assigned overrides/properties, missing/valid/optional keyword arguments, and unknown `**kwargs`, plus reruns of the original Flask calls. No second discovery round.
- The same round found misleading test-report wording and a file-query omission: file-mode blast radius skips the supported variable-use fallback that symbol mode uses, even when no limit was reached. Correct wording to describe indexed evidence and tests that call or use a symbol. A narrow file supplement must retain supported edges, reject namesakes/candidates, deduplicate before limits, and remain bounded; otherwise record the documented coverage gap without adding a new resolver.
- File-mode investigation closed as a documented gap: Flask cli.py has 266 indexed symbols; applying the single-target report to each would turn a 0.05 s query into as many as 266 report runs. A correct bounded batched resolver is separate feature work. No approximation or source change was made. Evidence: /tmp/file-blast-radius-investigation.md.
- Julie parser certification passed on 1ab37ea7: 42 tests, log /tmp/julie-3.9.1-certification.log.
- GitHub connector merged PR52 as 6f15896, but its push event produced no main CI run/check suite despite the workflow's main-push trigger. Remaining merges must use the authenticated native gh path and verify the resulting main SHA has CI before the final bump. Do not reuse the PR-head check as evidence for a different main SHA.
- Correction: the delayed main CI run39612513526 subsequently appeared and passed all six checks for6f15896. Preflight queries CI by exact SHA, not branch. The final pin PR may be integrated by a normal fast-forward of its checked head, preserving both its checks and the dogfood ancestry; no force push or history rewrite.
- Julie tagv3.9.1 points to main992426e4 (the CI-tested1ab37ea7 has the identical tree). Manual release run39614728309 uses that same tag/SHA after no tag-triggered run appeared. Watch for duplicates before accepting final asset hashes.
- Initial full reach run passed270 symbols on all9 repositories, but source inspection of its dev disagreements found one actual false exclusion: TypeScript shorthand `tx` in an object literal also writes an interface member and must not be excluded solely by its same-named local value binding. A conservative fix is required before the code-kb tag; rerun the full reach gate after this behavior change. Two missing edges remain coverage gaps. No cases or holdout labels change.

- Julie 3.9.1 publication run39614728309 completed successfully. All six archives passed asset-digest, archive, architecture, embedded-version, and binary-checksum checks. Public hashes now replace the temporary values in code-kb; the restored public Linux binary reports3.9.1 with SHA-256777fe822a0eddbdfd9f357c018c7a1a881907bedc698f913736a30ffc0ce0d12.

- Julie closeout is pushed on clean main at `3766aa0a`; tag `v3.9.1` still peels to
  `992426e4`. The publication record explicitly covers the approved evidence-only
  follow-up. `scripts/check-release-state.sh` passed.

- The TypeScript fix now passes the exact multiline Zod constructor fixture.
  It checks indexed member owners, reads at most a 512 KiB file prefix and a
  4 KiB/64-line caller snippet, and retains only an unresolved property-write
  candidate. The 412-test reference suite, local-value reference assertion,
  strict workspace Clippy, formatting, doc sync and diff checks passed.
  CRLF, final-line EOF, nested calls, ordinary local reads, imports, concrete
  methods, unrelated owners and candidate-only test impact are covered.

- code-kb v2.8.1 was atomically pushed with main at `c0668601df8e70ce7d33a2e55054fa63dc75171a`; PR54 merged by ancestry. PR CI 39624752524 and main CI 39626508079 both passed all six checks. Release run 39626508187 passed all eight jobs and published six archives plus six sidecars. Independent downloads passed API digest, sidecar, content, architecture and version checks.
- The empty-cache plugin downloaded code-kb 2.8.1, found its bundled Julie 3.9.1, and completed a Cobra lookup. Both code-kb crates are published at 2.8.1, core before CLI. The publication record is `docs/release-evidence/2026-10-21-v2.8.1-release.md`; evidence-only closeout follows outside the unchanged tag under the owner's release authorization.
- Fresh public registry source install and metadata-only binstall both passed version and Cobra query checks. Both crate archives name clean release commit `c0668601`; binstall's binary hash matches the verified GitHub bundle. Durable provenance and commands are in `docs/release-evidence/v2.8.1-installs.json`.
- Final local preflight passed 1,572 workspace tests and 20 plugin tests. The final 15-root corpus lost no symbols or edges; all 114 changed outputs were audited as intended wording changes. The 270-symbol reach gate passed with unchanged holdout labels; retained limits remain in the notes and publication record.
