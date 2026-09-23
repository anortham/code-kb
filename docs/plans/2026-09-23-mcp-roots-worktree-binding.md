# Required `project_root` on every tool (issue #3, code-kb 2.1.0)

## Problem

1. A running `code-kb serve` stays bound to the launch root after Claude Code `EnterWorktree`.
   Unscoped calls (`lookup_symbol`, `search_symbols`, `codebase_outline`, ...) answer from the
   launch-root index with no warning. Commit `1f3f87a` only documents a workaround.
2. A user in project A asks the agent to explore project B. Today an absolute path rebinds the
   whole server to B, and later unscoped calls, and calls from subagents that share the server,
   answer from B.
3. With `serve --db <file>`, `locate_db` always returns that file, so a rebind to project B
   reconciles B against A's index file.

## Decision (user, 2026-09-23)

Every tool except `telemetry_summary` takes a required `project_root`: the absolute path of the
project or git worktree the agent works in. The server never picks the root from MCP roots, the
process directory, or a path argument. This replaces core invariant 1 (no workspace parameter).

Why:
- The agent always knows where it works. In all 3 fresh sessions below, the first code-kb call
  after the worktree step already passed an absolute worktree path.
- MCP roots cannot carry the answer: Codex and Grok send none, Antigravity sends `[]`, and
  `grok -w` starts the server in the main checkout.
- A root chosen by server state goes stale (issue #3, Serena #1496, the Julie and Miller
  history below). A root in every call cannot go stale inside the server.
- MCP `2026-07-28` deprecates roots and says to pass directories in tool parameters.

A separate parameter, not `path` or `file_path`: those already mean a file or a scope inside the
project, and one name with one meaning in every tool is easier for an agent to copy from call to
call.

The earlier design (poll `roots/list` per call, per-call repository selection by absolute path)
is dropped. The Codex run below shows that it breaks clients without roots.

## Evidence

Probes with a logging stdio server under `claude -p` (Claude Code 2.1.280, 2026-09-23):

- `initialize` declares `capabilities.roots.listChanged: true` and carries no roots.
- `roots/list` returns `file://<repo>`; after `EnterWorktree` it returns
  `file://<repo>/.claude/worktrees/<name>`; after `ExitWorktree` it returns `<repo>` again.
- No `notifications/roots/list_changed` arrives, and the server is not restarted.
- A server that blocks a `tools/call` until its own `roots/list` answer arrives gets that answer
  in 0.3-2 ms.
- A Bash `cd` into another repository, or into a worktree of the same repository, does not
  change the roots.

