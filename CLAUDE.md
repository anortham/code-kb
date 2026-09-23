# code-kb Agent Guidelines

## Sync Contract

`AGENTS.md` and `CLAUDE.md` must stay byte-for-byte equivalent except for
future tool-specific sections that are explicitly labeled. When one changes,
update the other in the same commit.

## Product Boundary

`code-kb` is an agent-facing code-intelligence engine and Model Context Protocol
(MCP) server designed to give AI coding agents progressive disclosure, semantic
symbol search, and surgical context slicing with minimal token consumption.

- Backed by AST facts extracted by `julie-extractors` (SQLite schema v7).
- SQLite in WAL mode is the query engine; `code-kb` holds no repository data in RAM.
- All MCP tools have an exact 1:1 CLI command for direct terminal verification.
- Non-goals: Do not add web dashboards, GPU embedding runtimes, daemon watchers
  outside the 1:1 MCP session, or workspace registries.

---

## Core Invariants

### 1. Required `project_root` on Every Tool
**Every MCP tool except `telemetry_summary` requires `project_root`. No MCP tool
schema exposes `workspace`, `workspace_id`, `repo_path`, or `root_dir`.**

- `project_root` is the absolute path of the project or git worktree the agent works
  in. The schema description is: "Absolute path of the project or git worktree you are
  working in. Send the same value on every call. Change it when you move to a worktree
  or another project."
