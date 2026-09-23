<!-- code-kb routing directive -->
## Code Intelligence: Always use `code-kb` MCP tools

`code-kb` is active in this workspace. It provides instant AST-indexed symbol queries, skeletons, and surgical context slices backed by SQLite in WAL mode.

**CRITICAL ROUTING RULES (Saves 80-90% context tokens):**
- **DO** pass `project_root`, the absolute path of the project or git worktree you work in, on every code-kb call except `telemetry_summary`. Send the same value each time. Change it when you move to a worktree or another project.
- **DO NOT** run `grep`, `rg`, `find`, `cat`, or `view_file` to search symbols or discover interfaces.
- **DO NOT** read an entire file when you only need a function, class, or type signature.
- **DO** use `rg` for literal text: string literals, error messages, comments, and config values. `code-kb` indexes symbol names, signatures, and docstrings, not file contents.

### Quick Tool Routing:
Every call below also takes `project_root`, for example `lookup_symbol(project_root="/path/to/project", query="symbol_name")`. `path` and `file_path` are relative to `project_root`, or absolute inside it.
1. **Repo / Subsystem Orientation:** `codebase_outline(path, depth)` instead of `ls -R` or `find .`.
2. **Module Interface:** `file_skeleton(file_path)` instead of reading the file. Bodies are stripped; signatures and types remain.
3. **Symbol Definition:** `lookup_symbol(query="symbol_name", path="optional/dir")` for exact/prefix lookup.
4. **Natural Language / Concept Search:** `search_symbols(query="keyword concept", path="optional/dir")` over names, signatures, and docstrings; substrings inside identifiers are found (`sha256` finds `parseSha256Sidecar`).
5. **Function Body & Implementation:** `get_symbol_body(symbol_name, file_path)` or `get_symbol_body(symbol_id)` using the `id=` returned by discovery.
6. **Editing Prep / Call Graph:** `get_symbol_context(symbol_name, file_path)` or `get_symbol_context(symbol_id)` returns target body + immediate callee signatures + parameter types + tests in one call.
7. **Callers / References:** `find_references(symbol_name, direction="callers")` or `find_references(symbol_id)` for exact selection. Pending calls remain heuristic.
8. **Framework Facts (Routes, Queries, Models):** `find_structural_facts(category)` (omit category to list all available categories).
9. **Blast Radius & Test Impact:** `blast_radius(symbol="name")`, `blast_radius(symbol_id="id")`, `blast_radius(file="file")`, or `blast_radius()` (auto-detects uncommitted git changes). An ID plus file is a guard (alias: `impact`).
