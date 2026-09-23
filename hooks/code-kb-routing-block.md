<!-- code-kb routing directive -->
## Code Intelligence: Always use `code-kb` MCP tools

`code-kb` is active in this workspace. It provides instant AST-indexed symbol queries, skeletons, and surgical context slices backed by SQLite in WAL mode.

**CRITICAL ROUTING RULES (Saves 80-90% context tokens):**
- **DO NOT** run `grep`, `rg`, `find`, `cat`, or `view_file` to search symbols or discover interfaces.
- **DO NOT** supply `workspace`, `repo_path`, or `root_dir` parameters. Workspace binding starts from the configured root and can rebind from absolute paths, but a running MCP server cannot see host cwd changes. After entering a git worktree (e.g. Claude Code `EnterWorktree`), make the first code-kb call with an absolute path inside it, such as `file_skeleton(file_path="/worktree/path/to/file.rs")`; then unscoped lookups use that root. Binding is server-wide: use one server per concurrent root or include the target worktree's absolute path in calls.
- **DO NOT** read an entire file when you only need a function, class, or type signature.
- **DO** use `rg` for literal text: string literals, error messages, comments, and config values. `code-kb` indexes symbol names, signatures, and docstrings, not file contents.

### Quick Tool Routing:
1. **Repo / Subsystem Orientation:** `codebase_outline(path, depth)` instead of `ls -R` or `find .`.
2. **Module Interface:** `file_skeleton(file_path)` instead of reading the file. Bodies are stripped; signatures and types remain.
3. **Symbol Definition:** `lookup_symbol(query="symbol_name", path="optional/dir")` for exact/prefix lookup.
4. **Natural Language / Concept Search:** `search_symbols(query="keyword concept", path="optional/dir")` over names, signatures, and docstrings; substrings inside identifiers are found (`sha256` finds `parseSha256Sidecar`).
5. **Function Body & Implementation:** `get_symbol_body(symbol_name, file_path)` or `get_symbol_body(symbol_id)` using the `id=` returned by discovery.
6. **Editing Prep / Call Graph:** `get_symbol_context(symbol_name, file_path)` or `get_symbol_context(symbol_id)` returns target body + immediate callee signatures + parameter types + tests in one call.
7. **Callers / References:** `find_references(symbol_name, direction="callers")` or `find_references(symbol_id)` for exact selection. Pending calls remain heuristic.
8. **Framework Facts (Routes, Queries, Models):** `find_structural_facts(category)` (omit category to list all available categories).
9. **Blast Radius & Test Impact:** `blast_radius(symbol="name")`, `blast_radius(symbol_id="id")`, `blast_radius(file="file")`, or `blast_radius()` (auto-detects uncommitted git changes). An ID plus file is a guard (alias: `impact`).
