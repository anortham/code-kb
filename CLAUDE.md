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

### 1. Zero Workspace Parameters in Tool Schemas
**NEVER expose `workspace`, `workspace_id`, `repo_path`, or `root_dir` in any
MCP tool schema.**

- **Why:** Exposing a `workspace` parameter pollutes the LLM's prompt. Models feel
  obligated to inject workspace paths into every tool call, causing context burn,
  slash mismatches, and hallucinated paths. Previous projects (Miller, Goldfish)
  suffered severe usability penalties from this anti-pattern.
- **How Workspace Binding Actually Works:**
  1. *Explicit `--root`:* `code-kb serve --root <path>` in the MCP config binds
     that path. This is the documented path for GUI apps (Cursor, Windsurf, the
     Antigravity IDE, Visual Studio, Claude Desktop), which start the server from
     their own install directory. The config lives inside the project.
  2. *Process CWD:* Terminal harnesses (Claude Code, Codex, AGY, Grok CLI) start
     the server in the project directory, so `code-kb serve` alone binds it.
  3. *MCP Protocol Handshake:* The server extracts roots from `initialize`
     (`params.roots`, `rootUri`, `rootPath`, `workspaceFolders`).
  4. *Path Inspection:* If an absolute path is passed in `file_path` or `path`,
     `code-kb` silently binds to the enclosing repository root.
  5. *Automatic Initial Scan:* If bound to a repository where `.code-kb/artifact.db`
     does not exist yet, `code-kb` creates the index in the background at server
     start, or before a CLI command answers, rather than returning an error
     (`create_index`: a git worktree copies and reconciles its parent repository's
     index, anything else runs a full scan). Files changed while no server ran are
     reconciled at startup. The first tool call waits for that scan or
     reconciliation and reports a failed scan; CLI commands reconcile before answering.
  6. *Internal Compatibility:* If an unadvertised `workspace` argument is provided
     internally, the backend accepts it silently, but **never** documents it in
     `input_schema` or prompts for it in error messages.
- **Enforcement:** `crates/code-kb-cli/tests/mcp_test.rs` validates that no tool in
  `tools/list` exposes a `workspace` property. Any PR adding `workspace` to a tool
  schema will fail CI.

### 2. Zero In-Memory Heap Objects for Repositories
- Do not hydrate repository symbol graphs or file lists into RAM.
- Retained memory is about 25 MB today. Measure a live `serve` process before and after any change that could raise it.
- All symbol searches, skeleton rendering, context slices, and reference lookups
  are executed as direct, indexed SQLite queries with `open_read_only`.
- The search index is two FTS5 tables over `symbols`: `symbols_fts` for words and `symbol_names_tri`
  for name substrings. `ensure_fts_index` migrates both in one transaction, guarded by the `fts_rule`
  marker; a failed migration is reported by the first tool call and retried on the next start.

### 3. Single-Turn Atomic Edits
- Two edit tools share one write path (`commit_file_edit` in `edit.rs`): pre-flight syntax
  validation through `julie-extract check` (code-kb bundles no tree-sitter grammars of its own),
  concurrency verification, atomic file replacement, immediate SQLite re-indexing, and rollback
  when the re-index fails. Both finish in a single turn.
- `edit_file` (CLI: `code-kb edit-file`) replaces text in any file without reading it first.
  It matches in two tiers and no more: exact substring, then line by line with each line trimmed,
  so a different indentation still matches. There is no edit-distance matching. A match that
  occurs more than once, overlapping matches included, is refused with up to ten matching line
  numbers and a count of the rest unless `occurrence` is `first`, `last`, or `all`; `only` is
  the default, and `all` replaces the non-overlapping matches. The result file is capped at the
  same 8 MiB as the input, and only the first line of a failed edit's error reaches telemetry,
  never the quoted file lines.
- Until upstream header updates preserve C++ language detection, `.h` refreshes and edits trigger a full workspace re-extraction.
- `replace_symbol_body` keeps the optional `body_hash` check and replaces a whole symbol body.
- Do not implement two-step preview-and-confirm handshakes that waste agent turns.

### 4. Token-Dense Progressive Disclosure
- Always return the most compact representation that answers the query.
- Strip implementation bodies in `file_skeleton`.
- Include only immediate callee signatures, related types, and test
  locations in `get_symbol_context`.

### 5. Windows Compatibility
- Windows is a first-class target. Every release ships a Windows binary.
- Strip Windows verbatim prefixes (`\\?\C:\...`) using `dunce::simplified`.
- Contract relative paths and JSON outputs must use explicit forward slashes `/`,
  never platform-dependent separators.
- SQLite connections and file handles must be closed before file rename or deletion.
- Verify path identity using canonical paths and `dunce::simplified`, not by raw case-sensitive string matching.

### 6. Zero-Friction Tool Ergonomics
- Tool handlers accept intuitive parameter aliases (`file`/`path` for `file_path`,
  `symbol`/`name` for `symbol_name`, `body`/`code` for `new_body`, `q`/`name` for `query`,
  `old`/`find` for `old_text`, `new`/`replace` for `new_text`).
