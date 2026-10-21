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

## Sequence and acceptance

- [ ] Prepare Julie3.9.1, schema8/report3/epoch10, corrected C# output marker; run local release/contract gates and build optimized binary.
- [ ] Put that exact binary in code-kb .tools, update local pin to3.9.1, commit and run exactly one paired Claude/Codex dogfood round. Verify every wrong answer; fix before tagging, document nonblocking gaps. No repeat discovery round.
- [ ] Integrate Julie through green CI; tag/publish3.9.1 only after the downstream round has no open defect.
- [ ] Verify all six public archives and embedded checksums. Restore public Linux binary and put exact archive checksums into code-kb's pin.
- [ ] Integrate code-kb source and published pin through PRs and green main CI. Preserve the dogfood commit as an ancestor with merge commits.
- [ ] Bump only allowed version files/notes to2.8.1 after source integration. Build, run reach gate and previous2.8.0-vs-candidate corpus, inspect changed outputs and call-edge samples, measure live retained memory, run preflight.
- [ ] Run CI on the exact bump commit through a release PR without merging it. Atomically push that commit to main with its tag after checks pass.
- [ ] Verify six code-kb release archives, embedded binary versions/checksums, fresh-plugin download/start, and publish both code-kb crates in dependency order; smoke registry and binstall installs.
- [ ] Record publication evidence and current-release pointers as the documented evidence-only closeout. Verify remote tags/releases, no stranded changes, clean main in both primary checkouts.

## Verification ledger

Keep command logs in /tmp and target release directories. Reuse same-source review gate evidence where valid; version/contract changes require affected checks. CI supplies native Windows/macOS results. A shut-down local Windows VM need not be started when the documented preflight accepts CI. No release is complete until all required workflow jobs and public artifact/install checks pass.

- code-kb source PR52 passed all six required CI checks on f6a4833 (run39609828353).
- Local pin candidate87f263d uses Julie3.9.1, binary SHA-25649d25aa51d8072c946c2a275c190068447848c604cfef11969cd180c3dbb75cb. Its archive hashes are temporary3.9.0 values and must be replaced before this branch is pushed.
- Dogfood campaign: one discovery round, lead validation plus one Claude and one Codex participant. Round20261021T134404Z uses detached Flask d73fa1c worktrees. The initial Codex process received HTTP400 before inference because gpt-6.1-sol was unavailable to its ChatGPT account. Its log is preserved; only that participant was retried with locally advertised gpt-6-sol on the same candidate and Flask commit. No second discovery round or paid API fallback.
- Nine reach-gate repositories have been materialized; every cache worktree is clean and at its pinned commit.
