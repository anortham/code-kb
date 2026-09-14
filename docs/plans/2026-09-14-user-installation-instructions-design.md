# Design: User-Focused Per-Harness Installation Instructions

## Architecture Impact
`No Architecture Impact`

---

## 1. Problem & Context

Currently, [`README.md`](../../README.md#L34-L50) leads with developer-centric installation instructions:
- Requires installing Rust (1.95+ / Edition 2024).
- Requires manually installing `julie-extract` into PATH.
- Requires cloning the repository and running `cargo install --path crates/code-kb-cli --force`.

For end users wanting to integrate `code-kb` into their AI coding agents (Claude Code, Codex, Antigravity, Grok, Cursor, etc.), this creates unnecessary friction:
1. Every GitHub Release already ships precompiled, bundled binaries (`code-kb` + matching `julie-extract` side-by-side) for Linux x86_64, macOS Apple Silicon / Intel, and Windows x86_64 (Invariant 7). Users do not need Rust or a compiler.
2. Users configure `code-kb` within specific AI harnesses. Following the pattern established by [Ponytail](https://github.com/DietrichGebert/ponytail), each harness should have its own dedicated subsection with exact copy-paste plugin or MCP commands.
3. Users do not need to run `code-kb scan` manually; the server performs automatic workspace indexing on the first tool call in any new repository or git worktree.
4. Developers building from source should have their instructions cleanly separated into a `## Development` section.

---

## 2. Target Design & Section Structure

### 2.1 `## Install`

Replace current `## Installation` (lines 34–50) and `## Configuring for AI Harnesses` (lines 68–190) with a consolidated, user-first `## Install` section.

#### Step 1: Binary Setup (Zero-Dependency)
- **GitHub Releases (Recommended):** Download the latest release archive for your platform from GitHub Releases:
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

#### Step 2: Per-Harness Setup
Modeled after Ponytail's per-harness layout:

1. **`### Claude Code`**
   - Plugin Marketplace (Recommended):
     ```text
     /plugin marketplace add anortham/code-kb
     /plugin install code-kb@code-kb
     ```
   - CLI MCP fallback:
     ```bash
     claude mcp add --scope user code-kb -- code-kb serve
     ```
   - Note on features enabled: Injects routing instructions on session start and subagent start via native hooks, registers progressive disclosure skills.

2. **`### Codex`**
   - Plugin Marketplace:
     ```bash
     codex plugin marketplace add anortham/code-kb
     codex plugin add code-kb@code-kb
     ```
   - Manual MCP config in `~/.codex/config.toml`:
     ```toml
     [mcp_servers.code-kb]
     command = "code-kb"
     args = ["serve"]
     ```
   - Run `codex`, open `/hooks`, trust the lifecycle hooks.

3. **`### Antigravity CLI (AGY)`**
   - CLI command:
     ```bash
     agy mcp add code-kb code-kb serve
     ```
   - Global config (`~/.gemini/config/mcp_config.json`) with `"force_all_tools_eager": true`.
   - Progressive disclosure skill linking:
     ```bash
     ln -sf /path/to/code-kb/skills/code-kb ~/.gemini/config/skills/code-kb
     ```

4. **`### Grok CLI`**
   - Plugin install:
     ```bash
     grok plugin install anortham/code-kb --trust
     ```
   - Project-level `.mcp.json` fallback.

5. **`### Cursor`**
   - In `.cursor/mcp.json` (or Cursor Settings > Features > MCP):
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

6. **`### OpenCode`**
   - Add to `opencode.json`:
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

7. **`### Claude Desktop`**
   - Add to `claude_desktop_config.json` (`%APPDATA%\Claude` on Windows, `~/Library/Application Support/Claude` on macOS):
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

8. **`### GitHub Copilot CLI & Terminal Agents`**
   - For terminal harnesses inheriting CWD (Copilot CLI, Pi, Swival, Windsurf, Zed): configure the MCP server to run `code-kb serve`.

---

### 2.2 `### First Run & Automatic Indexing`

Clarify workspace behavior:
- Users do **not** need to manually run `code-kb scan`.
- When an agent calls any `code-kb` tool in a repository for the first time, `code-kb` automatically creates `<workspace>/.code-kb/artifact.db` and runs an initial scan.
- Manual indexing via `code-kb scan` remains available for pre-indexing large repositories before agent sessions.

---

### 2.3 `### Uninstall`

A concise table for removing `code-kb`:

| Harness | Command / Action |
|---|---|
| Claude Code | `/plugin remove code-kb` (or `claude mcp remove code-kb`) |
| Codex | `codex plugin remove code-kb` |
| Antigravity (AGY) | `agy mcp remove code-kb` |
| Grok CLI | `grok plugin uninstall code-kb` |
| Cursor / OpenCode | Remove the `code-kb` entry from `.cursor/mcp.json` / `opencode.json` |

---

### 2.4 `## Development`

Move build-from-source and contributor workflows down to a dedicated section:
- Prerequisites: Rust 1.95+, `scripts/restore-julie-extract.sh`.
- Local installation: `cargo install --path crates/code-kb-cli --force`.
- Running tests & pre-flight: `./scripts/release-preflight.sh` and `cargo test --workspace`.

---

## 3. Files Impacted

- [`README.md`](../../README.md): Rewrite sections `## Installation`, `## Quickstart: Indexing a Repository`, and `## Configuring for AI Harnesses` into `## Install`, `### First Run & Automatic Indexing`, `### Uninstall`, and add `## Development` near the bottom.

---

## 4. Acceptance Criteria Checklist

- [ ] `README.md` starts installation with zero-dependency precompiled releases (bundles `code-kb` + `julie-extract`) and Cargo.
- [ ] Each supported harness has its own dedicated subsection (`### Claude Code`, `### Codex`, `### Antigravity CLI (AGY)`, `### Grok CLI`, `### Cursor`, `### OpenCode`, `### Claude Desktop`, `### GitHub Copilot CLI & Terminal Agents`).
- [ ] Explicitly documents that manual `scan` is optional because initial indexing triggers automatically on the first tool call.
- [ ] Includes an `### Uninstall` matrix.
- [ ] Source compilation and developer prerequisites are moved to `## Development`.
- [ ] Formatting is clean, concise, unslop-compliant, and free of AI conversational filler.
- [ ] Links and code snippets are accurate and tested against repo paths and CLI flags.
