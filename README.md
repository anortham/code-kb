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
3. **Symbol Lookup & Discovery (`lookup_symbol` / `search_symbols`):** Instant exact/prefix identifier lookups and conceptual search over names, signatures, and docstrings; substrings inside identifiers are found (`sha256` finds `parseSha256Sidecar`).
4. **Surgical Symbol Context (`get_symbol_context`):** In a single turn, fetch a target function's body along with its callee signatures, parameter types, and associated unit tests.
5. **Atomic AST Edits (`replace_symbol_body`):** Replace symbol implementations atomically with pre-flight syntax validation by `julie-extract` for every language it parses, then re-index immediately.

---

## Key Principles

- **Small Retained Memory:** Written in Rust, zero heavy runtimes (no web dashboard, no GPU models), retained process memory is about 25 MB. The plugin launcher is a small Node script that replaces itself with the native binary via `process.execve` on supported POSIX Node 22+ runtimes (or waits via `spawn` on Windows/older Node).
- **Sub-5ms Query Latency:** Direct SQLite queries in WAL mode with zero in-memory heap bloat.
- **Zero Workspace Parameters:** Pure semantic tool calling (`lookup_symbol(query="...")`). The agent is never burdened with `workspace_id`, `repo_path`, or path confusion.
- **CLI-First Parity:** Every MCP tool has an exact 1:1 CLI command for instantaneous terminal verification and dogfooding.
- **Continuous 3-Tier Sync:** Tool-driven updates, JIT staleness guards before reads, and a debounced background watcher with a Git storm circuit breaker.

---

## Install

Install the plugin for your agent. The plugin ships a small Node.js launcher. On the first
run it downloads the matching `code-kb` release archive for your platform, verifies its
SHA-256, and unpacks `code-kb` and `julie-extract` into `~/.code-kb/dist/<version>/`. Later
runs start instantly. Nothing else to download or put on `PATH`.

Requirements:
- Node.js 18 or newer on `PATH` (the launcher is a Node script; the server itself is a native binary).
- Windows 10 build 17063 or newer (the launcher unpacks with the built-in `tar.exe`).

### Claude Code

```text
/plugin marketplace add anortham/code-kb
```
```text
/plugin install code-kb@code-kb
```
*(Send as two separate prompts.)* The plugin registers the MCP server, the progressive
disclosure skill, a `/code-kb:telemetry` command for usage and token-savings reports, and
SessionStart and SubagentStart hooks that inject routing instructions.

### Codex

```bash
codex plugin marketplace add anortham/code-kb
codex plugin add code-kb@code-kb
```
Run `codex`, open `/hooks`, and trust the two code-kb hooks.

To update, refresh the marketplace snapshot first. `codex plugin remove` followed by
`codex plugin add` reinstalls the version already in the snapshot:

```bash
codex plugin marketplace upgrade
codex plugin remove code-kb@code-kb
codex plugin add code-kb@code-kb
```

### Antigravity CLI (AGY)

```bash
agy plugin install https://github.com/anortham/code-kb
```
The plugin registers the MCP server and a `PreInvocation` hook. Antigravity has no
`SessionStart` hook, so routing directives arrive per turn through `injectSteps`.

### Grok CLI

```bash
grok plugin install anortham/code-kb --trust
```

### First Run & Automatic Indexing

You do not need to run `code-kb scan` by hand. The first `code-kb` tool call or CLI command in
a repository creates `<workspace>/.code-kb/artifact.db` and runs the initial scan. A git
worktree copies its parent repository's index instead of scanning again. Files that changed
while no session ran, for example after a branch switch, are reconciled before the first
answer. Pre-index a large repository before a session with:

```bash
code-kb scan
```

Launcher environment variables:

| Variable | Effect |
|---|---|
| `CODE_KB_HOME` | Directory that holds `dist/` (default `~/.code-kb`). |
| `CODE_KB_VERSION` | Release version to fetch instead of the plugin's own version. |
| `CODE_KB_BIN` | Run this binary and skip the download entirely (local builds). |

Two overrides need no environment variable, which matters in harnesses that do not pass the
environment to MCP servers (Codex):

- A binary or symlink at `~/.code-kb/bin/code-kb` (`code-kb.exe` on Windows) runs instead of any
  download, in every harness. For development: `ln -s /path/to/code-kb/target/release/code-kb ~/.code-kb/bin/code-kb`.
- A plugin installed from a source checkout runs that checkout's `target/release/code-kb` when it
  exists.

With either in place, `cargo build --release` plus a session restart is the whole development loop.

### Uninstall