- Optional parameters provide safe defaults (`direction` in `find_references` defaults to
  `"callers"`, `category` in `find_structural_facts` lists all categories with counts when omitted).
- Scoped search: `lookup_symbol`, `search_symbols`, `find_references`, and `find_structural_facts` support an optional `path`/`file_path` filter.
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
  across all ~40 supported languages by default (`include_external: true` / `--include-external` restores them).
- Blast radius & test prediction: `blast_radius` (alias: `impact`, CLI: `code-kb blast-radius` / `impact`)
  computes multi-hop reverse reachability via SQLite recursive CTEs and predicts targeted tests to run.
  Auto-discovers uncommitted git changes when no target is passed.
- Continuous testing boundary: Execution stays in native agent terminal commands (`cargo test`, `pytest`,
  `npm test`), while `code-kb` predicts the minimal set of targeted test targets to run before/after edits.
- Self-cleaning workspaces: Every repository and git worktree maintains its own isolated database at
  `<root>/.code-kb/artifact.db`, and `code-kb` writes a `.gitignore` with `*` inside that directory, so no project `.gitignore` edit is needed. Deleting a repository directory or running `git worktree remove`
  automatically cleans up the AST index database with no orphaned external state. Durable tool
  telemetry and token efficiency history are preserved centrally at `~/.code-kb/telemetry.db`.
- Cross-platform agent hooks: `code-kb hook [SessionStart|SubagentStart|PreInvocation]` outputs agent routing instructions
  directly from the native binary without external runtime dependencies (Node.js, bash, python), formatting JSON
  natively for Claude Code/Cursor (`SessionStart`), Copilot, and Antigravity (`PreInvocation` injectSteps).

### 7. Pinned Extractor & Bundled Distribution
- `code-kb` pins the exact extractor version in `scripts/julie-pins.json` (currently `3.3.1`).
- Build guard: `crates/code-kb-cli/build.rs` verifies that `julie-extract` is restored and matches the pinned version. A missing or mismatched extractor fails the build immediately (bypassable for offline packaging via `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1`).
- Single-download distribution: Release archives ship `code-kb` and matching `julie-extract` pre-packaged side-by-side. Users download one archive and receive both binaries ready to execute.
- Runtime discovery: `code-kb` checks `JULIE_EXTRACT_BIN`, next to its own executable (`current_exe().parent()`), `.tools/julie-extract`, and `PATH`. The first candidate whose version matches the pin wins; otherwise the first candidate found is used with a warning.
- Index version guard: the index is rebuilt once when it was written by a different `julie-extract` than the one in use. The guard compares against the binary in use, never against the pin alone, so a non-pinned extractor never causes repeated rebuilds.
- New artifacts are built at the extractor's `facts` level (symbol core, structural facts, literals, and type-usage and member-access identifiers; no call or variable-reference identifiers and no source regions). `find_references(direction="callers")` reads those identifiers. The version guard also rebuilds an index recorded at another level.
- Scans pass `--parent-pid` (Unix) so an extractor scan aborts when the `code-kb` process that started it dies.
- Plugin distribution: every harness plugin (`.claude-plugin/plugin.json`, `.codex-plugin/plugin.json` with `.mcp.json`, root `plugin.json` with `mcp.json`, `mcp_config.json`, and `hooks.json` for Antigravity) starts the server and hooks through `bin/code-kb-launcher.cjs`. The launcher is a dependency-free Node script: on first run it downloads the release archive for the plugin's version from GitHub Releases into `~/.code-kb/dist/<version>/<target>/`, verifies the `.sha256` sidecar, unpacks it, and then executes `code-kb` with the given arguments. `CODE_KB_BIN` names a binary to run instead; a binary or symlink at `~/.code-kb/bin/code-kb` overrides the download in every harness (Codex clones plugins into a cache and drops the environment of MCP servers, so only a file can reach it); and a plugin installed from a source checkout runs that checkout's `target/release/code-kb` when it exists. With one of these, `cargo build --release` plus a session restart is the whole dev loop. No binaries are committed to git. `tests/plugin/*.test.cjs` (run with `node --test`) cover the launcher and keep every manifest on the launcher and on one version.
- Release workflow: Documented step-by-step in `docs/RELEASING.md`; automated pre-flight check via `scripts/release-preflight.sh`. The launcher fetches by the version in `.claude-plugin/plugin.json`, so a version bump on `main` must be followed by its tag and release before users install from `main`.

### 8. Dynamic MCP Discovery & Zero Ghost Compatibility
- **An MCP server is an ephemeral, agent-facing discovery surface, not a frozen REST API.**
- Agents discover tools dynamically via `tools/list` on session start and carry zero state across sessions. There is no concept of "backward compatibility" for tool names across sessions.
- Do not freeze suboptimal tool names, preserve dead aliases, or compromise ergonomics for "backward compatibility" when improving tool schemas.
- Optimize ruthlessly for agent cognitive clarity and minimal tool-selection ambiguity. When a tool name or boundary causes model friction, rename or sharpen it cleanly.
