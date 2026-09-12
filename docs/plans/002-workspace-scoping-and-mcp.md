# code-kb: Workspace Scoping & MCP Architecture

## 1. Context & The "Workspace Registry" Anti-Pattern

In previous iterations and other multi-workspace tools, a common architectural pattern was building a centralized "workspace registry":
- An agent or client registers `Workspace A` (id: `ws-1234`) and `Workspace B` (id: `ws-5678`).
- Every MCP tool call requires explicit scoping: e.g., `find_symbol(workspace_id="ws-1234", query="Token")` or `file_skeleton(repo_path="C:/source/projA", file="src/main.rs")`.

### Why This Fails in Practice:
1. **LLM Cognitive Overhead & Hallucination:** LLMs frequently forget to pass the `workspace_id`, hallucinate the UUID, pass stale IDs from earlier conversation turns, or mix up projects.
2. **CWD Disconnect in Agent Launchers:** GUI clients (Claude Desktop, Cursor, VS Code plugins) frequently spawn MCP servers from cache directories (e.g., `~/.claude/plugin-cache/` or `%APPDATA%`), NOT from the workspace root. Tools that rely on `std::env::current_dir()` crash or inspect the wrong folder.
3. **Over-Engineering Scope:** Supporting cross-project exploration (an agent in Project A inspecting Project B) adds multi-tenancy routing, permission boundaries, and state drift for an edge case that accounts for < 0.1% of real coding tasks.

---

## 2. The code-kb Philosophy: 1:1 Session Binding & Zero Agent Friction

`code-kb` eliminates the workspace registry entirely by enforcing three design principles:
1. **Zero Workspace Parameters:** The LLM NEVER sees or passes a `workspace_id`, `repo_path`, or `root_dir`. Tool calls are purely semantic: `find_symbol(query, kind)` and `file_skeleton(file_path)`.
2. **1:1 Process-to-Workspace Binding:** Each MCP server instance is strictly bound to exactly ONE workspace root for the duration of its lifecycle.
3. **Transparent Path Resolution:** The server transparently handles relative paths (`src/main.rs`), absolute paths (`C:/source/project/src/main.rs`), and Windows verbatim paths (`\\?\C:\...`).

---

## 3. How code-kb Discovers the Workspace Root Reliably

To ensure the server always binds to the correct project—even when launched from a plugin cache directory—`code-kb` implements a robust 3-tier fallback discovery:

```text
┌─────────────────────────────────────────────────────────────┐
│ Tier 1: MCP Protocol Roots (Standard Spec)                  │
│ Client sends params.roots[0].uri during "initialize"        │
└──────────────────────────────┬──────────────────────────────┘
                               │ If empty or unsupported
┌──────────────────────────────▼──────────────────────────────┐
│ Tier 2: Explicit CLI Flag                                   │
│ code-kb serve --root <path> (via ${workspaceFolder})        │
└──────────────────────────────┬──────────────────────────────┘
                               │ If not provided
┌──────────────────────────────▼──────────────────────────────┐
│ Tier 3: Upward Git / Marker Discovery                       │
│ Traverse up from target file / CWD to find .git, Cargo.toml │
└─────────────────────────────────────────────────────────────┘
```

### 1. Tier 1: MCP Standard Protocol `initialize` Roots
The Model Context Protocol specification includes `roots` in the initialization handshake:
```json
{
  "method": "initialize",
  "params": {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": { "name": "claude-code", "version": "1.0" },
    "roots": [
      {
        "uri": "file:///c:/source/my-project",
        "name": "my-project"
      }
    ]
  }
}
```
When `code-kb` receives `initialize`, it parses `params.roots[0].uri`, converts it to a canonical filesystem path, and anchors the session.

### 2. Tier 2: Explicit `--root` CLI Argument
When configured in client configs (`.cursor/mcp.json`, `claude_desktop_config.json`):
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
If `--root` is passed, it takes precedence and binds the server directly to the specified root.

### 3. Tier 3: Git Worktree & Root Traversal
If launched without `--root` and without protocol roots, `code-kb` inspects the initial request target:
- Searches upward for `.git`.
- **Git Worktree Support:** If `.git` is a file containing `gitdir: <common-repo>/worktrees/<name>`, `code-kb` automatically detects:
  1. The **Worktree Root** (the specific view/directory being edited).
  2. The **Common Git Root** (where the shared Family Store lives).

---

## 4. Path Normalization Contract

When an agent calls a tool (e.g., `file_skeleton("src/auth/login.rs")`):
1. **Relative Paths:** Automatically joined with the bound workspace root:
   `workspace_root.join(input_path)`
2. **Absolute Paths:**
   - Canonicalized and checked for containment within `workspace_root`.
   - Windows verbatim prefixes (`\\?\C:\...`) are cleanly stripped.
   - Forward slashes `/` are normalized.
3. **Database Artifact Binding:**
   - The workspace root generates a stable repository identity hash: `SHA256(canonical_root)[0..16]`.
   - The SQLite artifact is opened from `%LOCALAPPDATA%\code-kb\stores\<repo_slug>-<hash>\artifact.db` (or Family Store view).

This guarantees that the LLM agent experiences zero tool errors regarding paths, IDs, or directory confusion.

---

## 5. Architectural Invariant: The Global MCP CWD Trap & Zero Schema Pollution

### The Problem
When MCP servers are registered globally in host clients (e.g. `~/.gemini/config/mcp_config.json`), the host client frequently spawns the server process with its own application directory as the CWD (e.g. `C:\Users\...\AppData\Local\Programs\antigravity`), where no project repository or `.code-kb/artifact.db` exists.

In previous architectures (e.g. Goldfish), developers reacted to this by adding an optional `workspace: string` parameter to every tool schema, instructing the LLM to provide it on every call.

### Why That Approach Fails
1. **Prompt Pollution:** Once an LLM sees `workspace` in a tool schema, it feels compelled to append `workspace: "..."` to every tool call, consuming context tokens and increasing latency.
2. **Path Hallucination:** Models frequently hallucinate path formats, slash directions, or cross-project paths.
3. **Ergonomic Regression:** The simplicity of semantic tool calling (`find_symbol(query: "Workspace")`) is destroyed.

### The code-kb Contract
1. **Tool Schemas Are Inviolate:** No tool shall ever expose `workspace`, `workspace_id`, or `repo_path` in its `inputSchema`.
2. **Silent Backend Auto-Scan:** If the server is bound to a repository directory that has not been indexed yet, `code-kb` runs `scan_workspace` automatically on the first tool call to create `.code-kb/artifact.db` and start the watcher.
3. **Silent Path Binding:** If a tool call contains an absolute path in `file_path` or `path`, `code-kb` silently anchors to that project root.
4. **CI Enforcement:** Automated tests (`crates/code-kb-cli/tests/mcp_test.rs`) verify that `tools/list` never exposes `workspace` across any tool schema.

