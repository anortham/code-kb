# Accuracy Base Implementation Plan

> **For agentic workers:** Use razorback:subagent-driven-development for independent tasks, or tasks whose separate context has a clear benefit, when delegation is available and permitted. Otherwise use razorback:executing-plans: the current agent runs the plan.

**Goal:** `find_references` callers accounts for every indexed row with the target's name, labels each row by its evidence, reconciles with a whole-word text count, and an offline ground-truth gate measures it in 7 languages.

**Architecture:** The existing SQL resolver stays the source of resolved rows. A new accounting step in `code-kb-core` classifies those rows and every other row with the name (pending calls, identifiers of all kinds, relationships to other definitions) into proven, likely, unresolved, or excluded, and a new `text_scan` module counts whole-word lines. The index level moves from `facts` to `full` so call and variable-reference identifiers exist. A Python gate scores the labeled answers against frozen SCIP-based truth.

**Tech Stack:** Rust 2024 (rusqlite, ignore, memchr), SQLite schema v7 from julie-extract 3.7.2, Python 3 standard library (gate scripts), SCIP indexers (offline truth only).

**Spec:** `docs/plans/2026-10-17-accuracy-base-design.md`

**Architecture Quality:** New module `crates/code-kb-core/src/text_scan.rs` (one public function, no state). New report types in `models.rs`. New module `crates/code-kb-core/src/reference_report.rs` holds the accounting and the report entry point; it calls `queries.rs` helpers made `pub(crate)`. The old `find_references*` functions keep their signatures and results for `blast_radius` and `get_symbol_context`. Main risk: accounting latency on names with many definitions; bounded by a 12-definition cap and a 1.5 s budget.

**Authority ledger:**
- `local_commit_authority: authorized — owner: "Ok let's do it. You are approved to work in both repos" (2026-10-17)`
- `push_authority: missing`
- `pr_authority: missing`
- Release (tag, publish, crates.io): missing.
- Plan approval: the owner authorized this work in chat (2026-10-17); this plan adds no consequential choice beyond the approved design, so execution proceeds without a second approval round.
- Reviewer choice: none.

## Global Constraints

- Every MCP tool keeps exactly its current schema properties; `project_root` stays required (Core Invariant 1). No new MCP parameter.
- CLI 1:1: `code-kb refs` prints the same text as the MCP `find_references` handler.
- Paths in text and JSON use `/` on every platform (Core Invariant 4).
- No repository data held in RAM across calls; the text scan streams files (Core Invariant 2).
- `AGENTS.md` and `CLAUDE.md` stay byte-identical.
- No new crate dependency except `memchr` (already in `Cargo.lock` through `ignore`).
- Label strings, exactly: `proven`, `likely`, `unresolved`, `excluded`.
- Basis strings, exactly: `same file`, `qualified path`, `receiver type`, `class chain`, `import`, `enclosing scope`, `same package`, `builds the class`, `name only`, `nearest of {k} definitions`, `receiver not typed`, `{k} definitions`, `resolves to `{Owner.name}` ({path}:{line})`, `receiver `{r}` is imported from `{source}``, `receiver `{r}` has type `{T}``, `other language`.
- Callers header line 1, exactly: ``References to {kind} `{display}` ({path}:{line})``.
- Callers header line 2, exactly: `Sites: {n} · proven {p} · likely {l} · unresolved {u} · excluded {x}`.
- Callers header line 3, exactly: ``Text: `{name}` is on {t} lines: {s} sites, {d} definitions, {i} imports, {o} text only.`` (singular forms `1 line`, `1 site`, `1 definition`, `1 import` when the number is 1).
- A text scan cut by its budget adds, exactly: `Text scan stopped after {secs} s: {scanned} of {total} files read; counts cover those files.`
- Constructor targets replace line 3 with, exactly: ``Text: skipped for a constructor; its call sites spell the class name. Use find_references on `{Class}`.``
- Text-scan budget 3 s; file size cap 2 MB; accounting cap 12 other definitions and 1.5 s.
- The MCP `find_references` description becomes, exactly: `Callers of a symbol: every indexed site with its name, each labeled proven, likely, unresolved, or excluded by its evidence, plus a whole-word text count that accounts for every line of the name. Use instead of rg or grep on a name. Callees: the calls a symbol makes. Pass file_path or a qualified name for a common name.`
- Gate pass marks: proven precision >= 0.98 in every language; accounted share >= 0.98 in every language. Recall target (report): proven+likely >= 0.90 overall.
- julie-extractors: no change to schema v7 or the CLI contract in this plan.

---

## Verification Strategy

**Project source of truth:** `CLAUDE.md`, `scripts/run_tests.sh`, `.github/workflows/ci.yml` (code-kb); `CLAUDE.md`, `docs/testing-strategy.md`, `.github/workflows/ci.yml` (julie-extractors).

**Worker red/green scope:** code-kb: `cargo test -p code-kb-core --test <file>` or `cargo test -p code-kb-core --lib <module>`; CLI: `cargo test -p code-kb-cli --test <file>`. Gate scripts: `python3 -m unittest discover -s scripts -p 'test_reach_*.py'`. julie: `cargo test -p julie-extractors <filter>` and `cargo xtask test language rust`.

