# Session root and cross-repository queries (issue #3)

## Problem

1. A running `code-kb serve` stays bound to the launch root after Claude Code `EnterWorktree`.
   Unscoped calls (`lookup_symbol`, `search_symbols`, `codebase_outline`, ...) answer from the
   launch-root index with no warning. Commit `1f3f87a` only documents a workaround.
2. A user in project A asks the agent to explore project B (a sibling dependency such as
   `julie-extractors`, or an unrelated repository). Today an absolute path rebinds the whole
   server to B and the binding stays there: later unscoped calls, and calls from subagents that
   share the server, answer from B. Results print B-relative paths with no root name.
3. Existing bug: with `serve --db <file>`, `locate_db` always returns that file
   (`workspace.rs` `locate_db`), so a path rebind runs `ensure_index_matches_extractor` and
   `reconcile_offline_edits` for repository B against A's index file.

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

Rules taken from this: send `roots/list` only inside a tool call; empty or failed roots keep the
current root; run the root safety check on every source (roots, paths, cwd); resolve the
target root and database once per call; a missing or failed foreign index is a typed error,
never empty results; answer from the existing index and reconcile after; no registry.

## Found while researching: foreign repositories break the extractor choice

A code-kb CLI call run from inside `~/source/miller` deleted Miller's index on 2026-09-23:

- `julie_extract_candidates` (`sync.rs`) walks the current directory upward for
  `.tools/julie-extract`, before `PATH`. When no candidate matches the pin, the first one wins.
  From Miller's directory that was Miller's own extractor 2.42.0.
- `ensure_index_matches_extractor` then deleted the index (`remove_artifact_files`) before
  `scan_workspace` ran, and the scan failed (`--level facts` unknown to 2.42.0).

Per-call cross-repository queries make this more likely. Required fixes: take `.tools/` only
next to the code-kb executable, never from the current or target directory; never rebuild an
index with an extractor older than the one that wrote it; build the replacement index in a
temporary file and rename it only after the scan succeeds.

No single consensus exists. The spec direction is directories in tool parameters or server
configuration. code-kb already takes paths in every tool that needs a scope, so the design
below uses those parameters and adds no `workspace` field.

## Design

### Session root

The session root is the default for every call that has no absolute path. It comes from, in
order: `--root` (pinned; roots are then ignored), `roots/list` when the client supports it,
then the process directory.

1. Record the client `roots` capability and the negotiated protocol version in `initialize`.
   Poll only for versions that use server-to-client `roots/list` (up to `2025-11-25`).
2. Move stdin reads to a reader thread that sends messages to the main loop over a channel.
   Parse client responses (`result` or `error`, no `method`) instead of answering them with a
   parse error. Use string ids with a `code-kb-` prefix for server requests.
3. Before each `tools/call`, send `roots/list` and wait for the response with that id. Hold
   other client requests that arrive during the wait and answer them in order afterwards. Apply
   `notifications/cancelled` to a held call.
4. Timeout (1 s) or error: stop polling for the rest of the process, keep the current session
   root, and add one line to that answer naming the root used. Never block later calls again.
5. Root choice: canonicalize each `file://` root. An empty list keeps the current root. A
   root that fails the safety check in step 10 is ignored. One root: use it. Several: keep the current
   session root while it is in the list, else take the first and name it in the answer.
6. Call `bind_workspace` only when the canonical session root changes. Keep one watcher, on the
   session root. Join or finish the old prepare thread before starting another.
7. Treat `notifications/roots/list_changed` as a hint only.

### Per-call repository selection

8. An absolute path in `file_path`, `path`, or `file` (and the hidden `workspace` argument)
   whose repository root differs from the session root answers that call from that
   repository's own `.code-kb/artifact.db`. The session root, its watcher, and later calls
   stay unchanged. Relative paths always resolve inside the session root.
9. An answer from another root starts with one `root: <absolute path>` line. A `symbol_id`
   resolves only in the database the call opened, so follow-up calls for that repository pass
   an absolute path under it.
10. Root safety check, for every source (roots, paths, cwd): refuse the filesystem root, the
    home directory, temp and plugin-cache directories, paths with unexpanded `${...}`, and any
    directory `is_project_root` rejects. First use of another root: a git worktree copies its
    parent index; any other repository runs one full scan and the answer says so. Later calls
    answer from the existing index at once and reconcile after. No watcher. A missing or
    failed index returns a typed error that names the root, never empty results. Body and skeleton reads already refresh the file they read. Keep only a
    short list of root paths, no open connections.
11. With `--db`, the pinned file belongs to the session root only. Other roots use their own
    index files. This fixes problem 3.

Cost: one local round trip per tool call for roots clients. No schema change.

## Docs and invariant

- Keep core invariant 1: no tool schema gains `workspace`, `root`, `project`, or `repo_path`.
  Amend its binding list: session root from `--root`, `roots/list`, or the process directory;
  an absolute path in an existing path argument selects a repository for that call only.
- Fix the handshake text in README and AGENTS.md/CLAUDE.md: roots come from `roots/list`.
- Replace the long routing-block rule from `1f3f87a` with one line: to query another
  repository or a worktree the host does not report, pass an absolute path under it in
  `path` or `file_path`; that call answers from that repository.

## Tests (`crates/code-kb-cli/tests/mcp_test.rs`)

- A roots client moves from the main root to a worktree root; an unscoped `lookup_symbol` for a
  worktree-only symbol succeeds.
- A client without roots receives no `roots/list` request.
- A client that never answers gets a result after the timeout, one notice line, and no second
  `roots/list`.
- A request that arrives during the wait is answered in order; a cancelled held call is dropped.
- `--root` pins the session root against a different client root.
- A two-root list that contains the current root keeps it.
- An absolute-path call into repository B does not change the next unscoped call's answer, and
  its answer names B's root.
- With `--db`, an absolute path into B leaves the pinned index unchanged.
- An absolute path at `/` or the home directory builds no index.
- A roots answer of `[]` keeps the current root.
- A `.tools/julie-extract` in the current directory or the target repository is never chosen.
- An index written by a newer extractor is not rebuilt by an older one, and a failed rebuild
  leaves the old index in place.

## Out of scope

- Roots through `InputRequiredResult` (protocol `2026-07-28`); add it when a client negotiates
  that version.
- Claude Code subagents with `isolation: worktree` share the parent's server (Claude Code docs:
  subagents inherit the parent's MCP tools). Their roots follow the parent session, so they use
  absolute paths under their worktree.
- Two long-lived sessions on different repositories should run separate servers (`--root`).
