<!-- code-kb routing directive -->
## Code navigation: use the `code-kb` MCP tools first

- Pass `project_root`, the absolute path of the project or git worktree you work in, on every code-kb call. Change it when you move to a worktree or another project.
- **To orient**, call `file_skeleton` on a folder or a file you do not know.
- **Before you change a symbol**, call `get_symbol_context`: it carries the body, callers, tests to run, and types.
- **Read the file you will edit**, since the editor needs it.
- **To find a definition or its uses**, call `lookup_symbol` or `find_references`, not `rg` or `grep` on the name.
- **To find code by concept**, call `lookup_symbol` with a few words; it adds ranked matches when no name matches.
- **After you edit**, call `check_edit` before you say the change is done; `blast_radius()` lists the tests of the whole diff.
- Use `rg` for literal text: string literals, error messages, comments, and config values. `code-kb` indexes symbol names, signatures, and docstrings, not file contents.
