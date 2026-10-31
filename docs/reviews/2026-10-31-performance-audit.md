# Performance audit — 31 October 2026

Status: complete; six measured bottlenecks fixed. Baseline source: `1fc6a1a7b86ba66094e0e8373880f3454d0d8140`.

## Scope and method

Reviewed search/lookup and context generation, reference evidence, blast radius, edit checks, index normalization, incremental updates, reconciliation, watcher startup, and telemetry/test-command helpers. Independent source reviews were checked against query plans, instrumented calls, and regression tests before retaining changes. A SQLite call in a loop is not automatically expensive: this engine runs in-process, and many indexed, bounded lookups are reasonable.

The trace probe calls the release-built core APIs directly against existing indexes with read-only connections. It records executed statements, SQLite virtual-machine instructions, full-scan steps, and wall time. Time includes construction of the JSON result value but excludes CLI startup, reconciliation, output writes, and probe reporting. Each corpus measurement discards one warm-up and records three warm samples; the reported nearest-rank p95 is the largest of those three samples, not a production latency estimate. Internal FTS queries appear in statement counts. Separate normalization probes use Python SQLite and five warm samples; they measure the candidate SELECTs, not extraction or the whole normalizer.

Artifacts and exact workloads are in [measurements.json](2026-10-31-performance-audit/measurements.json) and [normalization.json](2026-10-31-performance-audit/normalization.json). The disposable probe used the repository's locked dependency versions, with rusqlite's tracing feature added. Relevant corpus source and workloads stay fixed between compared runs. Timings are local; instruction counts and query plans provide less noise-sensitive evidence.

## Measured improvements

| Workload | Before warm p95 | After warm p95 | Result check |
| --- | ---: | ---: | --- |
| Hermes `AIAgent` lookup, 988,410 symbols | 406.74 ms | 11.55 ms | Exact JSON equality |
| Newtonsoft.Json `System` lookup | 5.27 ms | 1.63 ms | Exact JSON equality |
| Newtonsoft.Json `json writer` ranked search | 94.85 ms | 33.50 ms | Exact JSON equality |
| ASP.NET Core `HttpResponse.OnCompleted` report | 4,079.49 ms | 945.50 ms | Same sites; more evidence checked |
| Rust owner-normalization candidate query | 89.550 ms | 0.003 ms | Same candidate rows |
| Go owner-normalization candidate query | 55.025 ms | 0.002 ms | Same candidate rows |
| Deduplicate 5,000 unique caller IDs, isolated loop | 20.35 ms | 0.2323 ms | Same seeds and distinct count |

The normalization and deduplication rows are isolated measurements, not full command timings. Caller-report results are recorded separately below because the time budget affects how much evidence the report can finish checking.

## Findings

### 1. Incremental normalization chooses broad indexes despite existing narrow indexes

`db.rs::link_impl_owners` and `go_receiver_links` run after each successful single-file update/delete through `sync.rs::write_incrementally`. Both candidate queries select methods without an owner. SQLite instead chose `idx_symbols_parent`, visiting every parentless symbol before checking metadata, although the exact partial indexes already exist.

Hermes has 988,410 symbols and 246,246 parentless symbols, with no matching unlinked Rust/Go methods. The two reads spent roughly 2.71 million VM instructions returning nothing. Forcing the existing partial indexes reduces both to fewer than 1,000 instructions each. Query p95: Rust 89.550 ms to 0.003 ms; Go 55.025 ms to 0.002 ms. A batch of 50 updates could repeat these scans 50 times; that multiplication is a source-derived estimate, not a measured batch runtime.

The retained fix changes only index selection. The regression measures the actual linking function with and without 4,096 unrelated parentless symbols: before the fix, work grew from 151 to 45,205 VM instructions. Existing real-extraction Rust and Go ownership tests verify matching behavior.

### 2. Name lookup repeatedly scans the whole symbols table

