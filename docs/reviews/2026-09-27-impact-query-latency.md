# Impact query latency follow-up

One partial SQLite index reduces the Hermes impact query from **6.50 s to 3.80 s median**, a **41.6% reduction** in this paired run. Observed p95 falls **7.27 s to 4.21 s**. All five workloads return identical answers. The query still takes several seconds; this closes the import-lookup fix, while the broader priority 1 work remains open.

This follows the [two-index reference improvement](2026-09-27-reference-query-latency.md). Its reported 6.15 s impact baseline measured 6.50 s in this run; the comparison above uses the two arms of the current run. The [data file](2026-09-27-impact-query-latency-data.json) preserves all samples, resource counters, first responses, normalized results, identities, and reproduction inputs. Repeated result bodies are stored once by hash.

## Cause and change

The ordinary pending-call branch accounted for about 5.4 seconds of the remaining impact traversal. Its import checks repeatedly visited unrelated symbols. In a separate scan-status probe, 6,057 alias-import lookups visited 8,202,194 symbol rows; another 6,057 module-import lookups visited 2,832,100 rows. Existing indexes covered a file or a name, but not the import rows within that file.

`ensure_fts_index` now installs:

```sql
CREATE INDEX IF NOT EXISTS idx_symbols_import_path_name
ON symbols(path, name) WHERE kind = 'import';
```

The index serves both file-only and file-plus-name import lookups. It is created before the ready-FTS return, so existing databases acquire it without rebuilding search tables. Column guards preserve minimal database fixtures, SQLite maintains it during updates, and initialization errors propagate. Query SQL, inference rules, public interfaces, limits, and extractor schema are unchanged.

Single direct-core probes, separate from the paired MCP measurements, confirm where work disappeared:

| Recursive impact stage | Before | After |
|---|---:|---:|
| SQLite execution steps | 158,413,789 | 76,046,900 |
| Statement elapsed time | 5,833 ms | 3,374 ms |
| Sorts | 70,082 | 70,082 |

The complete core response from these probes also matched. Whole-core time was 5.93 → 3.49 seconds. These isolated observations are diagnostic evidence, not the public request timing claim. A builder-aggregation prototype remains outside this change.