**Worker ceiling:** the test binaries of the crate a task changes, plus `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo fmt --all -- --check`. Workers never run the gate on all repos or the release scripts.

**Worker gate invariant:** each named test proves one acceptance criterion; a worker reports the exact test names and pass counts.

**Lead affected-change scope:** after each merged batch: `CI=1 cargo nextest run --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` in the integration worktree.

**Branch gate:** code-kb: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `CI=1 cargo nextest run --workspace`, `node --test tests/plugin/*.test.cjs`, `cmp AGENTS.md CLAUDE.md`, `python3 scripts/reach_gate.py run` PASS. julie: `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features --no-deps -- -D warnings`, `scripts/check-agent-doc-sync.sh`, `node scripts/language-data-quality-report.mjs --strict`, `cargo xtask test default`, `cargo xtask test contract`, `cargo xtask compat-check --previous-binary /home/murphy/source/code-kb/.tools/julie-extract`.

**Security scope:** julie: `cargo deny check --all-features` (dependency audit, CI). code-kb: none declared.

**Replay/metric evidence:** Hard gates: proven precision and accounted share per language (gate). Report-only: proven+likely recall, refs p50/p95 latency on aspnetcore and hermes-agent (baseline v2.6.0 vs candidate), live `serve` RSS before and after the level change, index size and scan time.

**Escalation triggers:** any change to `pending_target_predicate` or `find_direct_references` → run `references_test`, `resolution_test`, `blast_radius_test`, `disambiguation_test`, `qt_cpp_test`, `qt_qml_test`. Any change to sync or level → `freshness_test`, `worktree_test`, `mcp_test`.

**Assigned verification failure:** Workers stop and report when assigned verification fails, unless this plan explicitly says to update that gate.

**Verification ledger:** Record invariant, command, scope label, commit SHA, result, and timestamp (plus hard-gate and report-only metrics). Reuse a passing entry for the same HEAD and scope instead of rerunning.

## Parallel Execution Contract

Workers run in their own worktrees, branched from `feat/accuracy-base`, and commit on their task branch. The lead reviews each branch and merges it into `feat/accuracy-base` (`serial-worker-commit` on the task branch).

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| Task 1: Gate harness | Batch A | Create `scripts/reach_gate.py`, `scripts/reach_truth.py`, `scripts/scip_decode.py`, `scripts/test_reach_gate.py`, `evals/reach/README.md`, `evals/reach/corpus.json`; modify `scripts/release-preflight.sh` | No | None - safe parallel batch. |
| Task 2: Text scan module | Batch A | Create `crates/code-kb-core/src/text_scan.rs`; modify `crates/code-kb-core/src/lib.rs` (module line and re-export only), `crates/code-kb-core/Cargo.toml`, `Cargo.toml` (workspace dependency line only), `Cargo.lock` | No | None - safe parallel batch. |
| Task 3: Rust path-qualified calls | Batch A | Modify `crates/code-kb-core/src/queries.rs` (`pending_target_predicate` only); create `crates/code-kb-core/tests/rust_paths_test.rs` | No | None - safe parallel batch. |
| Task 4: Full index level | Batch A | Modify `crates/code-kb-core/src/sync.rs`; tests in `crates/code-kb-core/tests/freshness_test.rs` and `sync.rs` unit tests | No | None - safe parallel batch. |
| Task 5: julie Rust type facts | Batch A | julie-extractors worktree: the Rust type-fact producer files, their tests and fixtures, `docs/contracts/extraction-output-changes.md`, the three crate `Cargo.toml` version lines, `Cargo.lock` | No | None - safe parallel batch (separate repository). |
| Task 6: Labeled reference accounting | Batch B | Create `crates/code-kb-core/src/reference_report.rs`, `crates/code-kb-core/tests/reference_report_test.rs`; modify `crates/code-kb-core/src/models.rs`, `crates/code-kb-core/src/queries.rs` (`pub(crate)` visibility and extracted shared helpers only; no behavior change), `crates/code-kb-core/src/lib.rs` (module line and re-exports) | Yes | Needs Task 2 (`scan_word`), Task 3 (predicate), Task 4 (full level in fixtures). |
| Task 7: Ground truth cases | Batch B | Create `evals/reach/cases/<repo>.json` for flask, express, zod, gson, cobra, newtonsoft-json, code-kb; one agent per repo owns one file | No (within batch) | Needs Task 1 scripts. |
| Task 8: Report output in MCP and CLI | Batch C | Modify `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/src/main.rs`, tests in `crates/code-kb-cli/tests/cli_test.rs`, `crates/code-kb-cli/tests/mcp_test.rs`, `crates/code-kb-cli/tests/adversarial_m3_test.rs`, `crates/code-kb-core/src/formatters.rs` unit tests | Yes | Needs Task 6 types and `reference_report`. |
| Task 9: Docs and gate run | Batch D | Modify `CLAUDE.md`, `AGENTS.md`, `README.md`, `skills/code-kb/SKILL.md`; create `docs/reviews/2026-10-17-accuracy-base-gate.md` | Yes | Needs Tasks 6-8 merged and the gate report. |

