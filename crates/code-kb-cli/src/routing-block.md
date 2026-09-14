<!-- code-kb routing directive -->
## Code Intelligence: Always use `code-kb` MCP tools

`code-kb` is active in this workspace. It provides instant AST-indexed symbol queries, skeletons, and surgical context slices backed by SQLite in WAL mode (<15 MB RAM).

**CRITICAL ROUTING RULES (Saves 80-90% context tokens):**
- **DO NOT** run `grep`, `rg`, `find`, `cat`, or `view_file` to search symbols or discover interfaces.
- **DO NOT** supply `workspace`, `repo_path`, or `root_dir` parameters. Workspace binding is automatic.
- **DO NOT** read an entire file when you only need a function, class, or type signature.

### Quick Tool Routing:
1. **Repo / Subsystem Orientation:** `codebase_outline(subpath, depth)` instead of `ls -R` or `find .`.
2. **Module Interface:** `file_skeleton(file_path)` instead of reading the file. Bodies are stripped; signatures and types remain.
3. **Symbol Definition:** `lookup_symbol(query="symbol_name", path="optional/subpath")` for exact/prefix lookup.
4. **Natural Language / Concept Search:** `search_symbols(query="keyword concept", path="optional/subpath")` using FTS5 BM25.
5. **Function Body & Implementation:** `get_symbol_body(symbol_name, file_path)` to read only the target symbol.
6. **Editing Prep / Call Graph:** `get_symbol_context(symbol_name, file_path)` returns target body + immediate callee signatures + parameter types + tests in one call.
7. **Callers / References:** `find_references(symbol_name, direction="callers")` (default direction: "callers").
8. **Framework Facts (Routes, Queries, Models):** `find_structural_facts(category)` (omit category to list all available categories).
9. **Atomic Symbol Edits:** `replace_symbol_body(symbol_name, file_path, new_body, expected_body_hash)` verifies tree-sitter syntax, checks concurrency hash, edits file, and updates SQLite index in one turn.
10. **Blast Radius & Test Impact:** `blast_radius(symbol="name")`, `blast_radius(path="file")`, or `blast_radius()` (auto-detects uncommitted git changes) to compute multi-hop callers and predict likely tests to run before/after edits (alias: `impact`).
