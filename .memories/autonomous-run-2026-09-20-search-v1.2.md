# Autonomous Execution Report - Search v1.2: name recall and deterministic rerank (plan 020)

**Status:** Awaiting publication approval
**Plan:** docs/plans/020-search-v1.2-recall-and-rerank.md
**Branch:** feat/search-v1.2-recall-rerank (worktree .worktrees/feat-search-v1.2, base a42bd4d, head 262adb2)
**PR:** pending — the documented release flow fast-forwards `main` and pushes; the user picks PR or fast-forward
**Publication authority:** local commit=authorized (user request "we need to implement the new plan", plan 020 task 7); push=missing (user CLAUDE.md: pushes and releases need approval); PR=missing; tag and crates publish=missing
**Duration:** one session, 2026-09-20 18:40Z to 21:20Z
**Phases:** 1/1 complete
**Tasks:** 7/7 complete (task 7 prepared up to the approval boundary)
**External-model policy:** none declared — no external model received the diff

## What shipped
- Task 1: evaluation sets outside the checkout at `~/.code-kb/search-eval/` (regression 26, development 89, sealed acceptance 40; seal 554a3922…), runner, v1.1.4 baselines.
- Task 2 (bc9d844): synthetic multi-language corpus gate `crates/code-kb-core/tests/search_corpus_test.rs`, 12 top-1 assertions.
- Task 3 (8ab5a50, 6997b20): `symbol_names_tri` trigram FTS5 table beside `symbols_fts`; one `BEGIN IMMEDIATE` migration with the marker written last, readiness over both tables, re-check under the lock, 60 s busy timeout; migration errors surface in `scan_workspace`, the CLI, and the MCP server (first tool call reports, next call retries); extractor-driven trigger test.
- Task 4 (09a6526, 5d844d3): three recall branches (word 40-160, trigram name 20-40, exact name through the trigram index) merged by rowid; measurements on code-kb and hermes-agent.
- Task 5 (b757458, 04a4f38, 31b37e0, fe93213): deterministic rerank from one weight table (name tiers with rarity-weighted coverage sampled from the word branch, token-level signature and doc coverage, kind prior, path role, documentation last, test intent), `--explain` on the CLI, `score` = rerank score, docs and MCP description; corpus gate 12/12.
- Task 6 (49eb86f): evaluation results recorded in plans 019 and 020.
- Task 7 (262adb2): version 1.2.0 in every manifest, `docs/release-notes/v1.2.0.md` with the migration costs; preflight steps 1-7 pass, step 8 waits for the push.

