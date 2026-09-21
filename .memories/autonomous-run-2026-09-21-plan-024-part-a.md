# Autonomous Execution Report - Plan 024 Part A (v1.3.1 lookup fixes)

**Status:** Awaiting publication approval
**Plan:** docs/plans/024-v1.3.1-lookup-fixes-and-v1.4-edit-file.md
**Branch:** fix/v1.3.1-lookup (worktree `.claude/worktrees/fix-v1.3.1-lookup`)
**PR:** not created (this repository releases from `main`; see Next steps)
**Publication authority:** local commit=authorized (plan approved 2026-09-21, "approved, codex review, use workflows with opus implementation agents"); push=missing; PR=missing
**Duration:** about 75 minutes (workflow 18 minutes, review and fix 55 minutes)
**Phases:** 1/1 complete
**Tasks:** 5/5 complete (Task 5's two release boxes wait on the push)
**External-model policy:** no policy declared in the repository; codex (openai) received the redacted diff twice

## What shipped
- Task 1 (f35c1d3): an exact-name lookup returns the row it names, whatever its test status; substring rows still honor the test filter.
- Task 2 (8f00348): a qualified name is checked against the whole parent chain; a bogus ancestor no longer resolves.
- Task 3 (ffdcf5a): a qualified lookup accepts a directory prefix as its path filter, in both slash styles.
- Task 4 (1bb434d): the lookup full-text fallback reports a database error instead of an empty answer, in the MCP server and the CLI.
- Task 5 (c735ae3): version 1.3.1 in every manifest, release notes, and the one-sentence lookup rule in README, AGENTS.md, CLAUDE.md, and both SKILL.md copies.
- Review fix (6ef0bda): a qualified name with several ancestor segments reads up to 2000 candidates before the chain check instead of 25.

## Judgment calls (non-blocking decisions made)
- `crates/code-kb-core/tests/adversarial_m1_test.rs` — four directory-prefix filters moved from the negative list to a positive assertion, because the old negatives encoded the defect Task 3 removes; the boundary negatives (`Services`, `Services/`) stay.
- `crates/code-kb-cli/tests/mcp_test.rs`, `cli_test.rs` — the broken-index tests drop the FTS shadow table `symbols_fts_data` instead of `symbols_fts`, because the readiness check would otherwise rebuild the table before the fallback runs; the asserted error text is the real fts5 corruption message.
- Task 2 acceptance named `mcp::server::McpServer::sanitize_symbol_name` as a positive case; the index records no `server` module symbol, so the agent verified the real chain and used a three-level test-module name instead.

## External review (codex, adversarial)
- **Passes:** general 1 / security 0
- **Findings:** 1
- **Verified real, fixed:** 1 (commits: 6ef0bda)
  - [general] high: the resolver capped candidates at 25 rows before the ancestor walk, so a valid chain past the cap was missed and a second valid chain past it hid an ambiguity. Reproduced with two red tests, fixed by binding the row cap (25 normally, 2000 when the walk applies).
- **Dismissed:** 0
- **Flagged for your review:** 0
- **Cost:** not reported by codex-cli
- Note: the security pass first failed at the outbound redactor, before Codex ran, because the razorback redactor treats the `GCM_CREDENTIAL_STORE` environment value (a five-letter word) as a secret and that word occurs inside "cached" in the security template. The pass ran with that variable unset. No invocation was consumed by the failed attempts.

## Review campaign
- **State:** clean
- **Evidence:** external-reviewed
- **Round:** 2/2
- **External invocations:** 2/2
- **Open critical/high:** 0
- **Open medium/low:** 0
- **Open at/above floor:** 0

## Tests
- Branch gate on c735ae3: `cargo test --workspace --locked` all targets green (exit 0), clippy with warnings denied clean, `cargo fmt --check` clean, 16 plugin tests pass.
- After the review fix (6ef0bda): `cargo test -p code-kb-core` 271 tests across 15 targets green, `cargo test -p code-kb-cli` 6 targets green, clippy clean, fmt clean. The two new regression tests were confirmed red without the fix.
- `scripts/release-preflight.sh` steps 1 to 7 passed in Task 5; step 8 needs the push and green CI.

## Blockers hit
- None. Push, tag, and crates publish wait on the owner's approval.

## Files changed
- 23 files, 395 insertions, 83 deletions: `crates/code-kb-core/src/queries.rs` (+112/-27), `crates/code-kb-core/tests/disambiguation_test.rs` (+156), `crates/code-kb-core/tests/adversarial_m1_test.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/tests/mcp_test.rs`, `crates/code-kb-cli/tests/cli_test.rs`, version manifests, `docs/release-notes/v1.3.1.md`, README, AGENTS.md, CLAUDE.md, both SKILL.md copies, `docs/site/index.html`, the plan's ticked boxes.

## Source control
- **Outstanding:** None — all commits ride on fix/v1.3.1-lookup. `main` is at bdb2bef (the plan commit, one ahead of origin/main).
- **Worktrees left in place:** `.claude/worktrees/feat-search-v1.3` and `.worktrees/feat-search-v1.2` (clean, fully merged, left for the owner); `.claude/worktrees/fix-v1.3.1-lookup` (this run).

## Next steps
- Owner approves: push `main` with `fix/v1.3.1-lookup` fast-forwarded onto it, wait for green CI, run `scripts/release-preflight.sh`, tag `v1.3.1`, publish `code-kb-core` and `code-kb-cli`, record the archive checksums in the release notes.
- Then Part B of plan 024 (v1.4) on a new worktree from `main`.
