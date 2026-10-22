---
name: code-kb
description: Navigates code with the code-kb MCP server (symbol lookup, concept search, file skeletons, symbol bodies, callers and callees, test impact, post-edit check) at a fraction of the tokens of grep or full-file reads. Use when exploring unfamiliar code, finding a symbol or its signature, preparing to edit a function, checking what a change affects, or finishing an edit.
---

# code-kb

code-kb indexes symbol names, signatures, and docstrings, not file contents. Use `rg` for
literal text: string literals, error messages, comments, and config values.

Before you `Read`, `cat`, or `sed -n` a source file, call `file_skeleton` on it, then
`get_symbol_body` or `get_symbol_context` for the symbols you need. Read a whole source file
only when you will change most of it. Find a definition or its uses with `lookup_symbol` or
`find_references`, not `rg` on the name.

## `project_root`

Pass `project_root`, the absolute path of the project or git worktree you work in, on every
call. Send the same value each time. Change it when you move to a
worktree or another project. The calls below leave it out to stay short. The CLI takes it as
`--root`, which defaults to the current directory.

`path` and `file_path` are relative to `project_root`, or absolute inside it. A path outside
`project_root` is an error.

A project with no index gets one on the first call. If the answer is
`Indexing <root> started; call again in a few seconds.`, make the same call again.

## Pick the smallest tool that answers

| Need | MCP call | CLI |
|---|---|---|
| A file's interface, bodies stripped; a folder returns its layout and main exports | `file_skeleton(file_path)` | `code-kb skeleton <file>`; `code-kb outline [--path <p>]` for a folder |
| A symbol by name or prefix, or code by concept (`"retry backoff"`); a query that names no symbol exactly also gets ranked matches | `lookup_symbol(query, path?)` | `code-kb lookup <query> [--path <p>]` |
| One symbol's implementation | `get_symbol_body(symbol_name, file_path?)` | `code-kb body <symbol> [--file <f>]` |
| Body, callee signatures, parameter types, and tests before an edit | `get_symbol_context(symbol_name, file_path?)` | `code-kb context <symbol> [--file <f>]` |
| Every site a change must reach: callers, overrides, constructions, enum variant uses, registrations, and the calling tests with run commands; or callees with `direction="callees"` | `find_references(symbol_name, file_path?)` | `code-kb refs <symbol> [--direction callees]` |
| What a change affects and which tests to run | `blast_radius(symbol=...)`, `blast_radius(file=...)`, or `blast_radius()` for uncommitted changes | `code-kb blast-radius [target] [--file <f>]` |
| After you edit: the symbols whose shape changed (removed, renamed, parameters, signature, fields, enum variants) and the sites your diff did not reach | `check_edit(base?)` | `code-kb check [--base REV]` |

Edit with the host's own file tools. code-kb refreshes the index when files change. Call
`check_edit` before you say the change is done.

Three CLI commands have no MCP tool. `code-kb facts [category]` lists routes, SQL, models, and config keys; no category lists the categories. `code-kb search <query> [--explain]` prints the ranked matches with their score breakdown. `code-kb stats` reports token savings and tool errors.

## Rules

- Use the parameter names from the tool schema.
- Lookup lines include `id=<symbol_id>`. Pass it as `symbol_id` (CLI `--symbol-id`) to `get_symbol_body`, `get_symbol_context`, `find_references`, or `blast_radius` to select one exact symbol. An edit or a rebuild can change the ID, so look it up again after one.
- For a name that several symbols share, such as `new`, pass `file_path` or a qualified name (`Server::new`).
- Lookup hides test code. Pass `is_test=true` (CLI `--include-tests`) to show it.
- Callee lists leave out standard library and runtime calls. Pass `include_external=true` (CLI `--include-external`) to show them.
- After the sites, `find_references` lists `Overrides`, `Overrides of`, `Constructions`, `Variant uses`, `Registrations`, and `Tests that call it` (each test with a `run:` command) when they have rows. Read them before you change a method, a class, or an enum. They never change a site's label.
- `blast_radius` lists only the tests that the index can reach. `### Open Ends` names symbols that no indexed code calls, such as a method that the runtime or a framework calls; the list does not show the tests that reach the target through them. A list that ends with ``A `depth` of 5 lists N more tests`` stopped at the depth. With either warning, run the wider test suite before you say the change is done.
- Each caller site has a label. `proven` sites have index evidence. A call shows its argument count (`; 2 args`); a `likely` or `unresolved` call whose count no definition of the target accepts is `excluded` with the range (`3 args; `render` takes 1-2`). Read the code before you rely on a `likely` or `unresolved` site. The `Text only` lines and the `Not visible to this index` line show what the index did not record.

- `check_edit` compares the working tree with `base`, else the commit the session started at, else `HEAD`. Under `Not in your diff`, `proven` and `likely` sites still use the old shape: edit them, or say why they need no change. `Unresolved outside the diff` is a count; read those sites with `find_references`. The `Stop` and `SubagentStop` hooks run the same check and block the stop once for each diff that leaves a `proven` site outside it, except for a `signature changed` row or a JavaScript parameter change.

## Related skills

- `code-kb-telemetry`: token savings, call counts, and tool errors.
- `report-issue`: file a bug against code-kb with a diagnostic bundle.
