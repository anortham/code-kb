---
name: code-kb
description: Use when exploring unfamiliar code, inspecting types or function signatures, finding symbols across the codebase, obtaining surgical context before modifying a function, or performing atomic AST-validated symbol edits.
---

# code-kb: Token-Dense Code Intelligence

`code-kb` provides progressive disclosure, semantic symbol navigation, and surgical context slicing. Always prefer `code-kb` MCP tools over raw filesystem grep or full-file reads to save 80–90% of token consumption.

## 4-Phase Progressive Disclosure Workflow

### 1. Orientation (~200 tokens)
When entering a new repository, unfamiliar subsystem, or crate:
* Call `codebase_outline(subpath, depth)` to inspect directories and primary exports.
* **Never** run wide directory recursion or `ls -R`.

### 2. Interface Discovery
When inspecting how a module or component is shaped:
* Call `file_skeleton(file_path)` to inspect structs, traits, methods, signatures, and docstrings with implementation bodies stripped.
* Call `find_symbol(query="...", path="optional/subpath")` for exact or prefix symbol name matching across the entire codebase or scoped to a directory/file.
* Call `search_symbols(query="...", path="optional/subpath")` for natural-language / conceptual search (e.g. `"parse tokens"`, `"auth middleware"`) using SQLite FTS5 BM25 ranking.
* **Never** read a 500-line source file just to look up a signature or type definition.

### 3. Surgical Context
Before modifying or understanding a specific function/method:
* Call `get_context_slice(symbol_name, file_path)` to get:
  1. The target function's full implementation body.
  2. Immediate callee signatures and return types.
  3. Parameter type definitions.
  4. Associated unit tests.
* Call `get_symbol_body(symbol_name, file_path)` if only the exact implementation body is needed.
* Call `find_references(symbol_name)` (or `direction="callees"`) to check callers/callees. Callee search excludes external stdlib/runtime tokens language-agnostically across all ~40 supported languages; pass `include_external=true` to view external runtime calls.
* Call `blast_radius(symbol="...")` or `blast_radius(path="...")` or `blast_radius()` (auto-detects uncommitted git changes) to calculate multi-hop transitive callers and pinpoint targeted tests to run before/after editing. (Tool alias: `impact`).
* Call `find_structural_facts()` to list all detected framework categories, or `find_structural_facts(category="route")` to query specific routes, SQL queries, models, or config keys.

### 4. Atomic Symbol Edits
When modifying an existing function or method:
* Call `replace_symbol_body(symbol_name, file_path, new_body, expected_body_hash)`.
* Performs pre-flight tree-sitter syntax validation before touching disk.
* Checks optimistic concurrency hash to avoid overwriting conflicting edits.
* Re-indexes SQLite AST facts in a single atomic turn.

## Quick Reference

| Task | Anti-Pattern | MCP Tool Call | CLI Equivalent |
|---|---|---|---|
| Explore repo structure | `find .`, `ls -R` | `codebase_outline()` | `code-kb outline` |
| Inspect module interface | `view_file(large_file.rs)` | `file_skeleton(file_path)` | `code-kb skeleton <file>` |
| Locate function by name | `grep -rn "fn do_work"` | `find_symbol(query="do_work", path="...")` | `code-kb symbol <query> [--path <p>]` |
| Search by concept | `grep -rn "retry"` | `search_symbols(query="retry backoff", path="...")` | `code-kb search <query> [--path <p>]` |
| Read function body | Full file read | `get_symbol_body(symbol_name)` | `code-kb body <symbol>` |
| Prep for editing function | Read caller/callee files | `get_context_slice(symbol_name)` | `code-kb slice <symbol> [--include-external]` |
| Trace callers / callees | Text grep for call sites | `find_references(symbol_name)` | `code-kb refs <symbol> [--include-external]` |
| Assess impact & find tests | Wide test suite runs | `blast_radius(symbol="...")` | `code-kb blast-radius [target]` |
| Discover routes / models | Search string literals | `find_structural_facts(category="route")` | `code-kb facts [category]` |
| Edit implementation | Multi-line search/replace | `replace_symbol_body(...)` | `code-kb edit <symbol> --file <f> --body <b>` |

## Parameter Flexibility & Aliases

To provide a zero-friction happy path, all MCP tools support common aliases and intelligent defaults:
* `symbol_name`: accepts `symbol`, `name`.
* `file_path`: accepts `file`, `path`.
* `query`: accepts `name`, `q`.
* `new_body`: accepts `body`, `code`, `content`.
* `expected_body_hash`: accepts `body_hash`, `expected_hash`.
* `subpath`: accepts `path`, `dir`.
* `direction` in `find_references`: defaults to `"callers"`.
* `include_external` in `find_references` & `get_context_slice`: defaults to `false` (filters noise across all ~40 languages).
* `blast_radius`: accepts `symbol`/`name`, `path`/`file`, `depth`/`max_depth`, `limit`. When target is omitted, automatically discovers uncommitted working-tree changes via git. Alias: `impact`.
* `category` in `find_structural_facts`: optional (omitting lists all detected categories and counts).
* `path` in `find_symbol` / `search_symbols`: optional filter by directory or file path prefix.

## Core Invariants

* **No Workspace Parameters:** Never supply or request `workspace`, `repo_path`, or `workspace_id`. The server binds to workspace root automatically.
* **Disambiguation:** If a symbol name is overloaded (e.g. `new`), supply `file_path` or qualified name (e.g. `Server::new` or `Alpha::create`).
* **Zero Heap Footprint:** All queries stream directly from SQLite; retained memory is strictly < 15 MB.
