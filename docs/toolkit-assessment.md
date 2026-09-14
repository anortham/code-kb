# Toolkit assessment

Reviewed 2026-09-14 against `main` at `258e895`, plus the local corrections described below.

## Verdict

The 11 exposed MCP tools form a cohesive code-intelligence toolkit. Keep their boundaries. The next investment should make target selection and result completeness consistent across tools. There is no evidence here that merging tools or adding a broad new feature would help more.

| Agent task | Existing tools | Assessment |
| --- | --- | --- |
| Orient and inspect interfaces | `codebase_outline`, `file_skeleton` | Useful progression from repository to file. |
| Locate code | `lookup_symbol`, `search_symbols` | Known identifiers and unknown concepts deserve separate entry points. |
| Read or prepare an edit | `get_symbol_body`, `get_symbol_context` | A body alone is a cheaper, useful subset of the context bundle. |
| Trace dependencies and choose tests | `find_references`, `blast_radius` | Immediate relationships and multi-hop impact answer different questions. |
| Inspect extracted patterns and literals | `find_structural_facts` | Useful underlying data, but category names and filtering need attention. |
| Change an existing symbol | `replace_symbol_body` | Fits the inspection flow; body hashes are available in read results. |
| Inspect usage | `telemetry_summary` | Useful operational support, separate from code understanding. |

All 11 have CLI counterparts. Additional CLI commands such as `scan`, `serve`, and `bug-report` do not violate the one-way promise that every MCP tool is verifiable from the terminal. The previous decision to retain separate lookup/search and body/context tools still holds.

## Priorities

### 1. Carry precise target identity through the workflow

**Strength: strong.** Lookup supports a path and kind filter, but subsequent tools do not accept the same selectors. References have no file selector. Even a file and qualified name cannot distinguish same-name, same-kind overloads within one class. Qualification resolution currently considers the immediate parent rather than the complete namespace chain.

Consider returning a reusable symbol identifier that downstream tools accept, or a consistent file-and-location selector. Choose one convention across body, context, references, impact, and edit. This should deepen the existing tools rather than add another discovery tool.

Evidence: `crates/code-kb-cli/src/mcp/server.rs`, `tool_definitions`; `crates/code-kb-core/src/queries.rs`, `get_symbol_by_name_internal`. Two related implementation defects were found and corrected during this review, listed below.

### 2. Make incomplete and stale answers visible

**Strength: strong.** Context requests cap dependencies at 10 and related tests at 5 without labeling those caps. Search and lookup report the returned count without saying whether more matches exist. Impact queries also have an internal 200-row ceiling before their requested output limit. A short result should not imply complete coverage.

Use consistent returned-count, truncation, and continuation information. Keep exact edit bodies intact. Treat predicted tests as suggestions, not proof that no other tests are needed.

Broad CLI queries read the existing index; they do not run the file freshness guard used by body and skeleton reads. MCP watchers and reconciliation improve freshness, but reconciliation can leave failed files behind while successful siblings update. An agent cannot currently inspect this index health through the catalog.

The only new tool worth considering from this review is a compact `index_status` reporting coverage, pending or failed updates, and recovery guidance. First decide whether this belongs in existing result metadata. Do not add a workspace registry or require workspace arguments.

Evidence: `ops.rs`, `get_context_slice_op`; `queries.rs`, `compute_blast_radius`; `formatters.rs`, search/context formatters; `sync.rs`, `reconcile`; `crates/code-kb-cli/src/main.rs`, read command branches.

### 3. Improve structural-fact ergonomics

**Strength: worth exploring.** The discovery call listed 130 `toml.key_value.v1` facts in this repository, while `category="config"` returned no matches. Category matching searches extractor pattern names and node metadata; it is not a normalized framework taxonomy. The tool also lacks a path filter.

Lead with the existing category-discovery call, document actual category semantics, and consider path and literal-text filters before adding route/config-specific tools. A repository with no route facts is not evidence that route extraction is universally unsupported.

Evidence: live MCP probes; `queries.rs`, `find_structural_facts` and `find_literals`.

### 4. Decide the policy for unsupported syntax grammars

**Strength: worth exploring after the documentation correction.** Indexing supports more languages than local syntax validation. Validation currently covers Rust, JavaScript, TypeScript, TSX, Python, and Go. Other extensions can be edited and return `Syntax: Skipped`, an intentional existing behavior. The original tool description's unconditional validation promise was broader than the implementation.

The descriptions now make this distinction visible before the call. Whether unsupported-language edits should fail closed is a product decision; adding dozens of parser dependencies is not a necessary first step.

Evidence: `syntax.rs`, `detect_grammar` and `validate_syntax`; `formatters.rs`, edit-result formatting; MCP edit description.

## Corrections made during the assessment

- Qualified impact targets now resolve to symbol IDs before graph traversal. Previously `McpServer::handle_call_tool_inner` reported no downstream callers while its bare name returned two. Unknown or ambiguous symbol targets now return errors.
- Qualified name resolution now applies the existing preference for primary definitions. Previously `JsonRpcResponse::error` was ambiguous between a field and method even with its file supplied, while the equivalent bare name selected the method.
- JSON skeleton reads now propagate refresh failures instead of returning stale symbols as a successful result.
- Published MCP examples use canonical schema parameters; aliases remain backend tolerance and may be rejected by client-side schema validation.
- Descriptions now identify conditional syntax validation and whole-file impact seeds. Automatic impact uses `git status`, not changed line ranges.

Regression tests cover the three code corrections. No new tools or schema parameters were added.

## Verification

- `cargo test --workspace`: 194 passed, zero failed or ignored.
- `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings`: passed.
- The freshly built CLI returned both known callers for `McpServer::handle_call_tool_inner` and the method body for `JsonRpcResponse::error`.
- Each code regression failed before its fix and passed afterward. Both distributed skills remain identical; `AGENTS.md` and `CLAUDE.md` remain identical.
- Runtime verification ran on Linux. No Windows runtime run, binary installation, or publication occurred.

All changes remain local and uncommitted on `main`. The existing installed MCP process is still running its previous binary.

## What to defer

Do not add rename/refactoring, a test runner, embeddings, batch operations, or another general search mode without a demonstrated failed workflow. Native terminal tools remain appropriate for running tests and for text outside indexed facts. Markdown headings and configuration structures are already indexed; treating all non-code files as a missing capability would be incorrect.

Measure successful end-to-end agent tasks, including overloaded names, external edits, failed indexing, and large results. Transport success rates and estimated token savings alone do not establish answer quality. The workspace telemetry inspected here includes prior testing and cannot be treated as an unbiased adoption study.

Top recommendation: make the existing locate → inspect → trace → edit flow preserve precise identity and disclose incomplete results before expanding the catalog.
