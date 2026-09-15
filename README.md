# code-kb

> **Interactive Showcase & Benchmarks:** [https://anortham.github.io/code-kb/](https://anortham.github.io/code-kb/)

`code-kb` is a fast, lightweight code-intelligence engine and Model Context Protocol (MCP) server designed specifically for AI coding agents. 

Backed by the rich AST fact tables produced by [`julie-extractors`](https://github.com/anortham/julie-extractors), `code-kb` provides progressive disclosure, semantic symbol navigation, and surgical context slicing—enabling agents to navigate, understand, and edit codebases with **80–90% fewer tokens** without burning context on full file reads or noisy text grep.

---

## Why code-kb?

Traditional AI coding agents burn massive amounts of context loading entire source files into their prompt windows just to inspect a single type signature or implementation detail. 

`code-kb` solves this with a **progressive disclosure** architecture:
1. **Repository Orientation (`codebase_outline`):** Understand directory structures and key exports in ~200 tokens.
2. **File Skeletons (`file_skeleton`):** Inspect function signatures, types, traits, and docstrings with implementation bodies stripped.
3. **Symbol Lookup & Discovery (`lookup_symbol` / `search_symbols`):** Instant exact/prefix identifier lookups and conceptual FTS5 search across all symbols.
4. **Surgical Symbol Context (`get_symbol_context`):** In a single turn, fetch a target function's body along with its callee signatures, parameter types, and associated unit tests.
5. **Atomic AST Edits (`replace_symbol_body`):** Replace symbol implementations atomically with pre-flight syntax validation for Rust, JavaScript, TypeScript/TSX, Python, and Go; other languages report validation skipped, then re-index immediately.

---

## Key Principles

- **Sub-15MB Retained Memory:** Written in Rust, zero heavy runtimes (no Node.js daemon, no web dashboard, no GPU models), retained process memory stays below 15 MB.
- **Sub-5ms Query Latency:** Direct SQLite queries in WAL mode with zero in-memory heap bloat.
- **Zero Workspace Parameters:** Pure semantic tool calling (`lookup_symbol(query="...")`). The agent is never burdened with `workspace_id`, `repo_path`, or path confusion.
- **CLI-First Parity:** Every MCP tool has an exact 1:1 CLI command for instantaneous terminal verification and dogfooding.
- **Continuous 3-Tier Sync:** Tool-driven updates, JIT staleness guards before reads, and a debounced background watcher with a Git storm circuit breaker.

---

## Install

### Step 1: Binary Setup (Zero-Dependency)

- **GitHub Releases (Recommended):** Download the latest release archive for your platform from [GitHub Releases](https://github.com/anortham/code-kb/releases):
  - Linux x86_64 (`.tar.gz`)
  - macOS Apple Silicon (`.tar.gz`)
  - macOS Intel (`.tar.gz`)
  - Windows x86_64 (`.zip`)
- Unpack and put the binaries in your `PATH` (e.g. `~/.local/bin`, `/usr/local/bin`, or `C:\tools`).
- *Note on Bundled Distribution:* Both `code-kb` and `julie-extract` are pre-packaged side-by-side in the release archive. `code-kb` locates `julie-extract` right next to its own executable automatically.
- **Cargo (Rust Users):**
  ```bash
  cargo binstall code-kb-cli
  # or
  cargo install code-kb-cli
  ```
- **Verification:**
  ```bash
  code-kb --version
  ```

### Step 2: Connect Your Agent

#### Claude Code

Plugin Marketplace (Recommended):
```text
/plugin marketplace add anortham/code-kb
```
```text
/plugin install code-kb@code-kb
```
*(Send as two separate prompts in Claude Code)*

CLI MCP fallback:
```bash
claude mcp add --scope user code-kb -- code-kb serve
```

*Note:* Injects routing instructions on session start and subagent start via native hooks, and registers progressive disclosure skills.

#### Codex

Plugin Marketplace:
```bash
codex plugin marketplace add anortham/code-kb
codex plugin add code-kb@code-kb
```

Manual MCP config in `~/.codex/config.toml`:
```toml
[mcp_servers.code-kb]
command = "code-kb"
args = ["serve"]
```

Run `codex`, open `/hooks`, and trust the lifecycle hooks.

#### Antigravity CLI (AGY)

CLI command:
```bash
agy mcp add code-kb code-kb serve
```

Global config (`~/.gemini/config/mcp_config.json`) with `"eager": true`:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve"],
      "disabled": false,
      "eager": true,
      "force_all_tools_eager": true
    }
  }
}
```

Lifecycle hook configuration (`~/.gemini/config/hooks.json`):
```json
{
  "code-kb": {
    "PreInvocation": [
      {
        "command": "code-kb hook PreInvocation",
        "timeout": 10,
        "type": "command"
      }
    ]
  }
}
```
*Note:* Antigravity does not support `SessionStart` hooks. It uses `PreInvocation` with `injectSteps` to deliver turn-level routing directives.

Progressive disclosure skill linking:
```bash
ln -sf /path/to/code-kb/skills/code-kb ~/.gemini/config/skills/code-kb
```

#### Grok CLI

Plugin install:
```bash
grok plugin install anortham/code-kb --trust
```

Project-level `.mcp.json` fallback:
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

#### Cursor

In `.cursor/mcp.json` (or Cursor Settings > Features > MCP):
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

#### OpenCode

Add to `opencode.json`:
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

#### Claude Desktop

Add to `claude_desktop_config.json` (`%APPDATA%\Claude\claude_desktop_config.json` on Windows, `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):
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

#### GitHub Copilot CLI & Terminal Agents

