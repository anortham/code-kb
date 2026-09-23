# ADR 001: Zero Workspace Parameters in Tool Schemas

> **Superseded in 2.1.0** by the required `project_root` parameter. See
> [the 2.1.0 plan](../plans/2026-09-23-mcp-roots-worktree-binding.md). A root held in server state
> went stale when the agent moved to a git worktree (issue #3), and MCP roots cannot supply the
> root, so every tool except `telemetry_summary` now takes the project path on each call.

## Status
**Accepted & Enforced** (2026-09-12)

## Context
In previous code-intelligence tools (such as Miller and Goldfish), tools were designed with explicit workspace parameters (e.g. `workspace_id` or `workspace: string`). In Miller, `McpWorkspaceTargetPolicy` required agents to pass a `workspace_id` on every tool call. In Goldfish, global MCP registrations prompted agents to pass `workspace: "<absolute-path>"` on every call.

When `code-kb` was first registered globally in Antigravity (`~/.gemini/config/mcp_config.json`), Antigravity spawned the process with its own install directory as the working directory (`C:\Users\alann\AppData\Local\Programs\antigravity`). Because no `.code-kb/artifact.db` was found in that folder on the first call, an implementation agent added an optional `workspace: string` parameter to all 8 tool schemas.

This re-introduced the exact anti-pattern `code-kb` was created to solve:
1. **LLM Prompt Pollution:** The moment an LLM sees `"workspace": { "type": "string", "description": "Required when server is registered globally without --root" }` in tool schemas, it feels obligated to inject `workspace: "c:/source/..."` into every single tool call.
2. **Context & Token Burn:** Every turn burns tokens passing repetitive filesystem paths that the backend should already know or resolve silently.
3. **Hallucination & Failure Mode:** Models frequently hallucinate workspace paths, forward/backward slash mismatches, or stale project paths across conversation turns.

## Decision
1. **Zero Schema Pollution:** No tool exposed by `code-kb` over MCP shall ever declare `workspace`, `workspace_id`, `repo_path`, or `root_dir` in its `inputSchema`.
2. **Zero Prompting in Errors:** Error messages returned by `code-kb` must never instruct the LLM to pass a workspace parameter.
3. **Automated Enforcement:** `crates/code-kb-cli/tests/mcp_test.rs` programmatically asserts that `inputSchema.properties.get("workspace")` is `None` across all registered tools. Builds will fail if this invariant is violated.
4. **Silent Backend Resolution:**
   - **Tier 0 (Explicit `--root`):** GUI hosts (Cursor, the Antigravity IDE, Visual Studio, Claude Desktop) start the server from their own install directory. Their project-local MCP config passes `code-kb serve --root <absolute-path>`. The workspace lives in the config, never in a tool call.
   - **Tier 1 (Project-local `.mcp.json`):** CWD is the workspace root; server binds automatically. This covers terminal harnesses (Claude Code, Codex, AGY, Grok CLI).
   - **Tier 2 (Protocol Handshake):** Server extracts `roots`, `rootUri`, `rootPath`, or `workspaceFolders` during `initialize`.
   - **Tier 3 (Path Inspection):** Server silently binds to a workspace if an absolute path is passed in `file_path` or `path`.
   - **Tier 4 (Automatic Scan):** If bound to a valid repository (containing `.git`, `Cargo.toml`, `package.json`, etc.) where `artifact.db` has not been generated yet, `code-kb` automatically triggers `scan_workspace` to build the database artifact on the first tool call without returning an error.
   - **Internal Fallback:** If an unadvertised `workspace` argument is provided internally or via client configuration, the backend handles it gracefully, but never advertises it in schemas.

## Consequences
- Agents interacting with `code-kb` experience zero cognitive overhead and pass purely semantic parameters (`query`, `symbol_name`, `file_path`).
- Token consumption per tool call remains at the theoretical minimum.
- Global MCP installations work out of the box with zero agent friction.
