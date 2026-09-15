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
- SQLite in WAL mode is the query engine; `code-kb` keeps retained memory < 15 MB.
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
  1. *Per-project `.mcp.json`:* The host IDE sets CWD to the workspace root.
  2. *MCP Protocol Handshake:* The server extracts roots from `initialize`
     (`params.roots`, `rootUri`, `rootPath`, `workspaceFolders`).
  3. *Path Inspection:* If an absolute path is passed in `file_path` or `path`,
     `code-kb` silently binds to the enclosing repository root.
  4. *Automatic Initial Scan:* If bound to a repository where `.code-kb/artifact.db`
     does not exist yet, `code-kb` runs `scan_workspace` automatically on the
     first tool call rather than returning an error.
  5. *Internal Compatibility:* If an unadvertised `workspace` argument is provided
     internally, the backend accepts it silently, but **never** documents it in
     `input_schema` or prompts for it in error messages.
- **Enforcement:** `crates/code-kb-cli/tests/mcp_test.rs` validates that no tool in
  `tools/list` exposes a `workspace` property. Any PR adding `workspace` to a tool
  schema will fail CI.

### 2. Zero In-Memory Heap Objects for Repositories
- Do not hydrate repository symbol graphs or file lists into RAM.
- Retained memory must stay < 15 MB.
- All symbol searches, skeleton rendering, context slices, and reference lookups
  are executed as direct, indexed SQLite queries with `open_read_only`.

### 3. Single-Turn Atomic Edits
- `replace_symbol_body` must perform pre-flight tree-sitter syntax validation,
  optional `body_hash` concurrency verification, atomic file replacement, and
  immediate SQLite re-indexing in a single turn.
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
  `symbol`/`name` for `symbol_name`, `body`/`code` for `new_body`, `q`/`name` for `query`).
- Optional parameters provide safe defaults (`direction` in `find_references` defaults to
  `"callers"`, `category` in `find_structural_facts` lists all categories with counts when omitted).
- Scoped search: `lookup_symbol`, `search_symbols`, `find_references`, and `find_structural_facts` support an optional `path`/`file_path` filter.
- Language-agnostic callee filtering: `find_references(direction="callees")` and `get_symbol_context`
  filter unresolved AST tokens against workspace symbols, eliminating external stdlib/runtime noise
  across all ~40 supported languages by default (`include_external: true` / `--include-external` restores them).
- Blast radius & test prediction: `blast_radius` (alias: `impact`, CLI: `code-kb blast-radius` / `impact`)
  computes multi-hop reverse reachability via SQLite recursive CTEs and predicts targeted tests to run.
  Auto-discovers uncommitted git changes when no target is passed.
- Continuous testing boundary: Execution stays in native agent terminal commands (`cargo test`, `pytest`,
  `npm test`), while `code-kb` predicts the minimal set of targeted test targets to run before/after edits.
- Self-cleaning workspaces: Every repository and git worktree maintains its own isolated database at
  `<root>/.code-kb/artifact.db`. Deleting a repository directory or running `git worktree remove`
  automatically cleans up the AST index database with no orphaned external state. Durable tool
  telemetry and token efficiency history are preserved centrally at `~/.code-kb/telemetry.db`.
- Cross-platform agent hooks: `code-kb hook [SessionStart|SubagentStart|PreInvocation]` outputs agent routing instructions
  directly from the native binary without external runtime dependencies (Node.js, bash, python), formatting JSON
  natively for Claude Code/Cursor (`SessionStart`), Copilot, and Antigravity (`PreInvocation` injectSteps).

### 7. Pinned Extractor & Bundled Distribution
- `code-kb` pins the exact extractor version in `scripts/julie-pins.json` (currently `2.43.0`).
- Build guard: `crates/code-kb-cli/build.rs` verifies that `julie-extract` is restored and matches the pinned version. A missing or mismatched extractor fails the build immediately (bypassable for offline packaging via `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1`).
- Single-download distribution: Release archives ship `code-kb` and matching `julie-extract` pre-packaged side-by-side. Users download one archive and receive both binaries ready to execute.
- Runtime discovery: `code-kb` checks `JULIE_EXTRACT_BIN`, next to its own executable (`current_exe().parent()`), `.tools/julie-extract`, and `PATH`. The first candidate whose version matches the pin wins; otherwise the first candidate found is used with a warning.
- Index version guard: the index is rebuilt once when it was written by a different `julie-extract` than the one in use. The guard compares against the binary in use, never against the pin alone, so a non-pinned extractor never causes repeated rebuilds.
- New artifacts are built at the extractor's `facts` level (symbol core plus structural facts, no identifier, literal, or source-region tables). The version guard also rebuilds an index recorded at another level.
- Scans pass `--parent-pid` (Unix) so an extractor scan aborts when the `code-kb` process that started it dies.
- Release workflow: Documented step-by-step in `docs/RELEASING.md`; automated pre-flight check via `scripts/release-preflight.sh`.

### 8. Dynamic MCP Discovery & Zero Ghost Compatibility
- **An MCP server is an ephemeral, agent-facing discovery surface, not a frozen REST API.**
- Agents discover tools dynamically via `tools/list` on session start and carry zero state across sessions. There is no concept of "backward compatibility" for tool names across sessions.
- Do not freeze suboptimal tool names, preserve dead aliases, or compromise ergonomics for "backward compatibility" when improving tool schemas.
- Optimize ruthlessly for agent cognitive clarity and minimal tool-selection ambiguity. When a tool name or boundary causes model friction, rename or sharpen it cleanly.