The temporary scan-status probe used the project's SQLite 3.53.2 amalgamation with `SQLITE_ENABLE_STMT_SCANSTATUS`. Per-loop visits identify repeated lookup work; inclusive cycle counts were not summed as separate costs. See SQLite's [scan-status documentation](https://www.sqlite.org/c3ref/stmt_scanstatus.html).

## Paired MCP measurements

Five alternating pairs per workload ran in persistent MCP sessions against fresh copies of the same frozen Flask and Hermes exports and indexes. First responses were recorded separately, followed by one additional warmup before measurement. No build, test, or profiler ran during measurement. All **50 measured requests succeeded**, with no errors, timeouts, or indexing notices. Normalized result hashes were stable across first responses, warmups, and samples, identical between arms, and identical to the preceding report's candidate results. Normalization replaces the temporary project root and excludes JSON-RPC request IDs.

Times are milliseconds. Brackets show the observed minimum–maximum.

| Workload | Before median [range] | After median [range] |
|---|---:|---:|
| Flask `FlaskCliRunner.invoke` callers, limit 200 | 14.97 [14.44–15.67] | 15.43 [14.71–15.82] |
| Flask context for the same method | 25.61 [25.57–29.47] | 27.19 [25.35–32.80] |
| Hermes `_Lazy.get` callers in `agent/relay_runtime.py`, limit 20 | 2,108.18 [2,104.10–2,435.54] | 2,048.51 [2,041.27–2,072.20] |
| Hermes `run_agent.py` impact, depth 2, limit 20 | 6,503.49 [6,280.49–7,274.60] | 3,798.00 [3,697.65–4,210.83] |
| Hermes `execute` context in `agent/relay_tools.py` | 71.64 [70.16–72.75] | 51.56 [50.22–55.48] |

Hermes context also improves in this sample. The small Flask changes and roughly 3% reference change do not establish a meaningful improvement or regression. Impact's first response was 6.69 → 3.95 seconds. Neither the first response nor the repeated requests are controlled cold-storage measurements.

The intended metric was request latency after warmup. Physical reads still occurred during this run, so these are observed repeated-request timings rather than fully cached timings. With five samples, the reported p95 is the observed maximum, not a reliable estimate of a population tail. No samples were discarded or rerun to obtain a better result.

## CPU, I/O, and memory

Linux `/proc` counters were captured around each request, with RSS sampled every 250 ms. Except for the explicitly labelled total, values below are medians of the five impact requests.

| Impact-query measurement | Before | After |
|---|---:|---:|
| CPU time per request | 6.39 s | 3.76 s |
| CPU utilization, 100% = one logical core | 98.2% | 99.5% |
| Bytes returned by read system calls | 955.58 MiB | 387.71 MiB |
| Read system calls | 244,649 | 99,275 |
| Physical storage reads, total across five requests | 111.03 MiB | 131.58 MiB |
| Sampled peak RSS | 282.36 MiB | 282.62 MiB |

CPU time falls about 41%; bytes returned by read calls fall about 59%. Physical reads vary from 0–42.43 MiB before and 0–86.26 MiB after, with zero major faults in both arms. This run does **not** establish a reduction in physical storage I/O. Read-call byte counts include repeated cached reads, exclude memory-mapped accesses, and do not measure unique content read from storage.

Retained Hermes memory after all workloads was **26.80 → 27.06 MiB RSS**, **21.91 → 21.95 MiB PSS**, and **19.05 → 19.12 MiB anonymous memory**. There is no material retained-memory increase in this short run. Peak RSS includes the existing 256 MiB Unix database mapping; Windows performance and long-duration memory growth were not measured.

## Index cost and regression guard

The new index contains 126,073 Hermes imports and adds **8,192,000 bytes (7.8125 MiB, 0.44%)** to the preceding candidate's database. Flask grows by 32 KiB. No repository graph or new cache is retained in memory.

A separate fresh-copy probe called the actual release `ensure_fts_index` against a database already containing the preceding two indexes. This incremental upgrade took **411.81 ms**; a second call took **0.19 ms**. This is one observation with bundled SQLite 3.53.2, excluding connection open/close, extraction, reconciliation, and MCP transport.

The new `ready_search_indexes_gain_bounded_import_lookups` regression starts with an already-ready FTS database lacking the import index. It initializes twice, checks exact returned rows, and limits both import lookup shapes to fewer than 100 SQLite execution steps despite thousands of unrelated rows. It failed at **10,016 steps** before the change and passes afterward, without a wall-clock threshold.

## Verification and source identity

| Gate | Result |
|---|---|
| Linux `cargo test --workspace --locked` | 547 passed |
| Linux format, Clippy with warnings denied, release build | Passed |
| Agent-guidance and skill-copy equality; whitespace check | Passed |
| Windows/NTFS database tests | 16 passed |
| Windows/NTFS affected reference, impact, Qt, and FTS integration tests | 136 passed |
| Windows/NTFS CLI/MCP receiver-label checks | 2 passed |
| Frozen-corpus output comparisons | All five workloads identical |

- Before source: `a688bb913b2aa6ba6914010f4df545d6fd4636a9`, whose code is unchanged from `0551d3180285466fd4c7fa88bc9f6526353916a0`; binary SHA-256 `98e8cf18c019018ab1a47ba0e0fa3607df0ead8d0731b4fc5aeb79a611f7cacd`.
- Verified source: `fe057e6838b704e5ef7047f1a320ade35122630a`; binary SHA-256 `0f028fd72e810d1ec6c7b48fbe29625403db89ab5498b46bf3441e67c6b5b78b`.
- Flask: `d73fa1cdcbd8b1465c151db8924ba58b1dd14e35`, 235 indexed files and 6,525 symbols.
- Hermes: `8706517544bcc3f41f3b6521725f80b349ead3c7`, 12,788 indexed files and 988,219 symbols.
- Same Fedora 44 / i9-12950HX host, Rust/Cargo 1.97.1, extractor 3.6.3, and bundled SQLite 3.53.2 as the preceding report.

The Linux gate ran on the changes committed as `fe057e6`; Windows and the benchmark ran with that commit clean. Later changes are reports and memory. Work remains local on `fix/receiver-reference-reliability` in `.worktrees/receiver-reference-reliability`; no push or release occurred.

Impact still returns twenty test rows and twenty impacted-symbol rows and reaches its existing discovery ceiling. Scoped references still return two supported rows and eighteen candidates. Output equality establishes preservation of these responses, not complete graph coverage or additional inference accuracy. Complete-task token/context savings and compaction frequency remain unmeasured.

## Reproduce

Restore the preceding report's driver from its data file, then restore this report's `reproduction.files`. Write this data file's `setup` object to `target/impact-query-latency/measure-setup.json`. Prepare the same frozen exports and indexes using the preceding report's instructions. Build the before source separately at `target/impact-query-latency/before-code-kb` and the candidate in the task checkout; update paths and rebuilt binary hashes in the setup to match the actual artifacts.

```sh
python3 target/impact-query-latency/measure.py \
  --candidate-binary target/release/code-kb --run-id repeat-01
```

Use a fresh run ID. The small wrapper reuses the previous driver and five-workload plan. For the incremental upgrade probe, first make a fresh SQLite backup of a baseline database containing the preceding two indexes and lacking the partial import index. The data file records the probe source and build/run commands. Keep profiler runs separate from request measurements.