`queries.rs::search_symbols_scoped` fetches 4× the requested limit, filters inherited writes and folds exact imports, then doubles the fetch size if more rows are needed. Its query combines equality with leading-wildcard LIKE and orders computed ranks. On Hermes, looking up `AIAgent` causes four full-table scans and 23.9 million VM instructions, taking 403.12, 404.17, and 406.74 ms warm. It returns 459 rows, including 455 import rows, although the ordinary result limit is 20.

The import rows are part of the JSON contract; dropping them would be a behavior change. The retained optimization uses the existing trigram index as a candidate filter and preserves the original equality/LIKE checks, ordering, repeated-fetch behavior, and import rows. Qualified names, short queries, local-variable queries, NUL-containing queries, and missing/stale indexes retain the original path. After the change, the same Hermes lookup takes 10.60, 10.95, and 11.55 ms and 196,271 VM instructions (99.2% fewer), with exactly the same JSON result. The smaller Newtonsoft.Json `System` lookup falls from 5.27 ms to 1.63 ms p95, also with identical output. SQLite's [trigram documentation](https://www.sqlite.org/fts5.html#the_trigram_tokenizer) explains the three-character minimum and why LIKE with ESCAPE does not use its acceleration; a quoted MATCH phrase supplies candidates while the original predicate remains authoritative.

### 3. Ranked search runs its broad fallback after filling its candidate budget

`collect_search_candidates` runs the AND-word query, then also runs the OR-word query even when the AND query fills the word cap. The second query is fully prepared, stepped, ranked, and materialized before Rust discovers that no new candidates can be admitted. Existing candidates already have their word-match score, so this work cannot enrich them.

On the saved Newtonsoft.Json corpus, `json writer` takes 89.48, 94.85, and 92.09 ms warm. The two word-branch executions account for 636,700 VM instructions and 62 ms of the final sample. A regression fixture fills the AND budget and adds OR-only distractors; the original code executes the word query twice instead of once. The fix skips OR when AND has filled the word budget. Afterward the workload takes 33.24, 33.42, and 33.50 ms with identical output. Total work falls from 958,175 to 328,802 VM instructions; the word branch runs once, using 36,354 instructions.

### 4. Receiver evidence converts row IDs before filtering

`typed_receiver_chain_owner_rows_sql` uses `CAST(pending.rowid AS TEXT) IN (...)`, preventing direct INTEGER PRIMARY KEY lookup. A report for ASP.NET Core's `HttpResponse.OnCompleted` overload at `src/Http/Http.Abstractions/src/HttpResponse.cs:110` has only 46 sites, but its four receiver-owner query executions account for 88.08 million of 89.03 million VM instructions and 1.95 million full-scan steps. Warm report times are 3.41, 3.13, and 4.08 seconds, with sites left unchecked by the report budget.

Casting incoming JSON IDs instead of the indexed row ID changes the plan from `SCAN pending` to `SEARCH pending USING INTEGER PRIMARY KEY`. Row keys and receiver-proof semantics remain unchanged. This removes the broad scan but does not alone remove the dominant cost: after that fix, five owner queries still spend 86.0 million instructions and about 3.5 seconds on repeated receiver proof. The row-ID fix is retained with an actual-helper regression: adding 2,000 unrelated pending rows no longer increases its work linearly.

### 5. Competing definitions repeatedly rebuild identical receiver-owner evidence

Expanded-statement diagnostics prove that all five owner queries in the row-ID-only run use the exact same ordered 20 pending IDs. Ownership and ancestry do not depend on the selected target. A one-entry cache now reuses the most recent exact chunk inside the report's read snapshot. It holds at most the existing 2,000-ID chunk, discards the prior entry before a miss, and preserves unknown owners and incomplete ancestry. Target-specific status and judging still run separately, including for removed symbols. The boundary is documented in [decision 003](../decisions/003-bounded-receiver-evidence.md).