## Judgment calls (non-blocking decisions made)
- `crates/code-kb-core/tests/search_corpus_test.rs` — five red gate tests carried `#[ignore]` between tasks 2 and 5 so every commit kept `cargo test --workspace` green; task 5 removed the attribute.
- `crates/code-kb-core/src/db.rs` — task 3 created the trigram table, triggers, and population (design 2 says one transaction populates both tables); task 4 only reads it.
- `crates/code-kb-core/src/queries.rs` (name branch) — ordered by `bm25(symbol_names_tri)` then name length, not name length alone as the plan text says, because an OR of several words let short names matching one common word crowd out the target.
- `crates/code-kb-core/src/queries.rs` (exact branch) — served through the trigram index with `length(name) = length(query)` instead of `COLLATE NOCASE`, which scanned every row (39 ms on 991k rows).
- `crates/code-kb-core/src/queries.rs` (rerank) — coverage is weighted by word rarity inside the word branch's matched rows (`ln(1 + N/(df+1))`), a design refinement after the regression set showed binary coverage treats `discover` like `path`; tiers stay binary; a partial name hit scores at least `W_NAME_ANY` so it always beats a bare kind prior.
- `crates/code-kb-core/src/queries.rs` (text coverage) — signature and doc words match at token boundaries (word or stem prefix), because substring matching credited `stem` inside "system".
- Weights tuned on the development set only: signature 8 -> 4, doc 6 -> 18, documentation row -100 -> -200 (strict last tier).
- Ruling: whole-name matches of any kind outrank all-words functions (plan rule); `concept-telemetry` symbol rank 1 -> 2 (`TelemetrySummary` struct) accepted as contested.
- Ruling: `concept-fts` file rank 1 -> 2 (`create_index` above `ensure_fts_index`) accepted as contested at the fix-round cap; both are plausible answers and totals rose on every set.
- Sealed acceptance set run once and reported as is; observed causes (README vocabulary gaps, test-file symbols the extractor did not flag) recorded as follow-ups, not tuned.
- Lead rule after one incident: only one worker commits in the shared worktree at a time (a parallel commit swept a sibling's staged files; rewritten before any push).

## External review
External review: none (not requested for this run).

## Review campaign
- **State:** not run
- **Evidence:** lead-only
- **Round:** 0/0
- **External invocations:** 0
- **Open critical/high:** 0
- **Open medium/low:** 0
- **Open at/above floor:** 0

## Tests
- Branch gate at fe93213: `cargo test --workspace` 305 passing, 0 failing; plugin tests 16 pass; clippy `-D warnings` clean; fmt clean; `cmp AGENTS.md CLAUDE.md` identical.
- Release preflight at 262adb2 (steps 1-7): sync contract, fmt, clippy, `cargo test --workspace --locked`, plugin tests, `cargo package`, Windows guest suite — all pass; step 8 (CI on HEAD) needs the push.
- Windows (`win-test`): 274 passing at 6997b20, 301 passing at fe93213, full suite again inside the preflight at 262adb2.
- Security scope: none declared in the plan; skipped.
- Evaluation: regression set file@1 22 -> 25, sym@1 17 -> 21 (2 of 26 rank one lower, ruled); development 55 -> 69 file@1, 42 -> 61 sym@1; acceptance 19/40 file@1, 14/40 sym@1 (target file@1 >= 8 and sym@1 >= 7 per repo met only by code-kb file@1).
- Latency and memory: warm search median 24.3 ms (target 25); rerank 0.44 ms at 177 candidates (target 1 ms at 200); live `serve` RSS +0.2 MB (noise); migration 25-34 ms on code-kb, 3.9-5.8 s on hermes-agent (+1.8% db, WAL peak 73.4 MiB, peak RSS 9.5 MB).

## Blockers hit
- None. Approval boundary: push, tag `v1.2.0`, and crates.io publish wait for the user.

## Files changed
- 43 files, +3185 / -210 (`git diff --stat a42bd4d..262adb2`). Code: `crates/code-kb-core/src/{queries.rs,db.rs,models.rs,formatters.rs,lib.rs,sync.rs}`, `crates/code-kb-cli/src/{main.rs,mcp/server.rs,routing-block.md}`, tests `search_corpus_test.rs`, `fts_triggers_test.rs`, `cli_test.rs`. Docs: README, CLAUDE.md/AGENTS.md, plans 019 and 020, release notes v1.2.0, routing block and skill copies, site version. Manifests: Cargo, four plugin manifests, release workflow. Dependency: `rust-stemmers` 1.2.0.

## Source control
- **Outstanding:** None — all 11 commits ride on feat/search-v1.2-recall-rerank. Main checkout `/home/murphy/source/code-kb` stays at a42bd4d on `main`, clean (its `.memories/` match HEAD; its git-ignored `.code-kb/artifact.db` was migrated to the new layout by the evaluation runs and flips back whenever the still-running v1.1.4 MCP server touches it, until that server restarts on the new binary). Branch `feat/pin-julie-extract-v2.42.3` deleted at the user's request. Evaluation results and sets stay in `~/.code-kb/search-eval/` (never inside the checkout).
- **Worktrees left in place:** `.worktrees/feat-search-v1.2` (this run's; disposition after the release).

## Next steps
- Approve the release path: fast-forward `main` to 262adb2 and push (documented flow), or push the branch and open a PR.
- Then: wait for green CI on the exact commit, `git tag -a v1.2.0`, push the tag, let `release-binaries` publish six archives, record checksums in `docs/release-notes/v1.2.0.md` (`gh release edit --notes-file`), publish `code-kb-core` then `code-kb-cli` to crates.io, verify `cargo install`.
- Restart harness sessions after `cargo build --release` in the main checkout so the running MCP server uses the new binary.
- Follow-ups outside plan 020: demote rows from test paths the extractor did not flag (three hermes-agent acceptance misses); the per-command reconcile walk dominates CLI latency on 12k-file repositories; container expansion if a later acceptance run shows the need.
