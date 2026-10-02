---
name: code-kb
description: Navigates code with the code-kb MCP server (symbol lookup, concept search, file skeletons, symbol bodies, callers and callees, test impact) at a fraction of the tokens of grep or full-file reads. Use when exploring unfamiliar code, finding a symbol or its signature, preparing to edit a function, or checking what a change affects.
---

# code-kb

code-kb indexes symbol names, signatures, and docstrings, not file contents. Use `rg` for
literal text: string literals, error messages, comments, and config values.

## `project_root`

Pass `project_root`, the absolute path of the project or git worktree you work in, on every
call except `telemetry_summary`. Send the same value each time. Change it when you move to a
worktree or another project. The calls below leave it out to stay short. The CLI takes it as
`--root`, which defaults to the current directory.

`path` and `file_path` are relative to `project_root`, or absolute inside it. A path outside
`project_root` is an error.

A project with no index gets one on the first call. If the answer is
`Indexing <root> started; call again in a few seconds.`, make the same call again.

## Pick the smallest tool that answers

| Need | MCP call | CLI |
|---|---|---|
| Directory layout and main exports | `codebase_outline(path?, depth=2)` | `code-kb outline [--path <p>]` |
| A file's interface, bodies stripped | `file_skeleton(file_path)` | `code-kb skeleton <file>` |
| A symbol by exact name or prefix | `lookup_symbol(query, path?)` | `code-kb lookup <query> [--path <p>]` |
| Symbols by concept (`"retry backoff"`) | `search_symbols(query, path?)` | `code-kb search <query> [--path <p>]` |
| One symbol's implementation | `get_symbol_body(symbol_name, file_path?)` | `code-kb body <symbol> [--file <f>]` |
| Body, callee signatures, parameter types, and tests before an edit | `get_symbol_context(symbol_name, file_path?)` | `code-kb context <symbol> [--file <f>]` |
| Callers, or callees with `direction="callees"` | `find_references(symbol_name, file_path?)` | `code-kb refs <symbol> [--direction callees]` |
| What a change affects and which tests to run | `blast_radius(symbol=...)`, `blast_radius(file=...)`, or `blast_radius()` for uncommitted changes | `code-kb blast-radius [target] [--file <f>]` |
| Routes, SQL, models, config keys | `find_structural_facts(category?)`; no category lists the categories | `code-kb facts [category]` |

Edit with the host's own file tools. code-kb refreshes the index when files change.

## Rules

- Use the parameter names from the tool schema.
- Lookup and search lines include `id=<symbol_id>`. Pass it as `symbol_id` (CLI `--symbol-id`) to `get_symbol_body`, `get_symbol_context`, `find_references`, or `blast_radius` to select one exact symbol. An edit or a rebuild can change the ID, so look it up again after one.
- For a name that several symbols share, such as `new`, pass `file_path` or a qualified name (`Server::new`).
- Lookup and search hide test code. Pass `is_test=true` (CLI `--include-tests`) to show it.
- Callee lists leave out standard library and runtime calls. Pass `include_external=true` (CLI `--include-external`) to show them.
- A caller found through an unresolved call is a best guess, and the list can miss callers. Read the code before you rely on what the list leaves out.

## Related skills

- `code-kb-telemetry`: token savings, call counts, and tool errors.
- `report-issue`: file a bug against code-kb with a diagnostic bundle.