### Task 1: Gate harness

**Files:**
- Create: `scripts/reach_gate.py`, `scripts/reach_truth.py`, `scripts/scip_decode.py`, `scripts/test_reach_gate.py`, `evals/reach/README.md`, `evals/reach/corpus.json`
- Modify: `scripts/release-preflight.sh`

**Interfaces:**
- Consumes: `code-kb --root <dir> scan`; `code-kb --root <dir> refs <name> -f <file> --limit 200 --json`. The candidate prints a report object (Task 8); v2.6.0 prints an array of `ReferenceSite` (`from_symbol_name`, `from_symbol_id`, `to_symbol_name`, `kind`, `path`, `start_line`, `start_column`, optional `occurrences`).
- Produces: case file format `evals/reach/cases/<repo>.json`:
  `{"repo": str, "commit": str, "language": str, "symbols": [{"id": str, "split": "dev"|"holdout", "name": str, "file": str, "line": int, "kind": str, "tags": [str], "truth_source": "scip"|"manual"|"scip+manual", "truth": [{"path": str, "line": int}], "adjudicated": [{"path": str, "line": int, "verdict": "true"|"false", "note": str}]}]}`.
  `corpus.json`: `[{"name": str, "language": str, "url": str, "commit": str, "local": str|null}]` with the 7 repos at the commits in Contract inputs.
  Commands: `reach_gate.py materialize`, `reach_gate.py run [--binary PATH] [--repos a,b] [--out DIR]`, `reach_gate.py disagreements --repo R [--split dev]`, `reach_truth.py sample --repo R --index INDEX.scip --count 30 --seed 20261017`, `reach_truth.py truth --repo R --index INDEX.scip`.
  Report: `<out>/report.json` and `<out>/report.md` ending with `## Result: PASS` or `## Result: FAIL`; default out `target/reach-gate/`.

**Contract inputs:** Repos and commits: flask `https://github.com/pallets/flask` d73fa1cdcbd8b1465c151db8924ba58b1dd14e35 (local `/home/murphy/source/flask`); express `https://github.com/expressjs/express` bed501c695a61886399ee622875f3be933c716d8 (local `/home/murphy/source/express`); zod `https://github.com/colinhacks/zod` e516c3b (local `/home/murphy/source/zod`; resolve the full SHA); gson `https://github.com/google/gson` 8b4b55051489132190cb8d1c61eb9dc7f5381295; cobra `https://github.com/spf13/cobra` adbc8813901bba65827259daa8e22ff94ec1f30e; newtonsoft-json `https://github.com/JamesNK/Newtonsoft.Json` 09bb545d72969ad7fb4ea07db0d5c34f4fc07877; code-kb `https://github.com/anortham/code-kb` 765d94079677cd21fc180f9b3e1e266b02c83a31 (local `/home/murphy/source/code-kb`). Materialize into `~/.cache/code-kb-reach/<name>-<first 12 chars of commit>` (clone from `local` when present, else `url`, then `git checkout --detach <commit>`). SCIP decoder prior art: `/tmp/claude-1000/-home-murphy-source-code-kb/d9541551-34eb-4370-8df7-d54f8f047719/scratchpad/gate-tooling/scip_refs.py` (copy to `scripts/scip_decode.py`, keep it dependency-free). Scoring and pass marks: the design doc section "Ground-truth gate".

**File ownership:** Create `scripts/reach_gate.py`, `scripts/reach_truth.py`, `scripts/scip_decode.py`, `scripts/test_reach_gate.py`, `evals/reach/README.md`, `evals/reach/corpus.json`; modify `scripts/release-preflight.sh`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**What to build:** Offline scripts that pin the corpus, turn SCIP indexes into per-symbol truth, sample symbols deterministically, run a code-kb binary over the cases, and score each label tier per language and split. The release preflight requires a fresh PASS report.

**Approach:** Standard library only. Scoring: a truth line `(P, L)` matches a site when `site.path == P` and `site.start_line <= L <= (site.end_line or site.start_line)`. Adjudicated `false` lines leave the truth set; adjudicated `true` lines join it. Metrics per language and split: proven precision = proven sites covering a truth line / proven sites; recall = truth lines covered by proven or likely sites / truth lines; accounted = truth lines covered by any site or listed in `text_only` / truth lines. A legacy array (v2.6.0) scores every row as proven with no text lines. `disagreements` prints, per symbol of the chosen split, proven sites with no truth line and truth lines with no site, each with the source line text. Holdout output shows aggregates only unless `--show-holdout`. `sample` stratifies by tags computed from SCIP and the repo: `common-name` (3+ definitions with the name), `overridden`, `many-refs` (20+), `value-ref` (a reference not followed by `(`), `decorator` (Python `@name`), and picks 30 with the seed, alternating dev and holdout. Truth strips import and `use` lines and unions overloads (same owner and name). Preflight: add a step after the corpus-check step that requires `target/reach-gate/report.md` with `## Result: PASS` and a `Commit:` line naming an ancestor of HEAD, following the existing corpus-check step's shell style.

