---
name: code-kb
description: Use when exploring unfamiliar code, inspecting types or function signatures, finding symbols across the codebase, obtaining surgical context before modifying a function, or performing atomic AST-validated symbol edits.
---

# code-kb: Token-Dense Code Intelligence

`code-kb` provides progressive disclosure, semantic symbol navigation, and surgical context slicing. Always prefer `code-kb` MCP tools over raw filesystem grep or full-file reads to save 80–90% of token consumption.

## 4-Phase Progressive Disclosure Workflow

### 1. Orientation (~200 tokens)
When entering a new repository, unfamiliar subsystem, or crate:
* Call `codebase_outline(path, depth)` to inspect directories and primary exports.
* **Never** run wide directory recursion or `ls -R`.

### 2. Interface Discovery
When inspecting how a module or component is shaped:
* Call `file_skeleton(file_path)` to inspect structs, traits, methods, signatures, and docstrings with implementation bodies stripped.
* Call `find_symbol(query="...")` for exact or prefix symbol name matching across the entire codebase.
* Call `search_symbols(query="...")` for natural-language / conceptual search (e.g. `"parse tokens"`, `"auth middleware"`) using SQLite FTS5 BM25 ranking.
* **Never** read a 500-line source file just to look up a signature or type definition.

### 3. Surgical Context
Before modifying or understanding a specific function/method:
* Call `get_context_slice(symbol_name, file_path)` to get:
  1. The target function's full implementation body.
  2. Immediate callee signatures and return types.
  3. Parameter type definitions.
  4. Associated unit tests.
* Call `find_references(symbol_name, direction="callers")` to check callers before changing a public signature.

### 4. Atomic Symbol Edits
When modifying an existing function or method:
* Call `replace_symbol_body(symbol_name, file_path, new_body, expected_body_hash)`.
* Performs pre-flight tree-sitter syntax validation before touching disk.
* Checks optimistic concurrency hash to avoid overwriting conflicting edits.
* Re-indexes SQLite AST facts in a single atomic turn.

## Quick Reference

| Task | Anti-Pattern | `code-kb` Pattern |
|---|---|---|
| Explore repo structure | `find .`, `ls -R` | `codebase_outline()` |
| Inspect module interface | `view_file(large_file.rs)` | `file_skeleton(file_path)` |
| Locate function by name | `grep -rn "fn do_work"` | `find_symbol(query="do_work")` |
| Search by concept | `grep -rn "retry"` | `search_symbols(query="retry backoff")` |
| Prep for editing a function | Read caller/callee files | `get_context_slice(symbol_name)` |
| Trace callers | Text grep for call sites | `find_references(symbol_name, "callers")` |
| Edit implementation | Multi-line search/replace | `replace_symbol_body(...)` |

## Core Invariants

* **No Workspace Parameters:** Never supply or request `workspace`, `repo_path`, or `workspace_id`. The server binds to workspace root automatically.
* **Disambiguation:** If a symbol name is overloaded (e.g. `new`), supply `file_path` or qualified name (e.g. `Server::new`).