For terminal harnesses inheriting CWD (Copilot CLI, Pi, Swival, Windsurf, Zed): configure the MCP server to run `code-kb serve`.

---

### First Run & Automatic Indexing

You do not need to manually run `code-kb scan`.

When an agent calls any `code-kb` tool in a repository for the first time, `code-kb` automatically creates `<workspace>/.code-kb/artifact.db` and runs an initial scan.

Manual indexing via `code-kb scan` remains available for pre-indexing large repositories before agent sessions:

```bash
code-kb scan
```

---

### Uninstall

| Harness | Command / Action |
|---|---|
| Claude Code | `/plugin remove code-kb` (or `claude mcp remove code-kb`) |
| Codex | `codex plugin remove code-kb` |
| Antigravity (AGY) | `agy mcp remove code-kb` |
| Grok CLI | `grok plugin uninstall code-kb` |
| Cursor / OpenCode | Remove the `code-kb` entry from `.cursor/mcp.json` / `opencode.json` |

---

## MCP Tool Catalog

| Tool | Purpose | Key Parameters | Aliases |
| :--- | :--- | :--- | :--- |
| `codebase_outline` | High-level architectural orientation of directory layout & symbols. | `path` (opt), `depth` (opt, default 2) | `dir`, `subpath` |
| `file_skeleton` | File outline with function & method bodies stripped (80–90% token savings). | `file_path` (req) | `file`, `path` |
| `lookup_symbol` | Fast identifier lookup (exact name or prefix) across repo or scoped path. | `query` (req), `path` (opt), `kind` (opt), `is_test` (opt), `limit` (opt) | `name`, `q` |
| `search_symbols` | Conceptual BM25 full-text search over symbol signatures & docstrings. | `query` (req), `path` (opt), `kind` (opt), `limit` (opt) | `name`, `q` |
| `get_symbol_body` | Slices the exact implementation body of a symbol from disk. | `symbol_name` (req), `file_path` (opt) | `symbol`, `name`, `path` |
| `get_symbol_context` | Surgical bundle: target body + callee signatures + parameter types + tests. | `symbol_name` (req), `file_path` (opt), `include_external` (opt, def: false) | `symbol`, `name`, `path` |
| `find_references` | Traverses callers or callees of a symbol (filters external stdlib noise). | `symbol_name` (req), `file_path` (opt), `direction` ("callers" \| "callees", def: callers), `include_external` (opt, def: false) | `symbol`, `name`, `file`, `path` |
| `blast_radius` | Multi-hop reverse reachability (CTEs) & targeted test prediction. | `symbol` (opt), `file` (opt), `depth` (opt, def: 2), `limit` (opt) | `name`, `path`, `impact` |
| `find_structural_facts` | Queries framework facts (routes, SQL queries, config keys, tables). Lists all categories when omitted. | `category` (opt), `path` (opt), `limit` (opt) | `cat`, `kind`, `type`, `file`, `file_path` |
| `replace_symbol_body` | Atomically replaces a symbol's implementation; syntax validation covers Rust, JavaScript, TypeScript/TSX, Python, and Go. | `symbol_name` (req), `file_path` (req), `new_body` (req), `expected_body_hash` (opt) | `symbol`, `file`, `body`, `code` |

---

## CLI Commands (Direct Terminal Usage)

Every MCP capability can be executed directly from your terminal with 1:1 parity:

```bash
# Architectural outline of current repo (depth 2)
code-kb outline

# Skeleton of a specific file (implementation bodies stripped)
code-kb skeleton src/main.rs

# Lookup symbols by exact name or prefix (alias: code-kb symbol)
code-kb lookup Workspace --kind struct --path crates/code-kb-core

# Conceptual BM25 full-text search across docstrings and signatures
code-kb search "syntax validation concurrency"

# Retrieve exact implementation body of a symbol
code-kb body Workspace

# Get surgical context bundle: body + callees + types + tests (alias: code-kb slice)
code-kb context ensure_fresh_file

# Find callers or callees of a function (language-agnostically filters stdlib noise)
code-kb refs open_read_only --direction callers

# View callees including external standard library tokens
code-kb refs open_read_only --direction callees --include-external

# Predict blast radius and targeted tests to run for a symbol or file
code-kb blast-radius compute_blast_radius

# Zero-argument blast radius: auto-discovers uncommitted git working-tree changes
code-kb blast-radius
# (alias: code-kb impact)

# Query framework structural facts (or omit category to list all detected categories)
code-kb facts
code-kb facts config --path Cargo.toml
code-kb facts route --limit 10

# Atomically edit a symbol body with pre-flight tree-sitter syntax validation
code-kb edit my_func --file src/lib.rs --body "{\n    println!(\"hello\");\n}"

# View active log file and recent diagnostic messages
code-kb logs

# Output agent lifecycle hook payload (SessionStart / SubagentStart / PreInvocation)
code-kb hook SessionStart
code-kb hook SubagentStart
code-kb hook PreInvocation
```

---

## Development

For contributors building `code-kb` from source:

### Prerequisites
- [Rust](https://www.rust-lang.org/) (1.95+ / Edition 2024)
- Extractor binary: Run `./scripts/restore-julie-extract.sh` to download the pinned [`julie-extract`](https://github.com/anortham/julie-extractors) binary

### Build and Install Locally
```bash
# Restore pinned julie-extract binary
./scripts/restore-julie-extract.sh

# Install code-kb binary from local checkout
cargo install --path crates/code-kb-cli --force
```

### Running Tests & Verification
```bash
# Run test suite
cargo test --workspace

# Run release pre-flight verification
./scripts/release-preflight.sh
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