**Acceptance criteria:**
- [ ] `reach_gate.py materialize` creates all 7 repo copies at their pinned commits; a second run reuses them.
- [ ] Unit tests cover span matching, adjudication override, legacy array scoring, label-tier metrics, the accounted share with text-only lines, the PASS/FAIL decision, and holdout hiding.
- [ ] `reach_truth.py` produces truth for `Config.from_mapping` from a flask SCIP index equal to the 8 lines in Contract inputs prior art (config.py:302, test_config.py:112,116,120,124,129, examples/celery/src/task_app/__init__.py:9, examples/tutorial/flaskr/__init__.py:9).
- [ ] `release-preflight.sh` fails without a PASS report and passes with one (shell test or manual run recorded in the task report).
- [ ] `python3 -m unittest discover -s scripts -p 'test_reach_*.py'` passes; commit on the task branch.

### Task 2: Text scan module

**Files:**
- Create: `crates/code-kb-core/src/text_scan.rs`
- Modify: `crates/code-kb-core/src/lib.rs`, `crates/code-kb-core/Cargo.toml`, `Cargo.toml`, `Cargo.lock`

**Interfaces:**
- Consumes: `workspace::is_hard_excluded(rel_path: &str) -> bool`; crate `ignore` (`WalkBuilder`), crate `memchr` (`memmem::Finder`).
- Produces:
  ```rust
  pub struct WordHits {
      pub hits: Vec<(String, usize)>, // (relative path with '/', 1-based line), sorted, one entry per line
      pub files_scanned: usize,
      pub files_total: usize,         // files the walk found, including unread ones when cut
      pub complete: bool,             // false when the budget cut the scan
      pub elapsed: std::time::Duration,
  }
  pub fn scan_word(root: &std::path::Path, word: &str, budget: std::time::Duration) -> WordHits;
  ```

**Contract inputs:** Word characters are ASCII letters, digits, `_`, and `$`; a hit needs a non-word byte (or start/end of line) on both sides. Case-sensitive. Skip: paths where `is_hard_excluded` is true, `.gitignore` and `.julieignore` rules (`add_custom_ignore_filename(".julieignore")`), any folder below the root that contains `.git` (nested repository or worktree), files over 2 MB, files with a NUL byte in the first 8 KiB. Hidden files are not skipped by the walker itself (`.github/` is scanned); `is_hard_excluded` handles `.git`, `.code-kb`, and the other excluded names. Parallel walk (`build_parallel`), at most 8 threads.

**File ownership:** Create `crates/code-kb-core/src/text_scan.rs`; modify `crates/code-kb-core/src/lib.rs` (module line and re-export only), `crates/code-kb-core/Cargo.toml`, `Cargo.toml` (workspace dependency line only), `Cargo.lock`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**What to build:** A streaming whole-word scan of a project that returns the lines holding a name. It reads each file once with a reused buffer and keeps only the hit list.

**Approach:** `memmem::Finder` over the file bytes, then a boundary check and a line count by `memchr` of `\n` between hits. Stop handing out files once the budget is spent; keep counting found files so `files_total` is right. Sort the hits at the end.

**Acceptance criteria:**
- [ ] Tests: whole-word boundaries (`get` matches `get(`, `.get`, not `getter`, `_get`, `$get`), CRLF files, a hit on the last line with no trailing newline, two hits on one line give one entry, `.gitignore` and `.julieignore` rules, an excluded folder (`node_modules`), a nested repo folder, a binary file, a file over 2 MB, and a budget of 0 giving `complete == false` with `files_scanned < files_total`.
- [ ] Paths use `/` (test with a nested folder).
- [ ] `cargo test -p code-kb-core --lib text_scan` and clippy pass; commit on the task branch.

### Task 3: Rust path-qualified calls

**Files:**
- Modify: `crates/code-kb-core/src/queries.rs` (`pending_target_predicate` and new private helpers next to it)
- Test: `crates/code-kb-core/tests/rust_paths_test.rs`

**Interfaces:**
- Consumes: `pending_target_predicate(conn, target, parent) -> String` (queries.rs:3487); the third branch (top-level target with a namespace) at queries.rs:3790-3797 and the namespace exclusion list at queries.rs:3733.
- Produces: the same function signature; new matches only.

**Contract inputs:** The rule in the design doc section "Resolver fix: Rust path-qualified calls". Only `rust` callers get the `crate`/`self`/`super`/crate-name rules. Package folder = the path prefix before the last `/src/`, `/tests/`, `/benches/`, or `/examples/` segment (or the repo root for a top-level `src/`). Same crate = same package folder and both paths under `<package>/src/`, or the same file.

**File ownership:** Modify `crates/code-kb-core/src/queries.rs` (`pending_target_predicate` only); create `crates/code-kb-core/tests/rust_paths_test.rs`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**What to build:** Pending Rust calls written `crate::f()`, `self::f()`, `super::f()`, `my_crate::f()`, or `crate::module::f()` resolve to the top-level function they name instead of being dropped. Calls that name another crate still do not match.

