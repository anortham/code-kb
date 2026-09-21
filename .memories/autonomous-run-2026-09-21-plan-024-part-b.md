# Autonomous Execution Report - Plan 024 Part B (v1.4.0 edit_file, recovery, facts, telemetry)

**Status:** Awaiting publication approval
**Plan:** docs/plans/024-v1.3.1-lookup-fixes-and-v1.4-edit-file.md
**Branch:** feat/v1.4 (worktree `.worktrees/feat-v1.4`), rebased onto `main` at 0ee82c6
**PR:** not created (this repository releases from `main`; see Next steps)
**Publication authority:** local commit=authorized (plan approved 2026-09-21, "approved, codex review, use workflows with opus implementation agents"); push=missing; PR=missing; tag and crates publish=missing
**Duration:** about 3 hours (workflow 8 tasks in series, then lead review, rebase, two Codex passes, and three fix commits)
**Phases:** 1/1 complete
**Tasks:** 8/8 complete (Task 13's push, CI, tag, and publish boxes wait on the owner)
**External-model policy:** no policy declared in the repository; codex (openai) received the redacted diff twice

## What shipped
- Task 6 (8b208d2): `edit_file` core in `edit.rs`. `commit_file_edit` is the one write path for both edit tools: syntax check, hash check, atomic write, re-index, rollback on a failed re-index.
- Task 7 (9d75430): the `edit_file` MCP tool and `code-kb edit-file`, with `occurrence` and the aliases `file`/`path`, `old`/`find`, `new`/`replace`.
- Task 8 (0b63b98): every not-found answer names the bound workspace and up to three near candidates, for symbols and for file paths.
- Task 9 (9ce8745): tokens saved has a baseline for every read tool; the header states how many calls the baseline covers; new column `est_tokens_saved_known`.
- Task 10 (790ef95): `CATEGORY_ALIASES` is the whole alias table for structural facts; facts and literals each get the whole limit with a cap notice.
- Task 11 (7801015): routing block, both skills, README, site, AGENTS.md, CLAUDE.md, and the TODO pointer name `edit_file`; the compiled routing block is kept equal by a test.
- Task 12 (d88a848): version 1.4.0 in every manifest and `docs/release-notes/v1.4.0.md`.
- Task 13 (d5dd7e1): branch gate, memory, and regression numbers recorded in the plan.
- Lead fix (dbbcd43): the touched list of an edit names the enclosing definition, not a local variable on the edited line.
- Review fix (b778619): see External review.

## Judgment calls (non-blocking decisions made)
- Task 4 of Part A had already shown that dropping `symbols_fts` triggers a rebuild; Part B tests reuse that shadow-table approach where a broken index is needed.
- The `model` alias maps to six SQL and JSON data-definition pattern ids, because the extractor has no `model` family of its own.
- The ambiguity message names ten lines and then a count, so a million matches cost a short message, not a seven-megabyte one.
- `all` replaces non-overlapping matches only; overlapping starts still count for `only`, `first`, and `last`.

## External review (codex, adversarial)
- **Passes:** general 1 / security 1
- **Findings:** 4 as returned, 3 after dedupe (one allocation defect was flagged by both passes)
- **Verified real, fixed:** 3 (commit: b778619)
  - [dual-flagged] high: `exact_matches` cloned the replacement for every match before `occurrence` applied; 1 MiB of one character with a 4 KiB replacement asked for 4 GiB. A match is now an offset pair, the replacement is built when applied, and a result above 8 MiB is refused.
  - [general] medium: the cursor skipped overlapping matches, so `ana` in `banana` passed the ambiguity check and `last` edited the wrong place. Overlapping starts are found now.
  - [security] medium: a failed edit's error quotes three file lines and the server stored it verbatim in `~/.code-kb/telemetry.db`. Telemetry now records only the first line of an error.
- **Dismissed:** 0
- **Flagged for your review:** 0
- **Cost:** not reported by codex-cli
- Note: both passes ran with `GCM_CREDENTIAL_STORE` unset, because the razorback redactor treats that value as a secret (see the Part A report). No invocation was lost.

## Review campaign
- **State:** clean
- **Evidence:** external-reviewed
- **Round:** 2/2
- **External invocations:** 2/2
- **Open critical/high:** 0
- **Open medium/low:** 0
- **Open at/above floor:** 0

## Tests
- Branch gate on b778619: `cargo test --workspace --locked` 375 passed, 0 failed; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all -- --check` clean; `node --test tests/plugin/*.test.cjs` 17 passed.
- Every fix commit started from a red test: the touched-symbols case, two overlap cases, the non-overlapping `all` case, the million-match case, and the telemetry first-line case.
- Live serve RSS after 20 searches: 16.6 to 16.7 MiB against a 28 MiB gate (Task 13, on d88a848; no code that runs in `serve` changed after that measurement except the edit path).
- Search regression set: 25 / 26 / 23 / 25 on both the candidate and the v1.3.0 binary; the plan's Results section explains the one case that differs from the v1.3.0 notes (`concept-fts`, the repository moved on). Please confirm this reading.
- Smoke on this repository with the release build: `code-kb body format_sybmol_body` names `format_symbol_body`; `code-kb skeleton crates/code-kb-cli/src/mcp/format.rs` names `formatters.rs`; `code-kb facts sql --limit 3` returns SQL literals and no CSS rows; `code-kb facts` opens with the alias line.
- `scripts/release-preflight.sh` has not run yet; step 8 needs the push and green CI.

## Blockers hit
- None. Push, CI, tag, and crates publish wait on the owner's approval.

## Files changed
- 41 files, 3148 insertions, 379 deletions against `main`. Core: `crates/code-kb-core/src/edit.rs`, `queries.rs`, `telemetry.rs`, `ops.rs`, `lib.rs`, tests `edit_test.rs`, `disambiguation_test.rs`, `references_test.rs`. CLI: `crates/code-kb-cli/src/mcp/server.rs`, `main.rs`, `formatters.rs`, `routing-block.md`, tests `mcp_test.rs`, `cli_test.rs`. Docs and manifests: README, AGENTS.md, CLAUDE.md, both skill sets, hooks routing block, site, TODO.md, release notes, version manifests, plugin tests.

## Source control
- **Outstanding:** None. All commits ride on feat/v1.4, ten ahead of `main` (0ee82c6, equal to origin/main).
- **Other worktrees, all clean and merged:** `.claude/worktrees/feat-search-v1.3`, `.claude/worktrees/fix-v1.3.1-lookup`, `.worktrees/feat-search-v1.2`. Left for the owner to remove.

## Next steps
- Owner approves: fast-forward `main` to feat/v1.4, push, wait for green CI on the release commit, run `scripts/release-preflight.sh`, tag `v1.4.0`, publish `code-kb-core` then `code-kb-cli`, record the archive checksums in `docs/release-notes/v1.4.0.md`.
- After the release: remove the three merged worktrees and their branches.
