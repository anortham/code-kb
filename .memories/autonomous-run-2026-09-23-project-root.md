# Autonomous Execution Report - Required `project_root` on every tool (issue #3, 2.1.0)

**Status:** Awaiting publication approval
**Plan:** docs/plans/2026-09-23-mcp-roots-worktree-binding.md
**Branch:** feat/project-root (worktree `.claude/worktrees/project-root`, base `58b9468`)
**PR:** not created — push and PR need approval
**Publication authority:** local commit=authorized (user: "we're ready to start the plan"); push=missing (user CLAUDE.md: push needs approval); PR=missing (same source)
**Duration:** about 2.5 h (2026-09-23; commits from 20:26 to 22:11 UTC, then acceptance runs)
**Phases:** 1/1 complete
**Tasks:** 4/4 complete, plus 4 review-fix rounds
**External-model policy:** no external model received the diff (all reviewers were Claude subagents in this session)

## What shipped
- Core: `Workspace::from_project_root` resolves a call's `project_root` (plain path or `file://` URI; a subfolder resolves to its project). It refuses a relative path, a missing path, a filesystem root, the home directory, and a folder with no project marker and no index. It never creates files.
- Core: `Workspace::nested_project_root` finds a nested git worktree or submodule. A language project under a dotfiles home (`~/.git`) resolves to that project, also in `Workspace::discover`.
- Server: every tool except `telemetry_summary` requires `project_root` (aliases `workspace`, `root`, never in a schema). The server resolves it once per call and keeps one active index.
- Server: a switch parks a running index build and reuses it on the way back. Every index build starts its root's watcher after the scan. A call waits up to 5 s (`CODE_KB_INDEX_WAIT_MS`), then answers `Indexing <root> started; call again in a few seconds.` A failed first scan retries on the next call.
- Server: an absolute path outside `project_root`, or inside a nested worktree or submodule, is refused with a text that names both roots. `--db` pins only the launch root, and it counts as the index of a markerless launch root. A refused call records the launch root in telemetry. The server no longer binds from `initialize` roots or from path arguments. Startup skips the index build for a refused launch root (home, install folder).
- CLI: index-reading commands refuse a home directory or a filesystem root like the tools, and create a missing index only for an accepted root. `code-kb scan` is unchanged.
- Docs: AGENTS.md and CLAUDE.md invariants 1, 5, 6; both routing-block copies; both SKILL.md copies; README (GUI configs no longer need `--root`); site page; ADR 001 marked superseded.

## Judgment calls (non-blocking decisions made)
- `crates/code-kb-cli/src/mcp/server.rs` `switch_root` — Chose to park a running index build per root and reuse it over starting a new one, because a scan writes into `artifact.db` in place and two scans of one index must never run.
- `server.rs` `spawn_index_prepare` — Chose to run the extractor version check inside the build thread over the old synchronous call, because a rebuild would block a call past the 5 s bound. Startup also no longer blocks on it. Cost if wrong: the first call after an extractor upgrade answers "Indexing ... started".
- `server.rs` `spawn_index_prepare` — Chose one owner for each watcher (the index build) over separate start points, because two start points left a window where edits were not indexed (review findings R1, C1, C2).
- `server.rs` bounded wait — Chose to apply the 5 s bound to a reconcile of an existing index too, because the plan says "missing or stale index".
- `crates/code-kb-core/src/workspace.rs` — Chose to refuse the home directory even when it has a `.git` (dotfiles repo), with a fallback to the nearest project below it, because an index of the whole home is the worst known failure (Julie and Miller history).
- `workspace.rs` `nested_project_root` — Chose `.git` only as a nested-project marker over `.git` plus an index, because an old member index in a non-git monorepo must not block the parent project.
- `server.rs` `resolve_root` — Chose to accept a markerless launch root when `--db` names an existing file, because that setup worked before this branch.
- `server.rs` `McpServer::new` — Chose to skip the startup index build for a launch root the resolver refuses, because the docs now tell GUI users to omit `--root` and a GUI app may start the server in the home folder.
- `crates/code-kb-cli/src/main.rs` — Chose to make the CLI refuse a home or filesystem root, although the plan said "no CLI change", because the old CLI scanned the whole home under a dotfiles home, and CLI 1:1 parity asks for the same rule.
- README — Chose to drop `--root` from the GUI config examples and document it once as an optional pre-warm, because `project_root` makes the flag unnecessary.
- Task 4 ran in an isolated agent worktree and was cherry-picked (`3cf1851` → `9d13b7a`), because concurrent cargo builds in one tree break each other.