**Approach:** Extend the third branch. Compare with `replace(value, '-', '_')` on both sides. Keep the existing `std`/`core`/`alloc` exclusions. Use the `scanned_repo` helper pattern from `references_test.rs` (a real julie-extract scan of fixture files) for a two-crate fixture: `crates/a-core/src/lib.rs` defines `safe_dir`, `crates/a-core/src/util.rs` calls `crate::safe_dir()`, `crates/a-core/src/m/n.rs` calls `super::super::safe_dir()`, `crates/a-cli/src/main.rs` calls `a_core::safe_dir()`, and `crates/b/src/lib.rs` defines its own `safe_dir` and calls `crate::safe_dir()`.

**Acceptance criteria:**
- [ ] Callers of `a-core`'s `safe_dir` include the `crate::`, `super::`, and `a_core::` calls and exclude `b`'s `crate::safe_dir()`.
- [ ] Callers of `b`'s `safe_dir` include only `b`'s call.
- [ ] A `std::fs::read` style call still does not match a project function named `read`.
- [ ] On the code-kb repo at 765d940, `code-kb refs safe_tempdir --limit 200` from this branch's binary reports at least 200 callers (record the number; rg finds 209 call sites).
- [ ] `references_test`, `resolution_test`, `blast_radius_test`, `disambiguation_test`, `rust_paths_test` pass; clippy passes; commit on the task branch.

### Task 4: Full index level

**Files:**
- Modify: `crates/code-kb-core/src/sync.rs` (`EXTRACTION_LEVEL` at sync.rs:35 and any test that names the level)
- Test: `crates/code-kb-core/tests/freshness_test.rs`, sync.rs unit tests

**Interfaces:**
- Consumes: `scan_workspace` (sync.rs:272-324) passes `--level` for a new artifact; `ensure_index_matches_extractor` (sync.rs:330-382) compares `index_level`.
- Produces: new artifacts recorded as `index_level = full`; an existing `facts` index rebuilds once through the existing guard.

**Contract inputs:** julie-extract `scan --level full` (CLI help: "`full` (everything; the default)"). The rebuild guard behavior in Core Invariant 7 stays as is.

**File ownership:** Modify `crates/code-kb-core/src/sync.rs`; tests in `crates/code-kb-core/tests/freshness_test.rs` and `sync.rs` unit tests

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**What to build:** code-kb builds new indexes at the `full` level, so call and variable-reference identifiers, annotations, source regions, and complexity metrics are present.

**Approach:** Change the constant; find every test or doc string in the owned files that names `facts` as code-kb's level and update it. Add a test that a `facts` index is rebuilt once to `full` and then left alone.

**Acceptance criteria:**
- [ ] A fresh scan records `index_level = full` and has `identifiers` rows of kind `call` and `variable_ref`.
- [ ] A `facts` index is rebuilt once to `full`; the next guard check does not rebuild.
- [ ] `freshness_test`, `worktree_test`, `references_test`, and the `code-kb-core` lib tests pass; clippy passes; commit on the task branch.

### Task 5: julie Rust type facts

**Files:**
- Modify: the Rust type-fact producer in `/home/murphy/source/julie-extractors/.worktrees/accuracy-base/crates/julie-extractors/src/rust/` (find it from the `type_facts` writers), its tests and golden fixtures, `docs/contracts/extraction-output-changes.md`, version lines in `crates/julie-extractors/Cargo.toml`, `crates/julie-extract-cli/Cargo.toml`, `crates/julie-extract-artifact/Cargo.toml`, and `Cargo.lock`
- Test: `crates/julie-extractors/src/tests/rust/` (new or existing type-fact test module)

**Interfaces:**
- Consumes: `type_facts` rows (`symbol_id`, `resolved_type`, ...), schema v7 unchanged.
- Produces: no truncated `resolved_type` values in Rust output; julie-extract version `3.7.3`; ledger section `## 3.7.3` with a `classification:` line.

**Contract inputs:** The 2026-10-17 research found 60 malformed Rust type facts in the code-kb index, for example `&[Box<dyn`; the 2026-09-08 quality gate requires zero (`docs/findings/2026-09-08-receiver-type-facts-wave-2-evidence.md`). Release steps stay out of scope (no tag, no publish).

**File ownership:** julie-extractors worktree: the Rust type-fact producer files, their tests and fixtures, `docs/contracts/extraction-output-changes.md`, the three crate `Cargo.toml` version lines, `Cargo.lock`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch (separate repository).

**What to build:** Find why Rust type facts are cut at a delimiter (`&[Box<dyn`), fix the producer so each fact is a whole type or no fact, and record the output change in the ledger with a patch version.

**Approach:** Reproduce with `julie-extract scan --level full` on `/home/murphy/source/code-kb` into a scratch db under `~/.cache/code-kb-accuracy/`, list malformed facts with unbalanced `<`, `[`, or `(`, write a failing test from the smallest case, then fix. Follow `docs/testing-strategy.md` for golden updates.

