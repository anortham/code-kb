---
expect:
  project_root: /^\//
---

References to function `symbol_not_found_parts` (crates/code-kb-core/src/queries.rs:3062)
Sites: 4 · proven 3 · likely 1 · unresolved 0 · excluded 0
Text: `symbol_not_found_parts` is on 11 lines: 4 sites, 1 definition, 1 import, 5 text only.

Proven (3):
- `symbol_not_found` [crates/code-kb-core/src/ops.rs:137] (kind: calls; qualified path)
- `find_references_scoped` [crates/code-kb-core/src/queries.rs:3158] (kind: calls; same file)
- `compute_blast_radius_scoped_with_ids` [crates/code-kb-core/src/queries.rs:5736] (kind: calls; same file)
Likely (1):
- `select_reference_target` [crates/code-kb-core/src/reference_report.rs:52] (kind: calls; name only)
Text only (5): docs, comments, strings, or code the index did not record
  AGENTS.md: 137
  CLAUDE.md: 137
  crates/code-kb-core/src/reference_report.rs: 43
  docs/plans/2026-10-02-skill-authoring-alignment.md: 119
  docs/plans/2026-10-17-accuracy-base.md: 268
Not visible to this index: calls inside macro bodies; calls through dynamic dispatch reach overrides that this list does not show.
