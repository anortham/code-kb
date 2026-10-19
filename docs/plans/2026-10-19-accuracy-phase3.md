# Accuracy Phase 3 Implementation Plan

> **For agentic workers:** Use razorback:subagent-driven-development for independent tasks, or tasks whose separate context has a clear benefit, when delegation is available and permitted. Otherwise use razorback:executing-plans: the current agent runs the plan.

**Goal:** Measure, without a paid model run, how many of the sites that agents really miss `check_edit` names, and how often the Stop hook blocks a correct diff. Then close the misses that the measure finds, until the pass marks below hold on the dev half and are scored once on the holdout half.

**Architecture:** Five slices. Slice 1 runs first, because its clean-commit measure decides whether the Stop hook that 2.7.0 turns on by default may keep its block rule.
1. Replay: a new offline harness, `scripts/edit_replay.py`, with three sets: clean commits (false blocks), held-back sites cut from real commits (misses), and missed-site events from session logs (misses that agents made).
2. Fixes: the block rule first when the clean set fails, then the causes of the misses in order of their count, tuned on dev only.
3. Corpus breadth: a second TypeScript and a second Java project in the reach gate, so TypeScript and Java fixes have dev data outside zod and gson.
4. Scan cost: the `full` index level costs about twice the scan memory of `facts` (hermes-agent 2.60 GB to 5.38 GB). Bring it down without a change to any callers answer.
5. Adoption: telemetry records the client and the outcome of each hook block, and a report after 14 days of organic use.

**Tech Stack:** Rust 2024 (rusqlite, serde_json), SQLite schema v7, julie-extract 3.8.1 (pinned at the start of phase 3; a newer version only if slice 4 needs it), git CLI, Python 3 standard library (harness and gate scripts).

**Spec:** No approved design covers phase 3. `docs/plans/2026-10-17-accuracy-base-design.md` defines phases 1 and 2 only. The active goldfish brief names phase 3 as "a missed-site replay over session logs" and phase 4 as "an agent A/B test (needs a budget approval)". The pass marks come from the research in `~/.code-kb/reports/2026-10-17-direction-research/designs.txt` ("Missed-site replay, offline" and "Gate 2, the completeness replay"). Every ruling in this plan is provisional until the owner approves the plan.

**Architecture Quality:**
- New script `scripts/edit_replay.py` with the subcommands `select`, `run`, and `score`, and its unit tests in `scripts/test_edit_replay.py`. It calls only the `code-kb` CLI (`code-kb --root <tree> check --base <rev> --json`) and git. It may import helpers from `scripts/reach_gate.py`.
- New frozen case files `evals/replay/clean.json` and `evals/replay/holdback.json` (repository, commit, parent, split, and for holdback the defining files and the truth lines). Session cases never enter the repository: they hold the owner's session content, so they stay under `~/.code-kb/reports/2026-10-19-replay/`.
- Fixes stay in the modules that own the rule: `crates/code-kb-core/src/edit_check.rs` and `edit_check/` for shapes and the block rule, `reference_report.rs` and `reference_report/` for site labels, julie-extractors for missing facts.
- Main risk: a replay that selects its cases with `check_edit` itself only measures what `check_edit` already sees. The holdback selector therefore reads git diffs only and never calls `code-kb`.

**Authority ledger:**
- `local_commit_authority: authorized — owner 2026-10-17: "You are approved to work in both repos"`. Applies after the owner approves this plan.
- `push_authority: missing` — ask once at the end of each slice that ships.
- `pr_authority: missing` (same ask).
- code-kb release (2.7.1 or 2.8.0): missing. A further change to the block rule (task 2.1) needs a release before users get it.
- julie-extractors release: missing for phase 3. The 2026-10-18 grant covered phase 2. Ask if slice 2 or 4 needs a julie change.
- Paid model runs: none in this plan. The phase 4 agent A/B test needs its own budget approval.
- Reviewer choice: none (Sonnet review agents inside the workflows, lead validation).