The final report executes the expensive owner query once. Warm times are 876.56, 897.31, and 945.50 ms; total work falls from 89.03 million to 18.78 million VM instructions. In the saved before/after pair, all 46 sites remain, all previously checked labels stay the same, and the three proven sites are unchanged. The nine sites formerly left unchecked by the timer now receive checks: eight remain unresolved and one is excluded by receiver-type evidence. Three existing exclusions gain receiver-type reasons instead of arity reasons. The separate competing-definition cap still leaves 43 other definitions unchecked; the report continues to disclose that limit. This is not byte-identical output: it completes more evidence within the unchanged budget. See [the output comparison](2026-10-31-performance-audit/receiver-output-comparison.json).

The remaining single owner proof still takes about 0.7–0.8 seconds and 17.2 million instructions on this corpus. It is now the dominant report cost.

Actual-helper regressions failed before the cache because a repeated exact key doubled owner-query VM work (5,706 to 11,412 instructions); incomplete ancestry also recomputed (10,971 to 21,942). They now verify reuse, reordered-key replacement, oversized inputs, per-target status execution, removed targets, and preservation of unknown status after the ancestry edge limit.

### 6. Edit-check deduplicates callers with repeated linear searches

`SiteLister::walked_tests` scans the report sites and runs `Vec::contains` for each eligible caller, then truncates the distinct callers to 200 walk seeds. The count includes every distinct caller, so the limit does not bound deduplication work. A hash set preserves the first-seen order and complete count while avoiding quadratic comparisons.

An isolated Rust microbenchmark of the old loop versus hash-set membership uses synthetic eligible caller IDs, outside database work and report construction. It verifies output equality, warms each arm, and alternates five measured batches. At 2,000 distinct callers, per-operation p95 falls from 2.335 ms to 0.0808 ms; at 5,000, from 20.35 ms to 0.2323 ms. With half the 5,000 input rows duplicated, p95 falls from 8.349 ms to 0.1282 ms. These are not whole edit-check timings. The 5,000 point is a stress workload, not a bound on report size: `RESOLVER_ROW_CAP` caps a different evidence query, not `report.sites`. Exact samples and compiler details are retained beside the [microbenchmark](2026-10-31-performance-audit/walked-tests-dedupe.rs).


## Checks that did not justify a change yet

- Ordinary outline rendering is bounded and uses bulk symbol/count queries. Re-rendering at smaller output budgets is a small bounded CPU loop, not repeated database work.
- Related types already prepare their type-owner query lazily and reuse it. Callee signatures use joined reads; their small result loops are not automatically an N+1 defect.
- Offline reconciliation reuses prepared indexed point reads. It intentionally detects equal-sized content changes; skipping hashing based only on file size would break freshness. The watcher scan closes edits made before watch registration; the final synchronous reconcile is a first-query freshness barrier while watcher events may still be queued. Removing either requires a replacement ordering barrier, not merely deleting a duplicate call. A post-create FTS-readiness call is redundant, but its remaining cost after the normalization fix has not been established as a performance problem.
- Initial normalization still looks up Rust owner candidates per unresolved method, using `idx_symbols_name_kind`. After normalization the inspected indexes held 2 unresolved candidates in code-kb, 14 in Julie, and none in Hermes or ASP.NET Core. Go already reuses a prepared owner query and caches receiver-name candidates and package clauses within a normalization call; these corpora had no unresolved Go candidates. A fresh scan can have more candidates, but this audit did not measure that loop as a bottleneck. No repository-wide cache was added.
- Incremental refresh launches Julie per changed file, up to 50 paths before choosing a full scan. A deleted directory with exactly 50 indexed children can issue 50 child deletes plus a directory delete. The process and normalization costs could favor a bulk extractor operation, but no measured incremental-versus-full-scan crossover supports changing the threshold. The retained partial-index fix removes the confirmed repeated normalization scans without changing refresh ordering.
- FTS `CREATE INDEX IF NOT EXISTS` calls do not rebuild the index each time. The confirmed normalization problem is query-plan selection.
- The initial claim that Newtonsoft.Json `System` required four lookup scans was rejected: inherited-write filtering removed its definitions, so import rows counted normally and the actual trace showed one scan.
- Rival counting in `Accounting::account_spec` can still perform sites × same-name-definitions work after the judging deadline, including conditional database lookups in `rivals`. A timed-out undecided site can be counted again in the unchecked-site loop. `OTHER_DEFINITION_CAP` bounds the earlier judge loop, not this tail. The ASP.NET profile did not establish this as a dominant cost; changing the tail must preserve accurate unresolved/likely labels and rival counts. This remains a scaling risk.
- A 230 MiB CLI peak did not establish blast-radius heap growth: Linux maps up to 256 MiB of index pages. On a frozen 57,932-symbol corpus, a live depth-5 file-target request increased anonymous RSS by 8,896–9,060 KiB. Source instrumentation counted 343 seeds and 1,112 reached rows, an estimated 316,032 bytes of traversal buffers, excluding allocator/hash-table overhead, plus a 45,593-byte JSON buffer. This did not attribute the request's memory peak to traversal storage, so no scratch database or streaming rewrite was retained. The distinction remains important: the advertised 200-row ceiling limits selected output, not the number of visited graph rows. See [graph-memory.json](2026-10-31-performance-audit/graph-memory.json).

