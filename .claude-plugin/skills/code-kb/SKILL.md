---
name: code-kb
description: Use when exploring unfamiliar code, inspecting types or function signatures, finding symbols across the codebase, obtaining surgical context before modifying a function, or checking tool telemetry and token savings.
---

# code-kb: Token-Dense Code Intelligence

`code-kb` provides progressive disclosure, semantic symbol navigation, and surgical context slicing. Always prefer `code-kb` MCP tools over raw filesystem grep or full-file reads to save 80–90% of token consumption. For literal text (string literals, error messages, comments, config values) use `rg`; `code-kb` indexes symbol names, signatures, and docstrings, not file contents.

## Required `project_root`

Pass `project_root`, the absolute path of the project or git worktree you work in, on every tool call except `telemetry_summary`. Send the same value on every call. Change it when you move to a worktree or another project. For example: `lookup_symbol(project_root="/path/to/project", query="do_work")`. The calls below leave out `project_root` to stay short. The CLI takes the same value as the global `--root` flag, which defaults to the current directory.

`path` and `file_path` are relative to `project_root`, or absolute inside it. An absolute path outside `project_root` is an error. A path never switches the project.

A project with no index gets one on the first call. The call waits up to 5 s. If the index is not ready, the answer is `Indexing <root> started; call again in a few seconds.` Make the same call again.

## 4-Phase Progressive Disclosure Workflow

### 1. Orientation (a few hundred tokens)
When entering a new repository, unfamiliar subsystem, or crate:
* Call `codebase_outline(path="...", depth=2)` to inspect directories and primary exports.
* **Never** run wide directory recursion or `ls -R`.

### 2. Interface Discovery
When inspecting how a module or component is shaped:
* Call `file_skeleton(file_path)` to inspect structs, traits, methods, signatures, and docstrings with implementation bodies stripped. A directory path returns its outline instead.
* Call `lookup_symbol(query="...", path="optional/dir")` for exact or prefix symbol name matching across the entire codebase or scoped to a directory/file. Test functions, test containers, and rows from test files are hidden unless `is_test` is true, except a row whose name equals the query.
* Call `search_symbols(query="...", path="optional/dir")` for natural-language / conceptual search (e.g. `"parse tokens"`, `"auth middleware"`) over names, signatures, and docstrings; substrings inside identifiers are found (`sha256` finds `parseSha256Sidecar`).
* **Never** read a 500-line source file just to look up a signature or type definition.

### 3. Surgical Context
Before modifying or understanding a specific function/method:
* Call `get_symbol_context(symbol_name, file_path)` to get:
  1. The target function's full implementation body.
  2. Immediate callee signatures and return types.
  3. Parameter type definitions.
  4. Associated unit tests.
* Discovery output includes `id=<symbol_id>`. Call `get_symbol_body(symbol_name, file_path)` or select that current-index ID with `symbol_id` / CLI `--symbol-id`.
* Call `find_references(symbol_name, file_path?)` or `find_references(symbol_id)` (or `direction="callees"`) to check callers/callees. Exact selection preserves resolved edges; unresolved pending calls remain heuristic. Callee search excludes external stdlib/runtime tokens across supported languages; pass `include_external=true` to view external runtime calls.
* Call `blast_radius(symbol="...")`, `blast_radius(symbol_id="...")`, `blast_radius(file="...")`, or `blast_radius()` (auto-detects uncommitted git changes). An ID plus file checks identity and does not seed the whole file. IDs are reselected after edits or rebuilds. (Tool alias: `impact`).
* Call `find_structural_facts()` to list all detected framework categories, or `find_structural_facts(category="route")` to query specific routes, SQL queries, models, or config keys.

### 4. Native Editing
After using the smallest reading tool that supplies the needed context, edit through the agent's native filesystem tools. `code-kb` automatically refreshes indexed files after filesystem changes, so the index remains current without an edit tool.

## Telemetry & Diagnostics

`code-kb` records lightweight invocation telemetry, token consumption, and token savings in `~/.code-kb/telemetry.db` across all sessions.

### Answering Token Savings and Tool Usage Inquiries
When a user asks questions such as *"how many tokens has code-kb saved me this month?"* or *"what is my token efficiency this week?"*:
* Call `telemetry_summary(time_window="month")` (valid windows: `"today"`, `"7d"`, `"30d"`, `"month"`, `"year"`, `"all"`; default is `"all"`).
* Pass `workspace_only=true` if the user wants metrics scoped to one project instead of global history across all projects. `telemetry_summary` takes no `project_root`, so the scope is the project of the most recent code-kb call.
* Report back the summarized numbers: total tool calls, successful vs failed calls, estimated tokens consumed, estimated tokens saved, and the net efficiency multiplier.
* Say what "saved" means: the size, in estimated tokens, of the files the answer points into, minus the tokens served. A skeleton, body, or context read is measured against its own file. A lookup, search, references, blast-radius, or facts answer is measured against the distinct files its rows name, at most 20 files. A call with no file to point at, such as an outline or a telemetry summary, records no baseline and is not counted as saved. An `Indexing ... started` answer records no baseline either.
* The summary states coverage beside the number: `Est. Tokens Saved: ~N (baseline known for K of M calls)`, and `~N (K/M)` per tool. Report the coverage with the number.