| Harness | Command / Action |
|---|---|
| Claude Code | `/plugin remove code-kb` |
| Codex | `codex plugin remove code-kb@code-kb` |
| Antigravity (AGY) | `agy plugin uninstall code-kb` |
| Grok CLI | `grok plugin uninstall code-kb` |

Delete `~/.code-kb/dist` to remove the downloaded binaries. Each workspace keeps its index
in `<workspace>/.code-kb/`; delete that directory to remove the index.

---

## Manual Configuration

Use this path for harnesses without a plugin manager, or when you would rather manage the
binary yourself.

### Step 1: Get the Binaries

- **GitHub Releases:** download the archive for your platform from
  [GitHub Releases](https://github.com/anortham/code-kb/releases):
  - Linux x86_64 and ARM64 (`.tar.gz`)
  - macOS Apple Silicon and Intel (`.tar.gz`)
  - Windows x86_64 and ARM64 (`.zip`)

  Unpack it and put both binaries on your `PATH` (for example `~/.local/bin`,
  `/usr/local/bin`, or `C:\tools`). `code-kb` and `julie-extract` are packaged side by side,
  and `code-kb` finds `julie-extract` next to its own executable.
- **Cargo:**
  ```bash
  cargo binstall code-kb-cli
  # or
  cargo install code-kb-cli
  ```
  Cargo installs `code-kb` only. Download the pinned `julie-extract` from the
  [julie-extractors releases](https://github.com/anortham/julie-extractors/releases) (version
  in `scripts/julie-pins.json`) and put it on your `PATH` or set `JULIE_EXTRACT_BIN`.
- **Verify:**
  ```bash
  code-kb --version
  ```

### Step 2: Register the MCP Server

Every harness runs the same command: `code-kb serve`. Hooks run `code-kb hook <Event>`.

#### How code-kb Finds Your Workspace

Tools never take a workspace parameter. The server binds a workspace in three steps, and each
later step replaces the earlier one:

1. At start: `--root <path>` on the `serve` command, or, without it, the directory the process
   starts in, searched upward for `.git` or a project marker.
2. At handshake: the roots the host sends in the MCP `initialize` request, if any.
3. At each tool call: an absolute path inside a repository in any `file_path` or `path` argument.

Terminal harnesses (Claude Code, Codex, AGY, Grok CLI, Copilot CLI, Pi, Swival, Zed) start the
server in the project directory, so `code-kb serve` alone is enough.

GUI apps (Cursor, Windsurf, the Antigravity IDE, Visual Studio, VS Code, Claude Desktop) start
the server from their own install directory, not from your project. For these apps, put the MCP
config inside the project and pass `--root` with the absolute path of the project:

```json
"args": ["serve", "--root", "/absolute/path/to/project"]
```

Without `--root`, the first tool call in a GUI app fails with
`Database artifact not found ... configure code-kb with '--root <repo-path>'`. That error is the
signal to add the flag. A tool call with an absolute path inside a repository also binds the
server, so a session can recover, but `--root` removes the guesswork.

#### Claude Code (without the plugin)

```bash
claude mcp add --scope user code-kb -- code-kb serve
```

#### Codex (without the plugin)

In `~/.codex/config.toml`:
```toml
[mcp_servers.code-kb]
command = "code-kb"
args = ["serve"]
```

#### Antigravity CLI (without the plugin)

```bash
agy mcp add code-kb code-kb serve
```

Global config (`~/.gemini/config/mcp_config.json`) with `"eager": true`. The AGY CLI starts the
server in the project directory. The Antigravity IDE starts it from its own install directory,
so add `--root` when you use the IDE:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve", "--root", "/absolute/path/to/project"],
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

Progressive disclosure skill linking:
```bash
ln -sf /path/to/code-kb/skills/code-kb ~/.gemini/config/skills/code-kb
```

#### Grok CLI (without the plugin)

Project-level `.mcp.json`:
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

In `.cursor/mcp.json` at the repository root. Cursor is a GUI app, so pass `--root`:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve", "--root", "/absolute/path/to/project"]
    }
  }
}
```

#### OpenCode

Add to `opencode.json` in the project:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve", "--root", "/absolute/path/to/project"]
    }
  }
}
```

#### Claude Desktop

