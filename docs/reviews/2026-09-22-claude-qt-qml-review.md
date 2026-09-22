# Claude review: Julie 3.3.1 Qt/QML integration

## Campaign record

| Field | Value |
| --- | --- |
| Source range | `a36f2c5..708f0f4` |
| Workflow | ordinary |
| Participants | lead, ClaudeCLI |
| Required reviewer | ClaudeCLI |
| External invocation budget | 1 |
| Max rounds | 2 |
| Round | 1 |
| External invocations consumed | 1 / 1 |
| Provider | anthropic |
| Provider policy | No external-model policy declared — diff sent to anthropic. |
| Completion | Completed; structured result validated. |

The normalized reviewer output is stored beside this record in
`2026-09-22-claude-qt-qml-review.json`.

## Triage status

The campaign remains open. These dispositions do not mark it clean; accepted
findings still require implementation and verification.

| Finding | Disposition | Evidence |
| --- | --- | --- |
| Header scan omits partial/schema/FTS handling | Accepted and fixed, pending final gate | Reuse `scan_workspace(force=true)` and retain the target-hash guard. |
| One offline full scan per changed header | Accepted and fixed, pending final gate | `test_reconcile_offline_edits_refreshes_two_headers_together` and `test_reconcile_offline_edits_forces_a_large_batch_with_a_header` assert one refresh for small and >50-change batches. The watcher was already fresh after the first header scan; `test_watcher_refreshes_a_header_batch` confirms one refresh for two headers. |
| Extension-only `.h` trigger | Dismissed | A `/tmp` probe changed a header from C to a Qt C++ `Q_PROPERTY` header. Direct `update --file` kept its stored language `c` and omitted the property; a forced scan changed it to `cpp` and found one property. A stored-`cpp` gate would miss this transition. |
| Ignored headers should fall back to update | Dismissed | Direct Julie 3.3.1 updates for ignored and `build/` headers exited zero as `unsupported` and produced no index rows. Returning success would violate the post-edit indexing invariant. The analogous non-header exit-zero invariant hole is a separate follow-up. |
| Watcher schedules a redundant scan during edit | Accepted and fixed, pending final gate | A separate controlled `/tmp/julie-delay-counter.dLyvzg` probe delayed non-initial scans for one second. Before the lock/recheck it logged initial, edit, and watcher scans (revisions 1→3); after `HEADER_SCAN_LOCK` and `header_hash_matches_disk`, it logged initial and edit scans only (1→2). The ordinary fast test `test_watcher_leaves_a_header_edit_fresh` separately asserts the +1 revision result. |
| `StructuralFact.metadata` needs serde default/JSON suppression | Dismissed | Missing `Option<T>` fields deserialize as `None` without `#[serde(default)]`; explicit `facts --json` intentionally preserves the upstream metadata while text output is compact. |
| Invalid metadata JSON newly aborts facts query | Dismissed | SQLite `json_extract('not json', '$.key')` already fails with malformed JSON, so the previous display-key query had the same whole-query failure mode. |
| Release bump and release notes | Deferred, out of scope | This task covers review and Qt/QML documentation, not publishing. The six Julie 3.3.1 archive hashes were verified earlier. |

Focused verification is currently green: `freshness_test` (20 tests),
`qt_cpp_test` (11), and `watcher_test` (6). The final branch gate remains
required before this campaign can close.

Linux final-gate verification passed: 416 workspace tests passed with no
failures or ignored tests; formatting, warnings-denied Clippy, 17 plugin tests,
both documentation-sync checks, and diff checks passed. Desktop and mobile
visual QA also passed. Windows verification remains pending.

The C-to-Qt/C++ transition probe used a released `julie-extract 3.3.1` binary
extracted from `/tmp/code-kb-julie-3.3.1-hashes/julie-extract-v3.3.1-x86_64-unknown-linux-gnu.tar.gz`;
its version command reported `3.3.1`. Probe files were temporary-only under
`/tmp/code-kb-review-97ih6b`.
