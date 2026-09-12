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
- Include only immediate caller/callee signatures, related types, and test
  locations in `get_context_slice`.

### 5. Windows Compatibility
- Windows is a first-class target. Every release ships a Windows binary.
- Strip Windows verbatim prefixes (`\\?\C:\...`) using `dunce::simplified`.
- Contract relative paths and JSON outputs must use explicit forward slashes `/`,
  never platform-dependent separators.
- SQLite connections and file handles must be closed before file rename or deletion.
- Verify path identity by handle (`same-file`), not by case-sensitive string matching.
