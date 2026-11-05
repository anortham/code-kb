# Toolkit release execution

User authority: approved all release preparation, integration, verification, pushes, tags and publication on 2026-11-04. This supersedes the earlier no-publication handoff. No force push, history rewrite, unrelated worktree removal or global registration cleanup is included.

Versions: Goldfish 8.0.10, Razorback 0.49.0, code-kb 2.13.0 with published Julie 3.12.0. Goldfish and Razorback release workers own their repositories. Lead owns code-kb and the final combined check.

## code-kb sequence

- [x] Inventory all worktrees, fetch origin, reuse e912 checkout on release/2.13.0.
- [x] Merge toolkit-efficiency into Julie integration; rejected candidate remains excluded.
- [ ] Run exactly one full dogfood round on the combined published-Julie candidate, before version bump; verify each finding. Fix wrong answers with regression and exact-call evidence, never repeat a full round just to find more.
- [x] Run fresh ground-truth reach gate for semantic inputs: a07c195 PASS, overall recall 0.915.
- [ ] Complete source gates and merge verified integration into main through required PR/CI.
- [ ] Bump only version-bearing files/release notes locally; source/dependency version normalization permits reusing matching reach/dogfood fingerprints.
- [ ] Compare release candidate against released v2.12.0 on real-project corpus on bump commit; inspect changed outputs and edge samples.
- [ ] Run complete release-preflight and required CI on bump commit.
- [x] Run isolated three-tool integration using Julie 3.12 candidate: 2026-11-05T00:57Z PASS on Goldfish 0f74184 and code-kb a07c195; recheck if runtime changes.
- [ ] Atomically push code-kb main and version tag according to docs/RELEASING.md; watch six-platform assets, verify checksum/install smoke, then publish core/CLI crates and verify registry install paths.

## Evidence and safety

Use code-kb first for structure; test first before any production fix. Every source change must be committed and clean before gating/publication. Reconcile main, toolkit-efficiency, Julie integration and retained dogfood worktrees before each push. Preserve unrelated worktrees and Flask user files. Only disposable harness-created fixtures may be cleaned by the documented runner.

Existing native Windows evidence covers the unchanged Julie runtime source at 4953bf6; rerun affected Windows tests if source fixes change that code. The lead owns the shared Windows VM.

External provider policy: no external-model policy block declared in code-kb AGENTS; authorized harness dogfood sends public Flask source to Anthropic/OpenAI. Redact constructed prompts immediately before dispatch with razorback security-review helper. Pin the exact release binary in both harness registrations.

Local source scope: existing format/lint/tests/package gates plus available gitleaks changed-history scan and Cargo dependency audit. Critical/high findings cannot ship; record lower-severity findings and false-positive dispositions. Do not hide a failing gate.

Do not rerun the completed 36-trial performance study or claim savings. Its rejected experiment and limits belong in release notes; efficiency success is not an existing release gate.

Verification evidence lives under ignored target/release-support, target/reach-gate, target/release-dogfood, and target/release-corpus; record summary/checkpoint artifacts in source control before release commits.

## Published releases

Goldfish 8.0.10 is published at `0f74184` with local tests/typecheck/tag guard and both main/tag CI passing. Razorback 0.49.0 has passed 505 tests and its version checks; the combined check passed and its worker is finalizing publication. code-kb source PR: https://github.com/anortham/code-kb/pull/86.