**Acceptance criteria:**
- [ ] A test reproduces at least two of the malformed shapes and passes after the fix.
- [ ] A scratch scan of `/home/murphy/source/code-kb` has 0 Rust type facts with unbalanced brackets (record the query and count before and after).
- [ ] Version `3.7.3` in the three manifests and `Cargo.lock`; ledger `## 3.7.3` section with classification.
- [ ] `cargo xtask test language rust`, `cargo xtask test default`, `cargo xtask test contract`, `cargo fmt --check`, clippy (CI flags), `scripts/check-agent-doc-sync.sh`, and `node scripts/language-data-quality-report.mjs --strict` pass; commit on `feat/accuracy-base` in the julie worktree.

### Task 6: Labeled reference accounting

**Files:**
- Create: `crates/code-kb-core/src/reference_report.rs`
- Modify: `crates/code-kb-core/src/models.rs`, `crates/code-kb-core/src/queries.rs`, `crates/code-kb-core/src/lib.rs`
- Test: `crates/code-kb-core/tests/reference_report_test.rs`

**Interfaces:**
- Consumes: `text_scan::scan_word` and `WordHits` (Task 2); `find_direct_references` and `constructed_class` (queries.rs:3964-3987); `pending_target_predicate`; `member_receiver_match`; `identifier_language_matches`; `get_symbol_overloads` (queries.rs:2804); `import_sites` (queries.rs:4647); selection helpers used by `find_references_scoped` and `find_references_for_symbol_ext`.
- Produces (models.rs, all `Serialize`/`Deserialize`/`Debug`/`Clone`/`PartialEq`):
  ```rust
  #[serde(rename_all = "snake_case")]
  pub enum SiteLabel { Proven, Likely, Unresolved, Excluded }
  pub struct LabeledSite {
      #[serde(flatten)] pub site: ReferenceSite,
      pub label: SiteLabel,
      pub basis: String,
      #[serde(default, skip_serializing_if = "Option::is_none")] pub end_line: Option<usize>,
  }
  pub struct TextOnlyLines { pub path: String, pub lines: Vec<usize> }
  pub struct TextScanSummary { pub files_scanned: usize, pub files_total: usize, pub complete: bool, pub elapsed_ms: u64, pub skipped: Option<String> }
  pub struct ReferenceCounts { pub sites: usize, pub proven: usize, pub likely: usize, pub unresolved: usize, pub excluded: usize, pub text_lines: usize, pub site_lines: usize, pub definition_lines: usize, pub import_lines: usize, pub text_only_lines: usize }
  pub struct ReferenceTarget { pub symbol_id: String, pub name: String, pub display_name: String, pub kind: String, pub path: String, pub line: usize, pub definitions_with_name: usize }
  pub struct ReferenceReport { pub target: ReferenceTarget, pub counts: ReferenceCounts, pub sites: Vec<LabeledSite>, pub text_only: Vec<TextOnlyLines>, pub text_scan: TextScanSummary, pub cannot_see: Vec<String>, pub imports: Vec<(String, usize)> }
  ```
  ```rust
  // reference_report.rs
  /// Selects the callers target by name exactly as `find_references_scoped` does today:
  /// `get_symbol_by_name(conn, name, path_filter)`, and the same `QueryError::SymbolNotFound`
  /// (built with `symbol_not_found_parts`) or ambiguity error, so error text is unchanged.
  pub fn select_reference_target(conn: &Connection, name: &str, path_filter: Option<&str>) -> Result<Symbol, QueryError>;
  /// The full report for a selected target. `root` is the workspace root for the text scan.
  pub fn reference_report(conn: &Connection, root: &Path, target: &Symbol, include_external: bool) -> Result<ReferenceReport, QueryError>;
  ```
  Handlers (Task 8) select with `select_reference_target` for a name and with `resolve_symbol_op` for an id, as today, then call `reference_report`.
  `sites` is complete (no limit) and sorted by label order (proven, likely, unresolved, excluded), then path, line, column.

**Contract inputs:** The design doc sections "Labels" and "Algorithm (callers of a selected target)", and the Global Constraints basis strings and caps. Exact-line pairing: for each (caller symbol, name), pair pending and relationship rows with `call` identifiers of the same name and containing symbol, in source order, each row with the first unpaired identifier on or after its start line; a paired site takes the identifier's line and column and consumes the identifier. Definition lines: a hit between a symbol's `start_line` and its body start (or its `start_line` when it has no body) for any symbol with the name. Import lines: import rows (`kind = 'import'`) with the name. `cannot_see` text by the target's language: Python `calls through getattr() or names built at runtime`; JavaScript/TypeScript `calls through computed property names or values built at runtime`; Rust `calls inside macro bodies`; Java/C#/Kotlin `calls through reflection or dependency injection`; every language `calls through dynamic dispatch reach overrides that this list does not show`.