Claude Desktop has one global config and no project directory, so `--root` pins one project.
Add to `claude_desktop_config.json` (`%APPDATA%\Claude\claude_desktop_config.json` on Windows, `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve", "--root", "/absolute/path/to/project"]
    }
  }
}
```

#### Other GUI Apps (Windsurf, Visual Studio, VS Code)

Use the app's project-level MCP config file and the same arguments:
`["serve", "--root", "/absolute/path/to/project"]`. On Windows write the path with forward
slashes, for example `C:/source/project`.

#### GitHub Copilot CLI & Terminal Agents

Terminal harnesses (Copilot CLI, Pi, Swival, Zed) start the server in the project directory:
configure the MCP server to run `code-kb serve` with no `--root`.

To remove a manual configuration, delete the `code-kb` entry from the harness config
(`claude mcp remove code-kb`, `agy mcp remove code-kb`, or edit the file) and delete the
binaries from your `PATH`.

---

## MCP Tool Catalog

| Tool | Purpose | Key Parameters | Aliases |
| :--- | :--- | :--- | :--- |
| `codebase_outline` | High-level architectural orientation of directory layout & symbols. | `path` (opt), `depth` (opt, default 2) | `dir`, `subpath` |
| `file_skeleton` | File outline with function & method bodies stripped (80–90% token savings); a directory returns its outline. | `file_path` (req) | `file`, `path` |
| `lookup_symbol` | Fast identifier lookup (exact name or prefix) across repo or scoped path. Test functions, test containers, and rows from test files are hidden unless `is_test` is true, except a row whose name equals the query. | `query` (req), `path` (opt), `kind` (opt), `is_test` (opt), `limit` (opt) | `name`, `q` |
| `search_symbols` | Conceptual search over symbol names, signatures & docstrings; substrings inside identifiers are found (`sha256` finds `parseSha256Sidecar`). Results are reranked by crediting each query term once from its strongest field (name, signature, docstring), weighted by the term's rarity across the index, plus kind and path priors; `score` is that rerank score. Test functions, test containers, and rows from test files are hidden unless `is_test` is true. | `query` (req), `path` (opt), `kind` (opt), `is_test` (opt), `limit` (opt) | `name`, `q` |
| `get_symbol_body` | Slices the exact implementation body of a symbol from disk. | `symbol_name` (req), `file_path` (opt) | `symbol`, `name`, `path` |
| `get_symbol_context` | Surgical bundle: target body + callee signatures + parameter types + tests. | `symbol_name` (req), `file_path` (opt), `include_external` (opt, def: false) | `symbol`, `name`, `path` |
| `find_references` | Callers or callees of a symbol, matched by name from AST call sites and ranked by same file, same directory, then receiver type; callers also include type usages and member accesses (filters external stdlib noise; qualify overloaded names). | `symbol_name` (req), `file_path` (opt), `direction` ("callers" \| "callees", def: callers), `include_external` (opt, def: false) | `symbol`, `name`, `file`, `path` |
| `blast_radius` | Multi-hop reverse reachability (CTEs) & targeted test prediction. | `symbol` (opt), `file` (opt), `depth` (opt, def: 2), `limit` (opt) | `name`, `path`, `impact` |
| `find_structural_facts` | Queries framework facts (routes, SQL queries, config keys, tables). Lists all categories when omitted. | `category` (opt), `path` (opt), `limit` (opt) | `cat`, `kind`, `type`, `file`, `file_path` |
| `replace_symbol_body` | Atomically replaces a symbol's implementation; `julie-extract check` validates the syntax for every language it parses (about 40), other paths report validation skipped. | `symbol_name` (req), `file_path` (req), `new_body` (req), `expected_body_hash` (opt) | `symbol`, `file`, `body`, `code` |
| `telemetry_summary` | Token savings, call counts, and error rates from `~/.code-kb/telemetry.db`, across all workspaces or scoped to the current one. | `time_window` (opt, def: all), `workspace_only` (opt, def: false), `json` (opt) | `since`, `window` |

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

# Conceptual search over names, signatures, and docstrings (sha256 finds parseSha256Sidecar)
code-kb search "syntax validation concurrency"

# Same search with the rerank breakdown under each result (JSON: an `explain` object per result)
code-kb search "sha256 sidecar" --explain

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

# Atomically edit a symbol body with pre-flight syntax validation by julie-extract
code-kb edit my_func --file src/lib.rs --body "{\n    println!(\"hello\");\n}"

# View active log file and recent diagnostic messages
code-kb logs

# Token savings, call counts, and error rates (alias: code-kb telemetry)
code-kb stats --since month
code-kb stats --workspace

# Self-contained diagnostic bundle (versions, index facts, recent errors, log tail) with a
# pre-filled GitHub issue link; the /report-issue skill files it through `gh issue create`
code-kb bug-report --title "lookup returns nothing" --description "what went wrong"

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

# Run the plugin launcher and manifest tests
node --test tests/plugin/*.test.cjs

# Run release pre-flight verification
./scripts/release-preflight.sh
```

To run the Claude Code plugin from this checkout, build and load it. The launcher runs
`target/release/code-kb` when it exists, so no download happens:
```bash
cargo build --release
claude --plugin-dir .
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