## External review (none, adversarial)
External review: none (not requested for this run). Internal reviews replaced it; see the review campaign.

## Review campaign
- **State:** clean
- **Evidence:** fresh-session (Claude workflow subagents, same model)
- **Round:** 3 review rounds plus lead inline review of every commit
- **External invocations:** 0
- **Open critical/high:** 0
- **Open medium/low:** 0 (Minor items fixed; three deferred Minor notes below)
- **Open at/above floor:** 0

Review rounds:
1. Branch review (4 lenses, 1 skeptic per finding): 10 confirmed, 0 refuted (3 Important, 7 Minor). All fixed in `ee0b471` and `be7e1e4`.
2. Fix-diff review (2 lenses): 5 confirmed Minor, 0 refuted. All fixed in `cf147ec`, plus a CLI guard.
3. Round C review (1 reviewer): 1 confirmed Minor. Fixed in `1f7deb7`, docs in `87d4a25`.

## Tests
- Linux branch gate at `87d4a25`: `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace --locked` 453 passed, 0 failed; `node --test tests/plugin/*.test.cjs` 17 passed.
- Windows (win-test guest, NTFS) at `87d4a25`: `cargo test --workspace --locked` 444 passed, 0 failed. The Windows-only drive-casing test ran.
- Baseline before the change: 409 passed.
- Black-box acceptance (Herdr, fixture `~/source/kb-e2e/flask-base`, branch release binary, no hint about code-kb; prompt "Spike a small feature in a new git worktree: add a --json flag to the flask routes command ..."):
  - Claude x4 (EnterWorktree): every code-kb call named the worktree as `project_root`.
  - Codex x2 (`git worktree add` to a sibling folder): every call after the worktree step named the worktree; calls before it named the main checkout.
  - Grok `-w` x1 (server started in the main checkout): no call named the main checkout; all named Grok's worktree or the nested worktree the prompt made it create.
  - No call failed for a missing `project_root`.

## Blockers hit
- None.

## Files changed
- `crates/code-kb-cli/src/mcp/server.rs` +390/- (per-call root, switching, bounded wait, refusals)
- `crates/code-kb-core/src/workspace.rs` +427/- (resolver, nested roots, dotfiles-home fallback, unit tests)
- `crates/code-kb-cli/src/main.rs` +26/- (CLI refusal)
- `crates/code-kb-cli/tests/mcp_test.rs` +1546/- (new and migrated tests)
- `crates/code-kb-cli/tests/adversarial_m2_server_test.rs`, `adversarial_m3_server_test.rs`, `cli_test.rs` (migrated and new tests)
- `AGENTS.md`, `CLAUDE.md`, `README.md`, `skills/code-kb/SKILL.md` (+ plugin copy), both routing blocks, `docs/site/index.html`, `docs/decisions/001-zero-workspace-parameters.md`
- `.memories/` checkpoints
- Total: 26 files, +2737/-820 (`git diff --stat 58b9468..87d4a25`)

## Source control
- **Outstanding:** None — all commits ride on feat/project-root. The main checkout is clean and 1 commit ahead of origin (`58b9468`, the plan commit, not pushed; a PR from this branch carries it).
- **Worktrees left in place:** `/home/murphy/source/code-kb/.claude/worktrees/project-root` (this branch, until the user chooses how to integrate it). The Task 4 agent worktree was removed after its cherry-pick. Grok's own session copy stays under `~/.grok/worktrees/kb-e2e-flask-base/` (Grok manages it).

## Deferred notes (Minor)
- Serve still creates `<launch root>/.code-kb/logs` when the launch root is not a project (older behavior; now more visible because GUI docs drop `--root`).
- An unknown tool name without `project_root` gets the missing-root error before "Unknown tool".
- The retry text after a failed scan can promise a retry that cannot run when a markerless launch root's pinned `--db` file is deleted mid-session.
- Goldfish checkpoints written from a worktree record the main checkout's branch and path; each checkpoint here was corrected by hand.

## Next steps
- Approve push of feat/project-root and a PR to main, or merge it locally.
- Release 2.1.0 per `docs/RELEASING.md` (version bump, release notes, tag): not done, needs approval.
- After the release: `cargo build --release` in the main checkout and restart sessions, so the dev harnesses use the new binary.