### Generating Bug Reports & Diagnosing Failures
When diagnosing unexpected tool errors or when assisting a user with filing an issue:
* Call `telemetry_summary()` to inspect recent error counts and failure rates across tools.
* Run `code-kb bug-report --title "..." --description "..." [--logs N] [--json]` via the terminal to produce a self-contained diagnostic markdown bundle: platform (OS, arch, both binary versions), index facts (extractor, schema, level, file and symbol counts), the last 10 tool errors, and the last N log lines (default 40) with home paths masked, along with a pre-filled GitHub issue URL. The `/report-issue` skill walks through filing it with `gh issue create`.
* Use `code-kb bug-report --json` or `code-kb stats --json` when programmatic or machine-readable diagnostics are needed.

## Quick Reference

| Task | Anti-Pattern | MCP Tool Call | CLI Equivalent |
|---|---|---|---|
| Explore repo structure | `find .`, `ls -R` | `codebase_outline()` | `code-kb outline` |
| Inspect module interface | `view_file(large_file.rs)` | `file_skeleton(file_path)` | `code-kb skeleton <file>` |
| Locate function by name | `grep -rn "fn do_work"` | `lookup_symbol(query="do_work", path="...")` | `code-kb lookup <query> [--path <p>]` |
| Search by concept | `grep -rn "retry"` | `search_symbols(query="retry backoff", path="...")` | `code-kb search <query> [--path <p>]` |
| Read function body | Full file read | `get_symbol_body(symbol_name)` | `code-kb body <symbol>` |
| Prep for editing function | Read caller/callee files | `get_symbol_context(symbol_name)` | `code-kb context <symbol> [--include-external]` |
| Trace callers / callees | Text grep for call sites | `find_references(symbol_name, file_path?)` | `code-kb refs <symbol> [--file <f>] [--include-external]` |
| Assess impact & find tests | Wide test suite runs | `blast_radius(symbol="...")` | `code-kb blast-radius [target]` |
| Discover routes / models | Search string literals | `find_structural_facts(category="route")` | `code-kb facts [category]` |
| Edit a file | Read the needed context | Native agent filesystem tools | Native editor or patch tool |
| Check token savings & usage | Guesswork, parsing logs | `telemetry_summary(time_window="month")` | `code-kb stats [--since <window>] [--workspace]` |
| Generate diagnostic bug report | Manual system info triage | `telemetry_summary()` (for errors) | `code-kb bug-report [--title <title>]` |
| Find literal text, error strings, comments | `search_symbols` (indexes symbols only) | none | `rg "exact text"` |

## Parameter Aliases & Safe Defaults

Use the canonical names from the MCP schema in tool calls. The aliases below are backend tolerance for hand-written calls; strict MCP clients can reject alias-only requests.

* `project_root`: required by every tool except `telemetry_summary`. An absolute path or a `file://` URI. A subfolder resolves to the enclosing project.
* `symbol_name`: accepts `symbol`, `name`.
* `file_path`: accepts `file`, `path`.
* `query`: accepts `name`, `q`, `symbol_name`, `symbol`.
* `codebase_outline.path`: accepts `subpath`, `dir`.
* `direction` in `find_references`: defaults to `"callers"`.
* `include_external` in `find_references` & `get_symbol_context`: defaults to `false` (filters runtime noise).
* `blast_radius`: accepts `symbol`/`name`, `path`/`file`, `depth`/`max_depth`, `limit`. When target is omitted, automatically discovers uncommitted working-tree changes via git. Alias: `impact`.
* `category` in `find_structural_facts`: optional (omitting lists all detected categories and counts). Aliases: `sql`/`query`/`queries`, `route`/`routes`, `config`, `model`/`models`, `signal`/`signals`, `import`/`imports`, `binding`/`bindings`, `component`/`components`, `module`/`modules`, `pragma`, `property`/`properties`. Any other value matches pattern ids by substring.
* `path` in `lookup_symbol` / `search_symbols` / `find_structural_facts`: optional filter by directory or file path prefix, relative to `project_root` or absolute inside it.
* `file_path` in `get_symbol_body` / `get_symbol_context` / `find_references`: optional file path to disambiguate symbols with identical names across files, relative to `project_root` or absolute inside it.
* `telemetry_summary`: takes no `project_root`. Accepts `time_window` (aliases: `since`, `window`; defaults to `"all"`), `workspace_only` (defaults to `false`), `json` (defaults to `false`).

## Core Invariants

* **`project_root` on Every Call:** Pass the absolute path of the project or git worktree you work in as `project_root` on every call except `telemetry_summary`. Never supply `workspace`, `repo_path`, or `workspace_id`.
* **Disambiguation:** If a symbol name is overloaded (e.g. `new`), supply `file_path` or qualified name (e.g. `Server::new` or `Alpha::create`).
* **Zero Heap Footprint:** All queries stream directly from SQLite; retained memory is about 25 MB.
* **Self-Cleaning Workspaces & Central Telemetry:** Each workspace or git worktree maintains its isolated database at `<root>/.code-kb/artifact.db`, cleaned automatically upon repo/worktree removal. Durable tool telemetry and token efficiency metrics persist centrally at `~/.code-kb/telemetry.db`.