## Live MCP memory check

A separate stdio `serve` process received the exact line-110 symbol ID, using a pinned copy of the unchanged ASP.NET index. `/proc/<pid>/status` was sampled every 5 ms, with one warm-up and three measured calls. Warm live times fell from 3.100–3.763 seconds to 867–893 ms. Peak anonymous-memory growth was comparable: maximum 20,828 KiB before and 21,164 KiB after. Final post-call anonymous memory was 21,188 KiB before and 21,432 KiB after. This shows no material retained-heap increase on the measured workload; it does not establish a universal memory bound.

Mapped file pages are separate: peak `RssFile` rose from about 171.8 MiB to 199.6 MiB as the faster report checked more evidence. Total RSS therefore should not be mistaken for Rust heap allocation. Both the original and pinned database remained byte-identical. Exact binary hashes, request, memory samples, and outputs are in [receiver-live-memory.json](2026-10-31-performance-audit/receiver-live-memory.json).

## Reproduction

The retained [trace probe](2026-10-31-performance-audit/trace_probe.rs) is an isolated diagnostic crate, not a product CLI change. Build it with `cargo build --release --locked --manifest-path docs/reviews/2026-10-31-performance-audit/Cargo.toml`. Run its `query-profile` binary with `PROJECT_ROOT MODE QUERY PATH REPEATS`. Modes include `lookup`, `search`, `refs`, `blast`, `context`, `tests`, `callees`, `types`, and `outline`. Use an empty PATH argument when no path filter is needed. The diagnostic selects the first name match for a reference workload; the audited `OnCompleted` target is the overload at line 110 (ID `df091962119b0c9572b3979e10b2e7ce` in the retained corpus), also used for the live MCP measurement. The JSON artifact records exact arguments and source identity for this audit.

## Verification

- `cargo test --workspace --locked`: **2,347 passed**, zero failures or ignored tests, including the real-extraction fixture matrix for all 42 pinned Julie languages.
- `node --test tests/plugin/*.test.cjs`: **20 passed**.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check`, and `cargo build --release --locked`: passed. Clippy initially requested constant initializers for two test-only counters; after that cleanup, all **112 query unit tests** were rerun and passed.
- The retained diagnostic crate builds with its locked dependencies. Before/after result comparisons, actual-helper VM-step regressions, and the live MCP request provide independent performance evidence.
- `check_edit` identified three private shape changes. The SQL builder's call sites and `Accounting` initialization are updated; the two existing owner-evidence callers compile through `Rc` dereferencing. Its test discovery was incomplete, so the full workspace suite was run.
- Lead and independent architecture reviews checked output parity, exact-chunk reuse, unknown status, removed targets, and evidence limits. `AGENTS.md` and `CLAUDE.md` remain byte-identical.

Verification ran on Linux; Windows was not exercised in this audit. Command results and suite counts are retained in [verification.json](2026-10-31-performance-audit/verification.json).
