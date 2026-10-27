# MCP trace review

- Scope: original 10 code-kb trials; completed 10/10.
- Worktree: `/home/murphy/.codex/worktrees/1e6f/code-kb`; branch `fix/cross-language-review`; HEAD `0ba3d1788f13299751f8b12cad9f3ce2ed548f12`; dirty before this report.
- Adoption: 10/10 trials used code-kb, 188 calls total (mean 18.8, median 19.0).
- MCP outcomes: 0 JSON-RPC errors, 1 operational result error, 5 ambiguous selections, 3 exact-name misses with ranked results, 0 empty responses, 17 cap notices across 11 calls.
- `trace_metrics.error_count` counts shell/runtime errors and is separate from MCP response errors.
- The five ambiguous responses are candidate-selection results. Slot 10 item 28 is the separate watcher freshness refusal; item 30 `check_edit` succeeds on the next tool call.
- Test-failure review: four failures preceded the first production edit (slots 2, 6, 15, 17). Slot 23 has an observed postimplementation failure-to-production-edit sequence, with post-edit test verification unavailable. Slot 12 is unclassifiable from captured diagnostics.
- Slot 15 uses the recovered trace record; no provider rerun. Actual elapsed/indexing time is unavailable.
- TypeScript slots 8 and 19 are excluded because setup prevented model exposure.
- Per-trial tool counts, event IDs, source-backed outcome notes, raw artifact paths, and SHA-256 hashes are in [mcp-traces.json](mcp-traces.json).
- Paired native-arm command classifications are in [native/native-traces.json](native/native-traces.json).

- Slot 2 `002-sqlglot-code-kb-r1`: 16 calls; no source-backed error, miss, ambiguity, or cap observed.
- Slot 4 `004-minijinja-code-kb-r1`: 20 calls; item_8:exact_name_miss_with_ranked_results, item_15:explicitly_capped.
- Slot 6 `006-hcl-code-kb-r1`: 19 calls; no source-backed error, miss, ambiguity, or cap observed.
- Slot 10 `010-commons-lang-code-kb-r1`: 22 calls; item_9:ambiguous_selection, item_19:explicitly_capped, item_20:ambiguous_selection/ambiguous_query_error_prefix, item_21:explicitly_capped, item_28:operational_result_error/transient_watcher_freshness_refusal_recovered_on_next_check, item_34:explicitly_capped.
- Slot 12 `012-nodatime-code-kb-r1`: 19 calls; item_10:exact_name_miss_with_ranked_results/explicitly_capped, item_12:explicitly_capped, item_14:explicitly_capped.
- Slot 13 `013-sqlglot-code-kb-r2`: 15 calls; no source-backed error, miss, ambiguity, or cap observed.
- Slot 15 `015-minijinja-code-kb-r2`: 19 calls; item_6:exact_name_miss_with_ranked_results, item_15:explicitly_capped.
- Slot 17 `017-hcl-code-kb-r2`: 16 calls; no source-backed error, miss, ambiguity, or cap observed.
- Slot 21 `021-commons-lang-code-kb-r2`: 20 calls; item_13:ambiguous_selection, item_15:explicitly_capped, item_18:explicitly_capped.
- Slot 23 `023-nodatime-code-kb-r2`: 22 calls; item_11:ambiguous_selection/ambiguous_query_error_prefix, item_14:ambiguous_selection/ambiguous_query_error_prefix, item_55:explicitly_capped.
