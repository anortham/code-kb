# Supplemental TypeScript trace review

- Four completed trials: slots 7/20 native and 8/19 code-kb. All four source reviews passed; protection remained unchanged; independent added tests passed **7 files / 59 tests**.
- Code-kb adopted MCP in both trials: **59 calls** total. Tool counts: `{"blast_radius": 8, "check_edit": 4, "file_skeleton": 19, "find_references": 4, "get_symbol_body": 17, "get_symbol_context": 4, "lookup_symbol": 3}`.
- MCP outcomes: **7 capped responses**, **4 expected ambiguity selections**, **2 empty body misses**, **1 operational freshness refusal**, and **0 JSON-RPC/transport errors**. There were **0 genuine native shell fallbacks** in these two code-kb trials.
- Across all 49 shell commands, classifications were `{"direct_native_structural_lookup": 12, "non_navigation_edit_test_build": 23, "non_navigation_other": 7, "permitted_literal_search": 1, "unknown": 6}`. Mixed actions remain `unknown`; direct lookups are descriptive counts.
- The four observed test failures all preceded the first production edit. They are recorded as preproduction test/baseline sequences, with **0 confirmed implementation repairs**.
- Slot 19 item 45’s `check_edit` freshness refusal is quoted in the JSON receipt; item 47 succeeds. The missing `.codex` path is consistent with the later registration-rewalk fix, but the attempt has no raw watcher events, so it cannot establish the exact trigger.

- Detailed item IDs, hashes, event classifications, and runtime regression evidence: [typescript-v2-traces.json](typescript-v2-traces.json), SHA-256 `173f8a8ae968e2ad11124ab8229b6ed2960edd717569ca0855c97485b47884fd`.
