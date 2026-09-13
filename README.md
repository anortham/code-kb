# code-kb

`code-kb` is a fast, lightweight code-intelligence engine and Model Context Protocol (MCP) server designed specifically for AI coding agents. 

Backed by the rich AST fact tables produced by [`julie-extractors`](https://github.com/anortham/julie-extractors), `code-kb` provides progressive disclosure, semantic symbol navigation, and surgical context slicing—enabling agents to navigate, understand, and edit codebases with **80–90% fewer tokens** without burning context on full file reads or noisy text grep.

---

## Why code-kb?

Traditional AI coding agents burn massive amounts of context loading entire source files into their prompt windows just to inspect a single type signature or implementation detail. 

`code-kb` solves this with a **progressive disclosure** architecture:
1. **Repository Orientation (`codebase_outline`):** Understand directory structures and key exports in ~200 tokens.
2. **File Skeletons (`file_skeleton`):** Inspect function signatures, types, traits, and docstrings with implementation bodies stripped.
3. **Semantic Symbol Search (`find_symbol` / `search_symbols`):** Instant exact, fuzzy, and conceptual FTS5 search across all symbols.
4. **Surgical Context Slices (`get_context_slice`):** In a single turn, fetch a target function's body along with its callee signatures, parameter types, and associated unit tests.
5. **Atomic AST Edits (`replace_symbol_body`):** Replace symbol implementations atomically with tree-sitter pre-flight syntax checks and immediate database re-indexing.

---

## Key Principles

- **Sub-15MB Retained Memory:** Written in Rust, zero heavy runtimes (no Node.js daemon, no web dashboard, no GPU models), retained process memory stays below 15 MB.
- **Sub-5ms Query Latency:** Direct SQLite queries in WAL mode with zero in-memory heap bloat.
- **Zero Workspace Parameters:** Pure semantic tool calling (`find_symbol(query="...")`). The agent is never burdened with `workspace_id`, `repo_path`, or path confusion.
- **CLI-First Parity:** Every MCP tool has an exact 1:1 CLI command for instantaneous terminal verification and dogfooding.
- **Continuous 3-Tier Sync:** Tool-driven updates, JIT staleness guards before reads, and a debounced background watcher with a Git storm circuit breaker.

---

## Installation

### Prerequisites
- [Rust](https://www.rust-lang.org/) (1.95+ / Edition 2024)
- [`julie-extract`](https://github.com/anortham/julie-extractors) installed in your `PATH` (used by `code-kb scan` for initial extraction)

### Install via Cargo
```bash
# Install directly from local repository
cargo install --path crates/code-kb-cli --force
```

Verify the installation:
```bash
code-kb --version
```

---

## Quickstart: Indexing a Repository

Before running the server, or on any project you want to explore, run `scan` from the repository root:

```bash
cd /path/to/my-project
code-kb scan
```

This creates `.code-kb/artifact.db` containing AST facts, symbols, relationships, types, and full-text search indexes. 

*(Note: If you run `code-kb serve` on a repository where `scan` hasn't been run yet, `code-kb` will automatically trigger an initial scan on the first tool call.)*

---

## Configuring for AI Harnesses

`code-kb` communicates over standard `stdio` JSON-RPC and integrates cleanly with all major AI coding harnesses.

### 1. Claude Code (Anthropic CLI)

Claude Code supports `code-kb` either as a plugin or via the `claude mcp` CLI.

#### Option A: User-Level (Global for all projects — Recommended)
```bash
claude mcp add --scope user code-kb -- code-kb serve
```
*When launched in any repository, Claude Code spawns `code-kb`, sets CWD to that repo, and passes project roots during initialization.*

#### Option B: Project-Level (This repository only)
```bash
claude mcp add --scope project code-kb -- code-kb serve
```
*Writes the configuration directly to `.mcp.json` in the current project.*

#### Option C: Claude Code Plugin
`code-kb` includes a Claude Code plugin manifest (`.claude-plugin/plugin.json`). To install:
```bash
claude plugin install anortham/code-kb
```

---

### 2. Cursor IDE

In your project root, create `.cursor/mcp.json` (or add to Cursor Settings > Features > MCP):

```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve", "--root", "${workspaceFolder}"]
    }
  }
}
```

---

### 3. Antigravity (AGY)

#### Project-Level (`.mcp.json` in repo root — Recommended)
Create `.mcp.json` in your repository root:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve"]
    }
  }
}
```

#### Global Level (`~/.gemini/config/mcp_config.json`)
Add to `mcpServers` in your global config:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve"]
    }
  }
}
```

---

### 4. Claude Desktop

Add to your `claude_desktop_config.json` (located at `%APPDATA%\Claude\claude_desktop_config.json` on Windows or `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve", "--root", "C:/path/to/your/project"]
    }
  }
}
```

---

### 5. Codex CLI, Pi & Terminal Agents

Terminal-based AI harnesses inherit your terminal's current working directory automatically. Simply configure the harness to execute `code-kb serve` on startup.

---

## MCP Tool Catalog

| Tool | Purpose | Key Parameters |
| :--- | :--- | :--- |
| `codebase_outline` | High-level architectural orientation of directory layout & symbols. | `path` (opt), `depth` (opt, default 2) |
| `file_skeleton` | File outline with function & method bodies stripped. | `file_path` (req) |
| `find_symbol` | Fast semantic symbol search (exact & substring). | `query` (req), `kind` (opt), `is_test` (opt), `limit` (opt) |
| `search_symbols` | Conceptual BM25 full-text search over symbol signatures & docstrings. | `query` (req), `kind` (opt), `limit` (opt) |
| `get_symbol_body` | Slices the exact implementation body of a symbol from disk. | `symbol_name` (req), `file_path` (opt) |
| `get_context_slice` | Surgical bundle: target body + callee signatures + types + tests. | `symbol_name` (req), `file_path` (opt) |
| `find_references` | Traverses callers or callees of a symbol. | `symbol_name` (req), `direction` ("callers" \| "callees") |
| `find_structural_facts` | Queries framework facts (routes, SQL queries, config keys, tables). | `category` (req), `limit` (opt) |
| `replace_symbol_body` | Atomically replaces a symbol's implementation with syntax check. | `symbol_name` (req), `file_path` (req), `new_body` (req) |

---

## CLI Commands (Direct Terminal Usage)

Every MCP capability can be executed directly from your terminal:

```bash
# Architectural outline of current repo (depth 2)
code-kb outline

# Skeleton of a specific file
code-kb skeleton src/main.rs

# Search symbols by name or kind
code-kb symbol Workspace --kind struct

# Retrieve exact implementation body of a symbol
code-kb body Workspace

# Get surgical context slice (body + callees + types + tests)
code-kb slice ensure_fresh_file

# Find callers or callees of a function
code-kb refs open_read_only --direction callers

# Query framework structural facts
code-kb facts table --limit 10

# Atomically edit a symbol body
code-kb edit my_func --file src/lib.rs --body "{\n    println!(\"hello\");\n}"

# View active log file and recent diagnostic messages
code-kb logs
```

---

## Architecture & Engineering Plans

`code-kb` was designed based on extensive benchmarking and retrospectives from earlier code intelligence engines:

- [**001: Architecture & Service Model**](docs/plans/001-architecture-and-service-model.md) — Multi-project workstation scope, Git worktree deduplication, and RAG vs. AST evaluation.
- [**002: Workspace Scoping & MCP**](docs/plans/002-workspace-scoping-and-mcp.md) — 1:1 session binding, eliminating workspace registries and `workspace_id` friction.
- [**003: Tool Catalog & Schema**](docs/plans/003-tool-catalog-and-schema.md) — The core read tools, token-minimized output formats, and `replace_symbol_body`.
- [**004: File Synchronization & Watchers**](docs/plans/004-file-synchronization-and-watchers.md) — 3-tier sync: tool-driven updates, JIT staleness guards, and debounced background watching.
- [**005: Cold-Start Reconciliation**](docs/plans/005-startup-reconciliation.md) — Detecting and reconciling offline edits in under 50ms on startup.
- [**006: Retrospective Lessons from Miller**](docs/plans/006-lessons-from-miller.md) — Analysis of calibration data, performance ledgers, and traps to avoid.
- [**010: Master Implementation Plan**](docs/plans/010-master-implementation-plan.md) — The phased engineering roadmap from workspace scaffolding to release.
- [**ADR 001: Zero Workspace Parameters**](docs/decisions/001-zero-workspace-parameters.md) — Decision record strictly forbidding workspace parameters in tool schemas.
- [**AGENTS.md Guidelines**](AGENTS.md) — Strict architectural invariants and rules for AI coding assistants working in this repository.

---

## License

Dual-licensed under MIT or Apache-2.0.