**File ownership:** Create `crates/code-kb-core/src/reference_report.rs`, `crates/code-kb-core/tests/reference_report_test.rs`; modify `crates/code-kb-core/src/models.rs`, `crates/code-kb-core/src/queries.rs` (`pub(crate)` visibility and extracted shared helpers only; no behavior change), `crates/code-kb-core/src/lib.rs` (module line and re-exports)

**Serialization required:** Yes

**Dependency reason:** Needs Task 2 (`scan_word`), Task 3 (predicate), Task 4 (full level in fixtures).

**What to build:** The accounting step that turns the resolver rows plus every other indexed row with the target's name into labeled sites, and the entry point that adds the text explanation. Nothing with the name is dropped.

**Approach:** Run the resolver for the target group with an internal limit of 5,000. Classify basis post hoc from each row's source and fields (carry `source`, `receiver`, `namespace`, `import_context`, and the row id in `#[serde(skip)]` fields or a private struct). For accounting against another definition D, run `pending_target_predicate` for D restricted to the unclaimed pending row ids (one query per D), and the identifier receiver check per unclaimed identifier; classify the match's basis relative to D, and exclude only on a proven basis. Order other definitions by shared leading path segments with the target, cap 12, budget 1.5 s; past the cap or budget, the remaining sites stay `unresolved` with basis `{k} definitions` and the count of unchecked definitions appears in `cannot_see`. Drop identifier rows that coincide with a definition of a symbol with the name (same path and line as the symbol's name line). Keep `find_references*` behavior unchanged.

**Acceptance criteria:**
- [ ] One fixture test per basis string, each asserting label and basis.
- [ ] Accounting tests: a site claimed by another definition with a typed receiver is `excluded` with `resolves to …`; a receiver imported from an unindexed module is `excluded`; a call in another language family is `excluded` with `other language`; a name with one definition gives `likely` / `name only`; a name with 3 definitions and no evidence gives `unresolved` / `3 definitions`.
- [ ] No-drop invariant test: for a fixture with pending rows, call identifiers, variable references, and relationships to two definitions, the number of report sites equals the number of distinct `(path, line)` name sites after pairing, and each appears once.
- [ ] Exact-line test: a multi-line JS chained call reports the identifier's line, and its text hit is explained by the site.
- [ ] Text explanation test: definition lines, import lines, and a comment mention are counted as definition, import, and text only; `counts.text_lines == counts.site_lines + counts.definition_lines + counts.import_lines + counts.text_only_lines`.
- [ ] Constructor test: `skipped` is set and no scan runs.
- [ ] Budget test: with 13 definitions sharing a name, at most 12 are checked and the note appears.
- [ ] Existing tests unchanged and passing: `references_test`, `resolution_test`, `blast_radius_test`, `disambiguation_test`, `qt_cpp_test`, `qt_qml_test`, plus the new test file; clippy passes; commit on the task branch.

### Task 7: Ground truth cases

**Files:**
- Create: `evals/reach/cases/<repo>.json` (one agent per repo)

**Interfaces:**
- Consumes: Task 1 scripts and case format; v2.6.0 binary `/home/murphy/.local/bin/code-kb` for the baseline disagreements.
- Produces: one case file per repo with 30 symbols (15 dev, 15 holdout), truth, and adjudications; a baseline report per repo in `target/reach-gate/baseline-<repo>/`.

**Contract inputs:** Indexer commands, versions, and quirks from the 2026-10-17 tooling scout (`/tmp/claude-1000/-home-murphy-source-code-kb/d9541551-34eb-4370-8df7-d54f8f047719/scratchpad/map-gate-tooling.txt`). Tool caches under `/home/murphy/.cache/gate-tooling-scratch/` and `~/.cache/code-kb-reach/tools/` (never under `/tmp`, which is a 16 GB tmpfs). Python truth needs the package installed editable in a venv. Express methods assigned as properties need hand-checked truth (`truth_source: "manual"`). C# uses the library project only (scope the symbols and truth to `Src/Newtonsoft.Json/`). Rust strips `use` lines. Every adjudication reads the source line and its context; a `true` verdict names the call or value use, a `false` verdict names why it is not a reference to the target.

**File ownership:** Create `evals/reach/cases/<repo>.json` for flask, express, zod, gson, cobra, newtonsoft-json, code-kb; one agent per repo owns one file

**Serialization required:** No (within batch)

**Dependency reason:** Needs Task 1 scripts.

**What to build:** Frozen, adjudicated truth for 30 sampled functions and methods per repo, so the gate can score any code-kb build offline.

**Approach:** Materialize, build the SCIP index, `sample`, `truth`, run the baseline, list disagreements for every symbol (dev and holdout), adjudicate each, write the case file.

**Acceptance criteria:**
- [ ] 30 symbols, 15 per split, at least 4 tags represented where the repo has them.
- [ ] Every baseline disagreement has an adjudication with a note.
- [ ] `reach_gate.py run --binary /home/murphy/.local/bin/code-kb --repos <repo>` runs to completion; the per-language baseline numbers are recorded in the task report.
- [ ] Commit the case file on the task branch.

### Task 8: Report output in MCP and CLI

**Files:**
- Modify: `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/src/main.rs`
- Test: `crates/code-kb-cli/tests/cli_test.rs`, `crates/code-kb-cli/tests/mcp_test.rs`, `crates/code-kb-cli/tests/adversarial_m3_test.rs`, formatter unit tests

**Interfaces:**
- Consumes: `ReferenceReport`, `select_reference_target`, and `reference_report` (Task 6).
- Produces: `formatters::format_reference_report(report: &ReferenceReport, limit: usize) -> String`; MCP `find_references` callers and CLI `refs` callers print it; CLI `--json` callers prints the report object; callees output is unchanged.

**Contract inputs:** Global Constraints header lines, description string, and the design doc section "Output". Listing: up to `limit` site rows in label order; each group starts with `Proven ({n}):`, `Likely ({n}):`, `Unresolved ({n}):`, `Excluded ({n}):`; a row is `- {caller} [{path}:{line}] (kind: {kind}; {basis})` where `{caller}` follows today's rules (`` `name` ``, `(no enclosing symbol)`, `` `<lambda> in enclosing` ``). Rows past the limit fold into `… {m} more {label} sites in {f} files: {path} ({c}), …` with at most 5 files named. `Text only ({n}):` lists `  {path}: {l1}, {l2}, …` for at most 10 files and 12 lines each, then `  … {m} more lines in {f} files`. Then the `cannot_see` line as `Not visible to this index: {items joined by '; '}.`, then today's import summary line. `limit` 0 keeps today's `ZERO_LIMIT_NOTICE`. CLI `--json` callers prints the whole `ReferenceReport` (every site, whatever `--limit` says); `--limit` caps only the text listing.

**File ownership:** Modify `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/src/main.rs`, tests in `crates/code-kb-cli/tests/cli_test.rs`, `crates/code-kb-cli/tests/mcp_test.rs`, `crates/code-kb-cli/tests/adversarial_m3_test.rs`, `crates/code-kb-core/src/formatters.rs` unit tests

**Serialization required:** Yes

**Dependency reason:** Needs Task 6 types and `reference_report`.

**What to build:** The text and JSON rendering of the report in the MCP tool and the CLI, with the new tool description, and updated tests that assert the labels.

**Approach:** Keep `format_references` for callees. Update the tests that assert old callers text (`receiver_reference_labels_match_the_mcp_contract`, `receiver_reference_labels_match_the_cli_contract`, `cli_renders_reference_sites_without_enclosing_symbols`, `test_cli_refs`, `test_cli_refs_with_file_filter`, `test_cli_json_strict_forward_slash_invariants`, the `--json refs helper` case at adversarial_m3_test.rs:442) so they assert the same sites with their labels. Telemetry `logical_result_count` becomes `counts.sites`.

**Acceptance criteria:**
- [ ] MCP and CLI text are byte-identical for the same call (test).
- [ ] Formatter tests for the three header lines (plural and singular), each group, folding past the limit, text-only folding, the budget-cut line, and the constructor line.
- [ ] JSON test: object with `target`, `counts`, `sites[].label`, `sites[].basis`, `text_only`, `text_scan`; forward slashes only.
- [ ] `tools/list` tests still pass with the new description; schema properties unchanged.
- [ ] `cargo test -p code-kb-cli` and `cargo test -p code-kb-core --lib formatters` pass; clippy passes; commit on the task branch.

### Task 9: Docs and gate run

**Files:**
- Modify: `CLAUDE.md`, `AGENTS.md`, `README.md`, `skills/code-kb/SKILL.md`
- Create: `docs/reviews/2026-10-17-accuracy-base-gate.md`

**Interfaces:**
- Consumes: merged Tasks 1-8; gate reports for v2.6.0 and the candidate.
- Produces: docs that describe the labeled report, the `full` level and its measured cost, and the gate in the release workflow; the gate review with numbers.

**Contract inputs:** Doc locations from the 2026-10-17 wiring map: `CLAUDE.md` Core Invariant 5 reference rules and Core Invariant 7 level and release bullets; `README.md` tool table row for `find_references` and the reference section; `skills/code-kb/SKILL.md` caveat line ("A caller found through an unresolved call is a best guess, and the list can miss callers"). STE writing rules from the owner's instructions.

**File ownership:** Modify `CLAUDE.md`, `AGENTS.md`, `README.md`, `skills/code-kb/SKILL.md`; create `docs/reviews/2026-10-17-accuracy-base-gate.md`

**Serialization required:** Yes

**Dependency reason:** Needs Tasks 6-8 merged and the gate report.

**What to build:** Accurate docs and a review that reports baseline and candidate gate numbers, latency, memory, and index size.

**Approach:** The lead runs the gate and the measurements; the fix loop for gate failures uses dev disagreements only.

**Acceptance criteria:**
- [ ] `cmp AGENTS.md CLAUDE.md` passes.
- [ ] The review has per-language proven precision, recall, and accounted share for v2.6.0 and the candidate (dev and holdout), refs p50/p95 on aspnetcore and hermes-agent, `serve` RSS before and after, and index size and scan time.
- [ ] Branch gate passes.