**Lead decisions (provisional, recorded):**
- The harness measures before any fix. No rule changes in slice 1.
- The clean-commit mark is a release question: if more than 2% of clean commits get a false block, task 2.1 changes what blocks before any other fix, and the lead asks the owner about a patch release.
- v2.7.0 (released 2026-10-19) already narrowed the block rule: the Stop hooks block only on a `proven` site, never on a `likely` site, a `signature changed` row, or a JavaScript parameter change. On 76 recent commits with a shape change in cobra, gson, flask, zod, express, Newtonsoft.Json, and code-kb, the old rule blocked 18 stops (none needed an edit) and the new one blocks 0. This branch starts before v2.7.0: rebase it onto `main` before task 1.1.
- The owner asked on 2026-10-19 to pin julie-extract 3.8.1 (a patch: `symbols` and `facts` scans skip call-argument work; extraction output equal to 3.8.0). The repin is the first commit of this branch: full suite 1500 of 1500, reach gate tables equal to the 2.7.0 report, `lost_proven.py` empty in all 7 repos.
- Holdback truth comes from what the commit's author changed, not from a compiler. A compiler run per case costs too much across 7 languages, and the author's edit is the site the agent must reach.
- Session cases that cannot be rebuilt are counted by cause and left out. They are not guessed.
- Blast radius test selection is out of scope. The research measured that static selection misses 29% to 52% of failing test files, and the tool already says "run the full test suite".

## Global Constraints

- Core Invariant 1: every MCP tool requires `project_root`. No schema names `workspace`, `workspace_id`, `repo_path`, or `root_dir`.
- CLI 1:1: every MCP tool has a CLI command that prints the same text.
- Paths in text and JSON use `/` on every platform.
- No repository data held in RAM across calls.
- `AGENTS.md` and `CLAUDE.md` stay byte-identical. Both routing-block copies (`hooks/code-kb-routing-block.md`, `crates/code-kb-cli/src/routing-block.md`) stay byte-identical, and under 3 KB.
- No new crate dependency. The harness uses the Python 3 standard library only.
- Reach gate marks hold after every task that changes Rust code: proven precision >= 0.98, excluded precision >= 0.98, and accounted share >= 0.98 in every language. `~/.cache/code-kb-accuracy/lost_proven.py` prints nothing for the gate projects against the previous task's build.
- Tune only on dev cases: dev symbols in the reach gate, dev commits and dev sessions in the replay. Never read the site details of a holdout case. Never run `dump_sites.py`. The split is fixed at selection time: a commit is dev when `int(sha[:8], 16) % 2 == 0`, a session when the same rule holds for the SHA-256 of its log path.
- Session logs are private. No session text, path, or diff enters the repository, a commit message, a PR, or a release note. The repository gets the harness and aggregate counts only.
- Each test binary gets `.tools/julie-extract` 3.8.1 next to it, so no binary falls back to an older `julie-extract` and rewrites the indexes.
- Heavy output goes under `~/.cache/code-kb-accuracy/phase3/`, never `/tmp`. `TMPDIR=/home/murphy/.cache/julie-tmp`. Full-history clones for the replay go under `~/.cache/code-kb-accuracy/phase3/repos/`; the reach gate copies of flask, express, and zod are shallow and stay untouched.
- Pass marks (provisional, from the research):
  - Clean set: at most 2% of commits with a shape change get a block (a `proven` or `likely` row outside the diff) that a hand check finds false. At least 50 such commits over at least 5 languages.
  - Holdback set: the check lists at least 80% of the truth sites as `proven` or `likely` in each language with at least 10 truth sites, and at least 90% of the listed rows outside the truth are true on a hand check of up to 50 rows.
  - Session set: the check names the file:line that the compiler or test later reported in at least 8 of the 9 verified compile cases (or the same share of the ones that can be rebuilt), and in at least 70% of the candidates that can be rebuilt.

---

## Verification Strategy

**Project source of truth:** `CLAUDE.md`, `.github/workflows/ci.yml`, `docs/RELEASING.md` (code-kb); `CLAUDE.md`, `docs/release.md`, `.github/workflows/ci.yml` (julie-extractors).

**Worker red/green scope:** `python3 -m unittest discover -s scripts -p test_edit_replay.py`, `python3 -m unittest discover -s scripts -p test_reach_gate.py`, `cargo test -p code-kb-core --test <file>`, `cargo test -p code-kb-core --lib <module>`, `cargo test -p code-kb-cli --test <file>`; julie `cargo test -p julie-extractors <filter>`.

