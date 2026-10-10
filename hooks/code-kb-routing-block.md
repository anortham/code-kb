<!-- code-kb routing directive -->
## Code navigation: use the `code-kb` MCP tools first

- Pass `project_root`, the absolute path of the project or git worktree you work in, on every code-kb call except `telemetry_summary`. Change it when you move to a worktree or another project.
- **Before you `Read`, `cat`, `head`, or `sed -n` a source file**, call `file_skeleton` on it, then `get_symbol_body` or `get_symbol_context` for the symbols you need. Read a whole source file only when you will change most of it.
- **To find a definition or its uses**, call `lookup_symbol` or `find_references`, not `rg` or `grep` on the name.
- **To find code by concept**, call `search_symbols`, not `rg`.
- **Before you change a symbol**, call `get_symbol_context`; to pick the tests to run, call `blast_radius`.
- Use `rg` for literal text: string literals, error messages, comments, and config values. `code-kb` indexes symbol names, signatures, and docstrings, not file contents.
