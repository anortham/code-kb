# Exact symbol selection

Lookup and search print each full current SQLite `symbol_id` as `id=<symbol_id>` on the result's existing metadata line. Body, context, references, and blast radius accept it directly through `symbol_id` or `--symbol-id`.

MCP tool schemas describe the name/ID choice using optional string properties. Handlers require exactly one nonempty selector; JSON null placeholders are ignored. The schema omits top-level `oneOf` and `minLength` for Claude tool API compatibility.

An ID is reloaded after its owning file is refreshed. If it disappears, callers must run lookup or search again. An optional file is a canonical workspace-identity guard for an ID, never an additional blast-radius seed.

Resolved edges preserve the selected ID. Pending unresolved calls still use the existing name and receiver heuristics, so exact selection cannot make those calls overload-perfect.
