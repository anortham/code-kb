# Toolkit assessment

Reviewed 2026-09-22 against `ddd036d` and the local `refactor/read-only-tools` changes.

> **Current status (2026-09-22):** The findings below describe the pre-implementation state. The approved read-tool precision plan implements exact `symbol_id` selection, truthful blast-radius limits, and consistent test-path discovery; see [the plan](plans/2026-09-22-read-tool-precision.md). The original findings remain as historical rationale.
>
> **Update (2026-10-18):** Accuracy phase 2 removed `codebase_outline`, `search_symbols`, `find_structural_facts`, and `telemetry_summary` from `tools/list`. `file_skeleton` on a folder returns the outline, `lookup_symbol` adds ranked matches when no name matches, and the CLI keeps `outline`, `search`, `facts`, and `stats`. The table and the counts below describe the seven current tools; `check_edit` (CLI `code-kb check`) was added 2026-10-19.

## Verdict

The 7 exposed MCP tools form a cohesive read-only code-intelligence toolkit. Keep their boundaries. The next investment should make target selection and result completeness consistent across tools. There is no evidence here that another write tool, a broad new feature, or merging existing tools would help more.

| Agent task | Existing tools | Assessment |
| --- | --- | --- |
| Orient and inspect interfaces | `file_skeleton` | A folder returns its outline, so one tool covers the progression from repository to file. |
| Locate code | `lookup_symbol` | One entry point serves a known identifier and a concept; ranked matches follow when no name matches. |
| Read or prepare a native edit | `get_symbol_body`, `get_symbol_context` | Discovery exposes a current-index `symbol_id` for exact selection; a body alone is a cheaper, useful subset of the context bundle. Native filesystem tools perform the write; indexed files refresh automatically after filesystem changes. |
| Trace dependencies and choose tests | `find_references`, `blast_radius` | Immediate relationships and multi-hop impact answer different questions. |
| Check an edit before finishing | `check_edit` | Lists the symbols whose shape changed against the session base and the sites the diff did not reach; CLI `code-kb check`. |
| Inspect extracted patterns and literals | CLI `code-kb facts` | Path-scoped category aliases make framework facts available in the terminal; no MCP tool. |
| Inspect usage | CLI `code-kb stats` | Useful operational support, separate from code understanding; no MCP tool. |

All 7 have CLI counterparts. Additional CLI commands such as `scan`, `serve`, and `bug-report` do not violate the one-way promise that every MCP tool is verifiable from the terminal. Lookup and search became one tool; the decision to retain separate body and context tools still holds.

## Priorities

### 1. Carry precise target identity through the workflow

**Strength: strong.** Lookup supports a path and kind filter, but subsequent tools do not accept the same selectors. References accept a path filter, but even a file and qualified name cannot distinguish same-name, same-kind overloads within one class. Precise overload selection is the main missing capability.

Consider returning a reusable symbol identifier that downstream tools accept, or a consistent file-and-location selector. Choose one convention across body, context, references, and impact. This should deepen the existing tools rather than add another discovery tool.

Evidence: `crates/code-kb-cli/src/mcp/server.rs`, `tool_definitions`; `crates/code-kb-core/src/queries.rs`, `get_symbol_by_name_internal`.

### 2. Make blast-radius limits truthful

**Strength: strong.** Context labels its dependency and related-test caps, and search and lookup report returned counts. Blast-radius output still risks implying completeness: a small requested limit can report only that many results found even when a larger request returns more. A short impact result should not imply complete coverage.

Make blast-radius truncation and continuation information explicit. Treat predicted tests as suggestions, not proof that no other tests are needed.

Evidence: `crates/code-kb-core/src/queries.rs`, `compute_blast_radius`; `crates/code-kb-core/src/formatters.rs`, impact formatters.

### 3. Use consistent test-path recognition

**Strength: strong.** `blast_radius` recognizes test paths that `get_symbol_context` does not consistently include in its related-test output. The two tools should apply the same test-path conventions.

Use the shared path rule for context test discovery so a targeted context slice does not omit tests that blast-radius can predict.

Evidence: `crates/code-kb-core/src/ops.rs`, `get_context_slice_op`; `crates/code-kb-core/src/queries.rs`, test-path predicates.

## What to defer

Do not add rename/refactoring, a test runner, embeddings, batch operations, or another general search mode without a demonstrated failed workflow. Native terminal tools remain appropriate for running tests and for text outside indexed facts. Markdown headings and configuration structures are already indexed; treating all non-code files as a missing capability would be incorrect.

Measure successful end-to-end agent tasks, including overloaded names, native edits, failed indexing, and large results. Transport success rates and estimated token savings alone do not establish answer quality. The workspace telemetry inspected here includes prior testing and cannot be treated as an unbiased adoption study.

Top recommendation: make the existing locate → inspect → trace → native-edit flow preserve precise identity and disclose incomplete results before expanding the catalog.