**Worker ceiling:** the test binaries of the crate a task changes, plus `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo fmt --all -- --check`. Workers run `edit_replay.py run` and `score` on dev cases only. Workers never run the reach gate, the holdout score, or release scripts.

**Lead affected-change scope:** after each task that changes Rust code: `CI=1 cargo nextest run --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `python3 scripts/reach_gate.py run --cache-dir /home/murphy/.cache/code-kb-reach-new` PASS, `lost_proven.py` empty, and `edit_replay.py score` on dev not lower than before the task.

**Branch gate (end of each slice):** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `CI=1 cargo nextest run --workspace`, `node --test tests/plugin/*.test.cjs`, `python3 -m unittest discover -s scripts -p 'test_*.py'`, `cmp AGENTS.md CLAUDE.md`, the reach gate PASS, `lost_proven.py` empty. julie (only if changed): `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features --no-deps -- -D warnings`, `scripts/check-agent-doc-sync.sh`, `node scripts/language-data-quality-report.mjs --strict`, `cargo xtask test default`, `cargo xtask test contract`, `cargo xtask compat-check`.

**Security scope:** julie: `cargo deny check --all-features` (CI). code-kb: none declared. Slice 1 adds one manual check: `git grep` for the session report folder name and for `/.claude/projects/` and `/.codex/sessions/` returns nothing in the repository.

## Parallel Execution Contract

Commit mode: `serial-worker-commit` for every task.

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| 1.1 Harness and clean set | P1 | `scripts/edit_replay.py` (new), `scripts/test_edit_replay.py` (new), `evals/replay/clean.json` (new), `evals/replay/README.md` (new) | Yes | 1.2 and 1.3 extend the same script. |
| 1.2 Holdback set | P1 | same as 1.1, plus `evals/replay/holdback.json` (new) | Yes | After 1.1. |
| 1.3 Session set | P1 | same as 1.1 (session cases outside the repository) | Yes | After 1.2. |
| 1.4 Baseline report | P1 | `docs/replay-baseline.md` (new) | Yes | Needs 1.1 to 1.3. |
| 3.1 Second TypeScript and Java projects | P1 | `evals/reach/corpus.json`, `evals/reach/cases/<new>.json`, `evals/reach/README.md` | No | None - safe parallel batch. No file shared with slice 1, 4, or 5. |
| 4.1 Scan cost | P1 | `crates/code-kb-core/src/sync.rs`, `scripts/release-corpus-check.py`, `docs/RELEASING.md`; julie files if needed | No | None - safe parallel batch. |
| 5.1 Adoption telemetry | P1 | `crates/code-kb-core/src/telemetry.rs`, `crates/code-kb-cli/src/mcp/server.rs` (client name only), `crates/code-kb-cli/src/hooks.rs`, `crates/code-kb-core/src/edit_check/session.rs`, tests | No | None - safe parallel batch. No file shared with 1.x, 3.1, or 4.1. |
| 2.1 Block rule | S2 | `crates/code-kb-core/src/edit_check.rs`, `crates/code-kb-cli/src/hooks.rs`, `docs/edit-check.md`, tests | Yes | Needs 1.4. Shares `hooks.rs` with 5.1, so it runs after 5.1. |
| 2.2 Close the misses | S2 | `crates/code-kb-core/src/edit_check.rs`, `edit_check/`, `reference_report.rs`, `reference_report/`, `docs/edit-check.md`, `docs/reach-sections.md`, tests; julie files if needed | Yes | After 2.1. Each cause is one commit. |
| 3.2 TypeScript unresolved noise | S3 | `reference_report.rs`, `reference_report/`, tests | Yes | After 2.2 and 3.1: shares `reference_report/` with 2.2 and needs the new dev data. |
| 2.3 Holdout score and docs | S4 | `docs/replay-baseline.md`, `docs/edit-check.md`, `docs/release-notes/v2.8.0.md`, `CLAUDE.md`, `AGENTS.md` | Yes | Last: it scores the final build once. |
| 5.2 Adoption report | S5 | `~/.code-kb/reports/<date>-adoption/` (outside the repository) | Yes | 14 days after the release that ships 5.1. |

---

## Slice 1: Replay

Each task in this slice measures. No task changes a rule.

### Task 1.1: Harness and clean set

**Files:** `scripts/edit_replay.py`, `scripts/test_edit_replay.py`, `evals/replay/clean.json`, `evals/replay/README.md`.
**Interfaces:** `edit_replay.py select clean --repos evals/reach/corpus.json --per-repo N`, `edit_replay.py run clean --code-kb <bin> --out <dir>`, `edit_replay.py score clean --run <dir> [--split dev|holdout]`.
**Contract inputs:** `code-kb --root <tree> check --base <rev> --json` (the `EditCheck` JSON: `changes[].proven`, `changes[].likely`, `changes[].in_diff`, `changes[].unresolved`, `cuts`); `evals/reach/corpus.json` (name, language, url, commit, local).
**File ownership:** the four files above.
**Serialization required:** Yes.
**Dependency reason:** 1.2 and 1.3 extend the same script.

**What to build:** A harness that walks back from the pinned commit of each reach corpus project, takes non-merge commits, runs `code-kb check --base <parent>` on a detached worktree at each commit, and keeps the commits where the check reports at least one shape change. Every `proven` or `likely` row outside the diff is a block. A Sonnet reviewer hand-checks each block against the code at that commit and the commits after it: true when the site still uses the old shape (a later commit fixed it, or the commit does not build), false otherwise. The verdict and its evidence go into `clean.json`.

**Approach:** Clone full history into `~/.cache/code-kb-accuracy/phase3/repos/<name>` from `url` (the local clones of flask, express, and zod are shallow). Build one index per clone, then let each detached worktree copy and reconcile it (Invariant 1, automatic index creation). Stop a repository at N commits with a shape change, or after 400 commits.

- [x] `select clean` writes at least 50 commits with a shape change over at least 5 languages, each with `repo`, `commit`, `parent`, `language`, and `split`.
- [x] `run clean` records the check JSON, the wall time, and the exit code of each commit; a check that errors counts as an error, never as clean.
- [x] `score clean` prints, per language and in total: commits, commits with a block, commits with a false block, and the false-block share; and p50 and p95 of the check time.
- [x] Unit tests cover the split rule, the block rule (only `proven` or `likely` outside the diff), and scoring on a fixture run with no network.
- [x] `python3 -m unittest discover -s scripts -p test_edit_replay.py` passes (the plan's `python3 -m unittest scripts/...` form cannot import the sibling modules). Worker commits (`serial-worker-commit`).

### Task 1.2: Holdback set

**Files:** `scripts/edit_replay.py`, `scripts/test_edit_replay.py`, `evals/replay/holdback.json`, `evals/replay/README.md`.
**Interfaces:** `edit_replay.py select holdback`, `run holdback`, `score holdback`.
**Contract inputs:** same as 1.1.
**File ownership:** the four files above.
**Serialization required:** Yes.
**Dependency reason:** After 1.1 (same script).

**What to build:** A set of commits that change a definition in one file and its uses in other files. For each, the harness builds a partial tree: the parent, plus the commit's change to the defining file only (`git checkout <commit> -- <defining file>`). It runs `code-kb check --base <parent>` on that tree. The truth is the lines in the other files that the commit changed and whose old text names the symbol as a whole word, at their line numbers in the parent.

**Approach:** The selector reads git diffs only and never calls `code-kb` (Architecture Quality). A removed line in the defining file holds a definition when it matches the definition keyword of its language (`def`, `fn`, `func`, `class`, `struct`, `enum`, `interface`, `type`, a Java or C# method head); the name on that line is the symbol. A commit qualifies when hunks in at least one other code file remove a line that names it. Keep at most 3 commits per symbol name.

- [ ] `holdback.json` holds at least 20 cases per language for at least 5 languages, each with the defining file, the symbol, the truth lines, and the split.
- [ ] `score holdback` prints per language: truth sites, sites listed as `proven` or `likely`, sites listed in neither, the `unresolved` counts, cases with no shape change found (the check missed the shape), and listed rows outside the truth.
- [ ] Each miss gets a cause from a fixed list: `shape not seen`, `site not indexed`, `site labeled unresolved`, `site excluded`, `cut by a limit`, `other`. The cause comes from the check JSON and `code-kb refs --json` on the parent tree, never from a holdout case.
- [ ] A Sonnet reviewer hand-checks up to 50 listed rows outside the truth on dev cases, and the verdicts go into the case file.
- [ ] Unit tests cover the selector on a fixture git repository and the truth-line rule. Worker commits.

### Task 1.3: Session set

**Files:** `scripts/edit_replay.py`, `scripts/test_edit_replay.py`. Case data only in `~/.code-kb/reports/2026-10-19-replay/`.
**Interfaces:** `edit_replay.py select sessions --examples <examples.json> --errclass <errclass.txt> --out <dir>`, `run sessions`, `score sessions`.
**Contract inputs:** `~/.code-kb/reports/2026-10-17-direction-research/sessions/examples.json` (keys `missed_A1` 6, `missed_A2` 16, `missed_B` 135, `brokeelsewhere_compile` 19, `brokeelsewhere_test` 44, `brokeelsewhere_other` 16; fields `file`, `fail_line`, `fix_edit_line`, `fix_file`, `fail_kind`, `check`) and `errclass.txt` (the E0061, E0063, E0004, and E0425 compile cases; the 9 verified ones are the subject of the research's gate 2).
**File ownership:** the two script files.
**Serialization required:** Yes.
**Dependency reason:** After 1.2 (same script).

**What to build:** For each case, rebuild the tree just before the failing check: the commit that was `HEAD` when the session started, plus the session's edits in order up to `fail_line` (Claude `Edit`, `MultiEdit`, and `Write`; Codex `apply_patch`). Run `code-kb check --base <that commit>`. The case is caught when the check lists, as `proven` or `likely`, the file:line that the failing check reported (or, without a line, the `fix_file` that the next edit changed).

**Approach:** The session's first git output or its `cwd` and start time give the start commit. A case is not rebuilt when its repository or commit is gone, or an edit does not apply. The script does not guess.

- [ ] `score sessions` prints, per group: cases, cases rebuilt, cases not rebuilt by cause, cases caught, and cases missed with the cause list of 1.2.
- [ ] No session content is written inside the repository; the security check in the Verification Strategy passes.
- [ ] Unit tests cover the edit replay of one Claude and one Codex fixture log (made-up content). Worker commits.

### Task 1.4: Baseline report

**Files:** `docs/replay-baseline.md`.
**Interfaces:** none.
**Contract inputs:** the `score` output of 1.1 to 1.3 on dev and holdout, run once with the 2.7.0 release binary.
**File ownership:** `docs/replay-baseline.md`.
**Serialization required:** Yes.
**Dependency reason:** Needs 1.1 to 1.3.

**What to build:** The baseline: each pass mark against the 2.7.0 binary, the miss causes ranked by count on dev, and the false blocks by rule. This is the one time the holdout halves are scored before task 2.3.

- [ ] The report states each pass mark as PASS or FAIL with its numbers, in aggregate counts only.
- [ ] The lead tells the owner the clean-set result before slice 2 starts. A FAIL there is a question about a patch release.
- [ ] Worker commits.

## Slice 2: Fixes

### Task 2.1: Block rule

Runs only when the clean set fails its mark. v2.7.0 already made the first change (see Lead decisions), so the slice 1 baseline measures the v2.7.0 rule.

**Files:** `crates/code-kb-core/src/edit_check.rs`, `crates/code-kb-cli/src/hooks.rs`, `docs/edit-check.md`, `crates/code-kb-core/tests/edit_check_test.rs`, `crates/code-kb-cli/tests/edit_hook_test.rs`.
**Interfaces:** the Stop and SubagentStop decision; the `check_edit` text keeps every row.
**Contract inputs:** the false blocks of 1.1 on dev, grouped by rule (change kind, label, basis).
**File ownership:** the files above.
**Serialization required:** Yes.
**Dependency reason:** Needs 1.4; after 5.1 (`hooks.rs`).

**What to build:** The smallest change to what blocks a stop that brings the dev false-block share to 2% or less, for example `likely` rows with one basis no longer block. `check_edit` still lists the rows; only the hook decision changes.

- [ ] Dev false-block share <= 2%; the holdback and session catch rates on dev drop by at most the rows the change stops blocking, and the report states the drop.
- [ ] A test for each rule that no longer blocks. `docs/edit-check.md` "What blocks a stop" matches the code. Worker commits.

### Task 2.2: Close the misses

**Files:** `crates/code-kb-core/src/edit_check.rs`, `crates/code-kb-core/src/edit_check/*.rs`, `crates/code-kb-core/src/reference_report.rs`, `crates/code-kb-core/src/reference_report/*.rs`, `docs/edit-check.md`, `docs/reach-sections.md`, tests; julie-extractors files if a cause needs a new fact.
**Interfaces:** `check_edit` rows and `find_references` labels.
**Contract inputs:** the ranked miss causes of 1.4 on dev. The known gaps in `docs/edit-check.md` "Known limits" are the first candidates: TypeScript `interface` and `type` fields, a new trait or interface method and its implementors, calls through an alias of a removed symbol, Java `import` lines, a Rust tuple struct, and enum variant payloads.
**File ownership:** the files above.
**Serialization required:** Yes.
**Dependency reason:** After 2.1. One commit per cause.

**What to build:** Fix the causes in order of their dev miss count until every pass mark holds on dev, or until each remaining cause needs a julie change that the owner has not approved. A cause that is a gap, not a wrong answer, can stay as a known limit; the report says which.

- [ ] Each fix has a regression test named for the behavior, a reach gate PASS, an empty `lost_proven.py`, and a dev score not lower in any set.
- [ ] `docs/edit-check.md` "Known limits" drops each closed gap and names each one left.
- [ ] Every pass mark holds on dev, or the report names the julie change each remaining cause needs. Worker commits.

### Task 2.3: Holdout score and docs

**Files:** `docs/replay-baseline.md`, `docs/edit-check.md`, `docs/release-notes/v2.8.0.md` (`v2.7.1.md` when task 2.1 ships alone first), `CLAUDE.md`, `AGENTS.md`.
**Interfaces:** none.
**Contract inputs:** the final branch build.
**File ownership:** the files above.
**Serialization required:** Yes.
**Dependency reason:** Last task; it scores the holdout halves once.

- [ ] The holdout halves of all three sets are scored once with the final build, and the report adds them next to the baseline.
- [ ] The release notes draft and `CLAUDE.md`/`AGENTS.md` describe every changed rule; `cmp AGENTS.md CLAUDE.md` passes.
- [ ] Branch gate green. The lead asks the owner about push, PR, and the release. Worker commits.

## Slice 3: Corpus breadth

### Task 3.1: Second TypeScript and Java projects

**Files:** `evals/reach/corpus.json`, `evals/reach/cases/<ts project>.json`, `evals/reach/cases/<java project>.json`, `evals/reach/README.md`.
**Interfaces:** `reach_truth.py sample` and `truth`, `reach_gate.py materialize` and `run`.
**Contract inputs:** the case file format in `evals/reach/README.md`; SCIP indexes from `scip-typescript` and `scip-java`.
**File ownership:** the files above.
**Serialization required:** No.
**Dependency reason:** None - safe parallel batch.

**What to build:** One TypeScript project and one Java project, each with a pinned full commit, 30 sampled symbols, SCIP truth, and the dev and holdout split of the README. The executor picks the projects and records why. A project qualifies when its SCIP indexer runs on it, it has at least 30 symbols with at least 3 truth sites outside their own file, and it is an application or a tool, not a library of the same kind as zod or gson. Local candidates: `/home/murphy/source/goldfish` (TypeScript).

- [ ] The gate runs 9 projects and still passes with the 2.7.0 binary, or the report lists each mark that fails on the new projects. A new project that fails is a finding for 3.2 and 2.2, not a reason to drop the project.
- [ ] `python3 -m unittest discover -s scripts -p test_reach_gate.py` passes. Worker commits.

### Task 3.2: TypeScript unresolved noise

**Files:** `crates/code-kb-core/src/reference_report.rs`, `crates/code-kb-core/src/reference_report/*.rs`, tests.
**Interfaces:** `find_references` labels.
**Contract inputs:** the 2.7.0 gate: in TypeScript, 200 of 1218 `unresolved` sites cover a truth line; in Java 29 of 526.
**File ownership:** the files above.
**Serialization required:** Yes.
**Dependency reason:** After 2.2 and 3.1.

**What to build:** Rules from the dev symbols of zod and the new TypeScript project that move `unresolved` sites with no truth line to `excluded` with a basis, and true sites to `proven`.

- [ ] On dev, TypeScript `unresolved` sites with no truth line fall by at least half, with excluded precision >= 0.98 and no lost proven site.
- [ ] Each rule has a test and a line in `CLAUDE.md`/`AGENTS.md` "Proof rules". Worker commits.

## Slice 4: Scan cost

### Task 4.1: Full-level scan cost

**Files:** `crates/code-kb-core/src/sync.rs`, `scripts/release-corpus-check.py`, `docs/RELEASING.md`; julie-extractors files if the cost is in the extractor.
**Interfaces:** the index level that code-kb asks julie for; `SCAN_SLOWDOWN_LIMIT`.
**Contract inputs:** the 2.7.0 notes: aspnetcore index 2.0 GB to 3.6 GB, scan 40.7 s to 57.6 s, peak memory 2.81 GB to 4.75 GB; hermes-agent 1.8 GB to 4.0 GB, 55.6 s to 86.8 s, 2.60 GB to 5.38 GB.
**File ownership:** the files above.
**Serialization required:** No.
**Dependency reason:** None - safe parallel batch.

**What to build:** Find what the `full` level writes that no code-kb query reads, and stop writing it, or cut the scan's peak memory. The callers report reads the call and read identifiers; the other `full` rows are the candidates.

- [ ] On aspnetcore and hermes-agent, index size and peak scan memory are at most 1.5 times the `facts` level of 2.6.0. The report gives both numbers before and after.
- [ ] The reach gate report is equal to the 2.7.0 report, and `lost_proven.py` is empty.
- [ ] `SCAN_SLOWDOWN_LIMIT` is 1.5 again, its shortcut comment is gone, and `docs/RELEASING.md` says 1.5. Worker commits.

## Slice 5: Adoption

### Task 5.1: Adoption telemetry

**Files:** `crates/code-kb-core/src/telemetry.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/src/hooks.rs`, `crates/code-kb-core/src/edit_check/session.rs`, tests.
**Interfaces:** new nullable column `client` in `tool_telemetry` (from `clientInfo.name` of the MCP `initialize` request); a new table `hook_telemetry` (timestamp, workspace root, event, diff hash, sites listed, blocked, and `followed`); the session file gains the sites of the last block.
**Contract inputs:** the column migration pattern in `init_telemetry_db` (`ALTER TABLE … ADD COLUMN` after `PRAGMA table_info`).
**File ownership:** the files above.
**Serialization required:** No.
**Dependency reason:** None - safe parallel batch.

**What to build:** Telemetry for the two adoption measures of the research: how often an agent acts on a block, and which client calls the tools. At the next Stop after a block, the hook records `followed` = true when the new diff changed a line of a site it listed. Telemetry stays local, follows the existing disable flags, and records no source text.

- [ ] An old telemetry database upgrades in place; a test covers it.
- [ ] Hook tests cover `followed` true and false, and a disabled telemetry flag that records nothing.
- [ ] Hook time stays within 0.01 s of 2.7.0 on a repeat Stop (0.07 to 0.08 s on zod). Worker commits.

### Task 5.2: Adoption report

**Files:** `~/.code-kb/reports/<date>-adoption/` only.
**Interfaces:** none.
**Contract inputs:** `~/.code-kb/telemetry.db` and the session logs of 14 days after the release that ships 5.1.
**File ownership:** the report folder.
**Serialization required:** Yes.
**Dependency reason:** Needs 14 days of organic use after that release.

**What to build:** The research's adoption measures: the share of Claude and Codex edit sessions where code-kb output reaches the context before the last turn (target 50%; 5.5% before an edit in the research), the share of blocks that the agent followed (target 50%), the median hook text in tokens (target 300 or fewer), and the p95 time of `check_edit`.

- [ ] The report states each measure against its target, by client.

---

## Out of scope

- Phase 4, the agent A/B test (arms: no code-kb, 2.6.0, and the phase 3 build with hooks; first-pass completeness, fix rounds, turns, tokens). It needs a budget approval.
- Blast radius changes (Lead decisions).
- Go and Java receiver typing, and C# `base.F()`, unless the replay ranks them as a miss cause in 2.2.

## Estimate

In agent terms: slice 1 is one to two sessions; slice 2 is one to three sessions, set by the miss causes; slices 3 and 4 are one session each; task 5.1 is half a session. Human time: plan approval, the release questions after 1.4 and 2.3, and the 14-day wait before 5.2.