- **Why:**
  - The agent always knows where it works. In fresh sessions, the first code-kb call
    after the worktree step already passed an absolute worktree path.
  - MCP roots cannot carry the answer. Codex and Grok send no roots, Antigravity sends
    `[]`, and `grok -w` starts the server in the main checkout. MCP `2026-07-28`
    deprecates roots and says to pass directories in tool parameters.
  - A root held in server state goes stale (issue #3). A root in every call cannot go
    stale inside the server.
- **Resolution, once per call:**
  - A plain path or a `file://` URI is accepted. A relative value is an error.
  - A subfolder or a file inside the project resolves to the enclosing project. The walk
    up returns the nearest folder with `.git` or an existing `.code-kb/artifact.db`. When
    there is no such folder, the walk returns the nearest folder with a language marker
    (`Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`). When the folder it finds is
    the home directory or a filesystem root, the walk returns the nearest folder below it
    with a project marker. The walk stops at a folder with an excluded name, such as
    `target`, `node_modules`, or `.claude`, and does not look above it. So a path under such
    a folder resolves to the nearest folder with a marker up to that folder, or the server
    refuses the call.
  - The server refuses a filesystem root (`/`, `C:\`), the home directory, and a folder
    with no project marker (`.git`, `Cargo.toml`, `package.json`, `go.mod`,
    `pyproject.toml`) and no code-kb index. The error names the path and the reason.
    A refused call creates nothing.
  - `workspace` and `root` are silent aliases of `project_root`. They never appear in a
    schema or in an error message.
  - The server never takes the root from the MCP `initialize` request.
- **Path arguments:** `path` and `file_path` are relative to `project_root`, or absolute
  inside it. An absolute `path` or `file_path` outside `project_root` is an error that
  names both paths. A relative or absolute path inside a nested git worktree or submodule
  counts as outside `project_root`. The error names that nested root. Path arguments never switch
  the project.
- **Active index:** The server keeps one active index. A call for another root switches
  to that root's `<root>/.code-kb/artifact.db`. Each call names its own root, so a switch
  never changes the answer to a later call.
- **Startup pre-warm:** `code-kb serve --root <path>`, or the process directory when
  `--root` is absent, is the startup pre-warm root. The server prepares that project's
  index at start. `--root` is optional for every client, including GUI apps.
  `--db <file>` pins the index file of the launch root only.
- **Automatic index creation:** A root with no `.code-kb/artifact.db` gets an index at
  startup or on the first call for it (`create_index`: a git worktree copies and
  reconciles its parent repository's index, anything else runs a full scan). Files
  changed while no server ran are reconciled at startup. A failed scan is an error that
  names the root, never empty results. CLI commands reconcile before answering.
- **Telemetry:** `workspace_root` records the call's resolved root. A refused call
  records the launch root.
- **CLI 1:1:** `project_root` maps to the global `--root` flag, which defaults to the
  current directory. The CLI refuses a home directory or a filesystem root the same way the
  tools do, and creates a missing index only for a root the tools accept. `code-kb scan`
  runs on any root you give it.
- **Enforcement:** The `tools/list` tests in `crates/code-kb-cli/tests/mcp_test.rs` and
  the invariant tests in `crates/code-kb-cli/tests/adversarial_m2_server_test.rs` and
  `crates/code-kb-cli/tests/adversarial_m3_server_test.rs` check every tool schema. A PR
  that adds a banned name to a schema, or drops `project_root` from `required`, fails CI.

### 2. Zero In-Memory Heap Objects for Repositories
- Do not hydrate repository symbol graphs or file lists into RAM.
- Retained memory is about 25 MB today. Measure a live `serve` process before and after any change that could raise it.
- All symbol searches, skeleton rendering, context slices, and reference lookups
  are executed as direct, indexed SQLite queries with `open_read_only`.
- The search index is two FTS5 tables over `symbols`: `symbols_fts` for words and `symbol_names_tri`
  for name substrings. `ensure_fts_index` migrates both in one transaction, guarded by the `fts_rule`
  marker; a failed migration is reported by the first tool call and retried on the next start.

### 3. Token-Dense Progressive Disclosure
- Always return the most compact representation that answers the query.
- Strip implementation bodies in `file_skeleton`.
- Include only immediate callee signatures, related types, and test
  locations in `get_symbol_context`.

### 4. Windows Compatibility
- Windows is a first-class target. Every release ships a Windows binary.
- Strip Windows verbatim prefixes (`\\?\C:\...`) using `dunce::simplified`.
- Contract relative paths and JSON outputs must use explicit forward slashes `/`,
  never platform-dependent separators.
- SQLite connections and file handles must be closed before file rename or deletion.
- Verify path identity using canonical paths and `dunce::simplified`, not by raw case-sensitive string matching.

### 5. Zero-Friction Tool Ergonomics
- Tool handlers accept intuitive parameter aliases (`file`/`path` for `file_path`,
  `symbol`/`name` for `symbol_name`, `q`/`name` for `query`).
- `workspace` and `root` are silent aliases of `project_root`. No schema names them.
- Optional parameters provide safe defaults (`direction` in `find_references` defaults to
  `"callers"`, `category` in `find_structural_facts` lists all categories with counts when omitted).
- Scoped search: `lookup_symbol`, `search_symbols`, `find_references`, and `find_structural_facts` support an optional `path`/`file_path` filter. The filter is relative to `project_root`, or absolute inside it.
- Lookup and search text include `id=<symbol_id>`. `get_symbol_body`, `get_symbol_context`, `find_references`, and `blast_radius` accept that current-index ID as `symbol_id` / `--symbol-id`; IDs are reselected after edits or rebuilds, and unresolved-call matching remains heuristic.
- Recovery on a miss: every not-found path builds its text from `symbol_not_found_parts` or
  `file_not_found_parts` in `queries.rs`. The text names the bound workspace, then either
  `Did you mean one of:` with up to three candidates as `kind `name` (path:line)`, or
  `No similar name is indexed; check the workspace and spelling.` Candidates come from the
  substring search first, then from `symbol_names_tri` trigram rows kept within an edit distance
  of the query. File candidates match by basename, then by stem, then by the last two segments.
- Structural-fact categories: `CATEGORY_ALIASES` in `queries.rs` is the whole alias table
  (`sql`/`query`/`queries`, `route`/`routes`, `config`, `model`/`models`, `signal`/`signals`,
  `import`/`imports`, `binding`/`bindings`, `component`/`components`, `module`/`modules`,
  `pragma`, `property`/`properties`), and each alias maps to the pattern-id families it names,
  never to a substring. An unknown category still falls back to a
  substring match, so raw pattern ids work. With no category the answer lists the aliases with
  facts in this index before the raw pattern list. Qt C++ property facts expose available property
  metadata, including optional `designable`, `scriptable`, `stored`, `user`, and `revision` attributes.
  Facts and literals have separate limits, each
  with its own cap notice.
- Search ranking: `search_symbols` admits rows from three branches (exact name, FTS5 word match, trigram name substring, so `sha256` finds `parseSha256Sidecar`), then a deterministic Rust rerank in `queries.rs` credits each query term once from its strongest field (name whole token 3, name stem 2, name substring 1, signature or docstring 1), weighted by the term's rarity across the index (a capped FTS5 match count per term), and adds the whole-name and all-words bonuses (the whole-name bonus is 100 for definition kinds and 60 for every other kind) and the kind, path, documentation, and test priors; `score` is that rerank score. Rows from test files are hidden by default: a path rule in `queries.rs` (`is_test_path` and its SQL mirror `test_path_predicate`) hides the whole file, not only the symbols `julie-extract` flags, and `is_test: true` / `--include-tests` shows them again. A `lookup_symbol` row whose name equals the query is shown either way. Equal scores break by name strength, the sum over query words of 3 for a whole-token name match, 2 for a stem match, 1 for a substring match, and 0 for none, then names that do not start with `_` before names that do; `--explain` reports the name strength as `name_strength`. `code-kb search --explain` prints the breakdown and the rerank timer; the MCP tool takes no `explain` parameter, and `--verbose` stays debug logging.
- Reference rules: a member access whose receiver names a type-like target groups to one row per
  file with an `occurrences` count. An identifier row is dropped when a relationship row already
  covers the same site. An import alias satisfies a pending receiver unless the import source
  starts with `Qt`. An `extends` row never resolves to its own component. A handler row is
  labelled `handler` when the receiver names the owner and `handler (candidate)` otherwise. Test
  paths include `/autotests/` directories and file names that start with `tst_`. A Qt C++
  header's `property` and `event` rows come from the extractor's macro pre-pass and render like
  any other member; a skeleton `event` row whose signature does not spell `signal` or `event`
  carries `// event` before its line range.
- Language-agnostic callee filtering: `find_references(direction="callees")` and `get_symbol_context`
  filter unresolved AST tokens against workspace symbols, eliminating external stdlib/runtime noise
  across supported languages by default (`include_external: true` / `--include-external` restores them).
- Blast radius & test prediction: `blast_radius` (alias: `impact`, CLI: `code-kb blast-radius` / `impact`)
  computes multi-hop reverse reachability via SQLite recursive CTEs and predicts targeted tests to run.
  Auto-discovers uncommitted git changes when no target is passed.
- Continuous testing and editing stay in native agent terminal commands. `code-kb` predicts the
  targeted test targets to run before or after a change; filesystem changes automatically refresh indexed files.
- Self-cleaning workspaces: Every repository and git worktree maintains its own isolated database at
  `<root>/.code-kb/artifact.db`, and `code-kb` writes a `.gitignore` with `*` inside that directory, so no project `.gitignore` edit is needed. Deleting a repository directory or running `git worktree remove`
  automatically cleans up the AST index database with no orphaned external state. Durable tool
  telemetry and token efficiency history are preserved centrally at `~/.code-kb/telemetry.db`.
- Cross-platform agent hooks: `code-kb hook [SessionStart|SubagentStart|PreInvocation]` outputs agent routing instructions
  directly from the native binary without external runtime dependencies (Node.js, bash, python), formatting JSON
  natively for Claude Code/Cursor (`SessionStart`), Copilot, and Antigravity (`PreInvocation` injectSteps).

### 6. Index Freshness
- A debounced watcher refreshes filesystem changes, startup reconciles changes made while no server ran,
  and body and skeleton reads refresh their target file before answering.
- Only the active root keeps a watcher; a root the server switched away from loses its watcher by
  the next call. When a call switches roots, the watcher and the offline
  reconcile restart for the new root.
- A call waits up to 5 s for its root's index. If the index is not ready, the answer is
  `Indexing <root> started; call again in a few seconds.` The next call for that root checks again.
- Until upstream header updates preserve C++ language detection, a batch of changed `.h` files triggers a
  content-aware workspace scan that only re-extracts changed files.

### 7. Pinned Extractor & Bundled Distribution
- `code-kb` pins the exact extractor version in `scripts/julie-pins.json` (currently `3.5.0`).
- Build guard: `crates/code-kb-cli/build.rs` verifies that `julie-extract` is restored and matches the pinned version. A missing or mismatched extractor fails the build immediately (bypassable for offline packaging via `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1`).
- Single-download distribution: Release archives ship `code-kb` and matching `julie-extract` pre-packaged side-by-side. Users download one archive and receive both binaries ready to execute.
- Runtime discovery: `code-kb` checks `JULIE_EXTRACT_BIN`, next to its own executable (`current_exe().parent()`), `.tools/julie-extract` in that directory or any directory above it, and `PATH`. The current directory is never searched, so another repository's vendored extractor is never run. The first candidate whose version matches the pin wins; otherwise the first candidate found is used with a warning.
- Index version guard: the index is rebuilt once when it was written by a different `julie-extract` than the one in use. The guard compares against the binary in use, never against the pin alone, so a non-pinned extractor never causes repeated rebuilds. An index that a newer `julie-extract` wrote (artifact metadata or any file row) is kept rather than rebuilt by an older binary. A rebuild scans into `artifact.db.rebuild` and replaces the index only after the scan succeeds.
- New artifacts are built at the extractor's `facts` level (symbol core, structural facts, literals, and type-usage and member-access identifiers; no call or variable-reference identifiers and no source regions). `find_references(direction="callers")` reads those identifiers. The version guard also rebuilds an index recorded at another level.
- Scans pass `--parent-pid` (Unix) so an extractor scan aborts when the `code-kb` process that started it dies.
- Plugin distribution: every harness plugin (`.claude-plugin/plugin.json`, `.codex-plugin/plugin.json` with `.mcp.json`, root `plugin.json` with `mcp.json`, `mcp_config.json`, and `hooks.json` for Antigravity) starts the server and hooks through `bin/code-kb-launcher.cjs`. The launcher is a dependency-free Node script: on first run it downloads the release archive for the plugin's version from GitHub Releases into `~/.code-kb/dist/<version>/<target>/`, verifies the `.sha256` sidecar, unpacks it, and then executes `code-kb` with the given arguments. `CODE_KB_BIN` names a binary to run instead; a binary or symlink at `~/.code-kb/bin/code-kb` overrides the download in every harness (Codex clones plugins into a cache and drops the environment of MCP servers, so only a file can reach it); and a plugin installed from a source checkout runs that checkout's `target/release/code-kb` when it exists. With one of these, `cargo build --release` plus a session restart is the whole dev loop. No binaries are committed to git. `tests/plugin/*.test.cjs` (run with `node --test`) cover the launcher and keep every manifest on the launcher and on one version.
- Release workflow: Documented step-by-step in `docs/RELEASING.md`; automated pre-flight check via `scripts/release-preflight.sh`. The launcher fetches by the version in `.claude-plugin/plugin.json`, so a version bump on `main` must be followed by its tag and release before users install from `main`.

### 8. Dynamic MCP Discovery & Zero Ghost Compatibility
- **An MCP server is an ephemeral, agent-facing discovery surface, not a frozen REST API.**
- Agents discover tools dynamically via `tools/list` on session start and carry zero state across sessions. There is no concept of "backward compatibility" for tool names across sessions.
- Do not freeze suboptimal tool names, preserve dead aliases, or compromise ergonomics for "backward compatibility" when improving tool schemas.
- Optimize ruthlessly for agent cognitive clarity and minimal tool-selection ambiguity. When a tool name or boundary causes model friction, rename or sharpen it cleanly.
