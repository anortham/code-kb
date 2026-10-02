---
expect:
  project_root: /^\//
---

Callers of `symbol_not_found_parts` (crates/code-kb-core/src/queries.rs:2210), 3 rows:
- function `symbol_not_found_message` (crates/code-kb-core/src/queries.rs:2290) call
- function `lookup_symbol` (crates/code-kb-core/src/queries.rs:880) call
- function `get_symbol_body` (crates/code-kb-core/src/queries.rs:1430) call
Imported by 0 files.