Client capabilities in `initialize` logs on this machine: Claude Code and Antigravity declare
`roots`; Codex and Grok do not. Public reports agree: an independent Claude Code probe saw the
same missing notification and proposed the same per-call poll with explicit-root precedence
(archcore-ai/cli#31); it also saw `--add-dir` add a second root. Gemini CLI reportedly sends
the notification.

MCP protocol `2026-07-28` deprecates roots (SEP-2577, merged 2026-05-15). They stay in the spec
for at least twelve months. The spec says existing implementations "SHOULD migrate to passing
directories or files via tool parameters, resource URIs, or server configuration". In that
revision a server gets roots through an `InputRequiredResult`, not a server-to-client request.

### Probe of four clients (`scripts/mcp-probe.py`, 2026-09-23)

| Client | Protocol offered | Roots | Server cwd with the client's own worktree start |
| --- | --- | --- | --- |
| Claude Code 2.1.280 | `2025-11-25` | `roots/list` follows `EnterWorktree` and `ExitWorktree`; no `list_changed` | `claude -w`: the worktree |
| Codex 0.156.1 | `2025-06-18` | none; MCP server env is dropped | `codex --worktree`: the worktree, under `~/.codex/worktrees/<id>/<repo>` |
| Grok 1.0.41 | `2025-11-25` | none | `grok -w`: the main checkout; only the shell runs in `~/.grok/worktrees/...` |
| Antigravity (agy 1.2.9) | `server/discover` with `2026-07-28` first, then `initialize` `2025-11-25` | `roots/list` answers `[]`; `--add-dir` sends `list_changed` | no worktree start |

- Grok reads a project `.grok/config.toml` only in a trusted folder.
- Antigravity answered `[]` only on the `2025-11-25` fallback. Its roots over `2026-07-28` are
  not probed yet.

### Fresh agent sessions on code-kb 2.0.2 (Herdr, clone of flask, 2026-09-23)

Task: "spike a feature in a new git worktree". No hint about code-kb or roots.

- Claude, 2 runs: the razorback worktree skill chose `EnterWorktree`. The first code-kb call
  passed an absolute path in the worktree, as the 2.0.2 routing block says. All calls
  answered from the worktree. A follow-up question used `grep` only.
- Codex, 1 run: `git worktree add` to a sibling folder, no `cd` for the server. The first call
  was `codebase_outline(path=<worktree>)`, then `lookup_symbol("routes_command")` with no path and
  `search_symbols(path="tests")`. All 9 calls answered from the worktree, only because 2.0.2
  keeps the server on the last absolute-path root.

Result: a root chosen from roots or from one path call breaks Codex; only the sticky 2.0.2
behavior saved this run.

## Prior art

- Bound at start (flag or cwd): Serena in `claude-code` mode, mcp-language-server, Zed. Breaks
  on worktrees (Serena issue #1496 is this bug).
- Directory on each call: JetBrains `projectPath` (optional, "always provide if known"),
  claude-context `path` (required), Sourcegraph `repo`, GitHub `owner`/`repo`.
- Sticky activate tool: Serena `activate_project`, code-index-mcp `set_project_path`. Serena
  issues show stale bindings carried into later work.
- Follow the client: the reference filesystem server replaces its directories with roots and
  refetches on `list_changed`; a Serena fork reads the client cwd before each call.

## Lessons from Julie and Miller

Both predecessors tried roots and then removed them, and both ended by requiring a workspace
selector on every call.

- Julie resolved roots on the first request (April, `e004927e`) and swapped the default on
  `list_changed`. A `roots/list` sent from inside `on_initialized` deadlocked Claude Code
  (`63013aa8`): Claude Code answers it only after `initialized` is processed. Julie deleted
  roots in `ebdc4050` because its new shared HTTP service has no sessions, not because stdio
  roots failed. The swap, session attachment, and deferred auto-index were deleted too
  (`20160dfe`, 3,770 lines).
- Miller cached roots per session and rebound only on `list_changed` (`WorkspaceBindingService`).
  It removed roots in `e36a6f6a` because MCP `2026-07-28` deprecates them and SDK 2.x warns
  `MCP9005`. Its cross-workspace dogfood found that agents pass a root path, not an id.
- Wrong roots got indexed in both: `$HOME` (roots sent `file://$HOME`), `/tmp`, plugin caches,
  unexpanded `${workspaceFolder}`, a third-party tool folder (4.3 GB). Codex and Antigravity
  answered `roots/list` with `[]` (Julie `docs/findings/2026-09-11-deployment-story.md`).
- Silent failures: Julie returned "No symbols found" when a foreign index was missing or its
  lookup failed (review findings #13, #14), and read a database and a root in separate steps
  (#10).
- Miller's first cross-workspace reads refreshed before answering: p95 over 20 s. `5ca324bf`
  answers from the existing index and refreshes in the background. Its answers start with a
  `workspace:` line.
- Miller's central registry drifted to 56% dead rows, most of them removed worktrees. Per-repo
  `.code-kb/` avoids a registry.

Rules kept from this: run the root safety check on every call; resolve the root and index once
per call; a missing or failed index is an error that names the root, never empty results; do not
block long on a new index; no registry.

## Done in 2.0.2

Commit `2d41604` fixed the foreign-extractor bug found during this research: `.tools/` is read
only next to the code-kb executable, an older extractor never rebuilds a newer index, and a
rebuild replaces the index only after its scan succeeds.

## Design

1. **Schema.** Every tool except `telemetry_summary` gets `project_root` (string) in
   `required`. Description: "Absolute path of the project or git worktree you are working in.
   Send the same value on every call. Change it when you move to a worktree or another
   project." `path` and `file_path` keep their meaning: relative to `project_root`, or absolute
   inside it. `workspace` and `root` are accepted as silent aliases (invariant 5) and never
   appear in a schema.
2. **Resolution, once per call.** Accept a plain path or a `file://` URI. A relative value is an
   error. Canonicalize it and walk up with `Workspace::find_workspace_root`, so a subfolder of
   the project works. Refuse `/`, the home directory, and a folder where `is_project_root` is
   false and no `.code-kb/artifact.db` exists. Every refusal names the path and the reason and
   creates nothing.
3. **Active index.** The server keeps one active root with the current `bind_workspace` state
   (index path, watcher, prepare thread). A call whose root differs from the active root
   switches with `bind_workspace`. Each call names its own root, so a switch never changes the
   answer to a later call.
   `ponytail:` one watcher; alternating roots restart the watcher and the offline reconcile on
   each switch. Keep a small per-root cache only if telemetry shows agents alternate often.
4. **Missing or stale index.** `bind_workspace` already starts `spawn_index_prepare` (a worktree
   copies its parent index and reconciles; any other project runs a full scan). The call waits
   up to 5 s, polling `JoinHandle::is_finished`. When the scan is not done, the call returns a
   normal answer: "Indexing <root> started; call again in a few seconds." The next call for that
   root checks again. A failed scan returns an error that names the root, never empty results.
   Fallback if this confuses agents: a `manage_workspace` tool that indexes on request.
5. **Paths outside the root.** An absolute `path` or `file_path` that is not inside
   `project_root` is an error that names both paths. Path arguments never switch the root.
6. **Delete.** Root binding from `initialize` (`roots`, `rootUri`, `rootPath`,
   `workspaceFolders`), the path-inspection rebind at the top of `handle_call_tool_inner`, and
   the "configure `--root`" not-found text. `--root` and the process directory stay only as the
   startup pre-warm root and as the CLI default.
7. **`--db`.** The pinned file belongs to the launch root only. Every other root uses its own
   `.code-kb/artifact.db`. This fixes problem 3.
8. **Telemetry.** `workspace_root` records the call's resolved root (already true after a
   switch). A refused call records the launch root.
9. **CLI 1:1.** `project_root` maps to the existing global `--root`, which defaults to the
   current directory. No CLI change.

## Docs and invariants

- AGENTS.md and CLAUDE.md (one commit, byte-for-byte): rewrite invariant 1 as "every tool
  except `telemetry_summary` requires `project_root`; no schema exposes `workspace`,
  `workspace_id`, `repo_path`, or `root_dir`", and drop its binding list. Update invariant 5
  (scoped search and aliases) and invariant 6 (automatic initial scan) to match.
- Server instructions, the `code-kb hook` routing block, `skills/code-kb/SKILL.md`, and README:
  one rule, "pass the absolute path of the project or worktree you work in as `project_root` on
  every call". Remove the `1f3f87a` worktree workaround text.

## Tests (`crates/code-kb-cli/tests/mcp_test.rs`, core unit tests in `workspace.rs`)

- `tools/list`: every tool except `telemetry_summary` has `project_root` in `required`; no tool
  exposes `workspace`, `workspace_id`, `repo_path`, or `root_dir`.
- A call without `project_root`, or with a relative one, returns an error that asks for the
  absolute project path.
- A subfolder as `project_root` answers from the enclosing project.
- Main root, then worktree root, then main root: the worktree-only symbol is found only in the
  worktree call.
- A project with no index and a zero wait (test-only `CODE_KB_INDEX_WAIT_MS=0`) returns the
  "Indexing ... started" answer; a later call answers from the new index.
- `/` and the home directory are refused, and no `.code-kb` folder is created.
- An absolute `file_path` outside `project_root` is an error that names both paths.
- With `--db`, a call for another project leaves the pinned file unchanged.
- A `symbol_id` from `lookup_symbol` resolves in `get_symbol_body` with the same `project_root`.
- `telemetry_summary` works without `project_root`.

## Acceptance test (black box)

Same setup as the 2.0.2 baseline: `~/source/kb-e2e/flask-base`, fresh sessions in Herdr, same
prompts, no hint about code-kb. Runs: Claude x2, Codex x1, Grok `-w` x1. Pass: every code-kb
telemetry row after the worktree step has the worktree as `workspace_root`, and no call fails
for a missing `project_root` after the first call. Reset the fixture to a clean `main` after
each run.

## Out of scope

- A `manage_workspace` tool (the fallback in design step 4).
- A watcher per root.
- Roots through protocol `2026-07-28`; no longer needed.
