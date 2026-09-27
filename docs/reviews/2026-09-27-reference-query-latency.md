# Reference query latency follow-up

The subsequent [partial import index follow-up](2026-09-27-impact-query-latency.md) reduces impact further, to 3.80 s median in its paired run. The measurements below describe the preceding two-index change.

Two SQLite indexes reduce the difficult Hermes reference query from **20.08 s to 2.04 s median**, a **9.87× speedup**. The same change reduces the impact query from **8.17 s to 6.15 s**. All returned results are identical across the five measured workloads.

The [data file](2026-09-27-reference-query-latency-data.json) retains every sample, first response, output hash, process counter, memory reading, query plan, verification command, and the exact drivers. This follows the [receiver correctness and initial diagnosis](2026-09-26-receiver-reference-reliability.md); its candidate is this comparison's baseline.

## Cause and fix

The receiver-type check repeatedly searched common variable names across the repository before checking their file and type. The duplicate-reference check repeatedly searched every pending call with the same name before checking its location. Hermes contains 38,001 pending `get` calls; names such as `result`, `data`, and `kwargs` also have thousands of indexed definitions.

`ensure_fts_index` now creates these indexes before returning for an already-ready FTS database:

| Index | Columns | Work avoided |
|---|---|---|
| `idx_type_facts_resolved_symbol` | `resolved_type, symbol_id` | Looking through unrelated variables to establish a receiver's type |
| `idx_pending_name_site` | `target_terminal_name, path, start_line` | Looking through unrelated call sites when suppressing duplicate references |

The actual SQLite plans use both indexes. Existing databases acquire them without an FTS rebuild. SQLite maintains them during file updates. Column guards preserve support for minimal test databases; initialization errors propagate and can be retried. Reference SQL, matching rules, result limits, public interfaces, and extractor schema remain unchanged.

This corrects the initial diagnosis's emphasis on repeated inference. Aggregating ancestry checks did not help: the isolated pending stage took 16.94 s before and 17.08 s after that experiment. Additional symbol indexes and UNION changes did not justify their cost. The selected fix adds no query cache, resolver subsystem, dependency, or permanent profiling.

## Measured results

Five alternating pairs per workload ran in persistent MCP sessions, after recording the first response and discarding one additional warmup. Every arm used a fresh copy of the same frozen source export and index. No build or test ran during measurement. All **50 measured requests succeeded**, with zero errors, indexing notices, or timeouts. Each workload's normalized result hash was stable within and identical between arms; JSON-RPC request IDs were excluded from the hash.

Times are milliseconds. Brackets show the observed minimum–maximum, not a population tail estimate. OS caches were warm; the first response is not a cold-storage measurement.

| Workload | Before first | After first | Before median [range] | After median [range] |
|---|---:|---:|---:|---:|
| Flask `FlaskCliRunner.invoke` callers, limit 200 | 15.83 | 15.15 | 14.70 [14.34–15.42] | 14.57 [13.99–15.33] |
| Flask context for the same method | 25.85 | 23.97 | 24.91 [24.55–25.19] | 24.87 [24.04–25.33] |
| Hermes `_Lazy.get` callers in `agent/relay_runtime.py`, limit 20 | 21,593.72 | 2,044.21 | 20,082.39 [19,967.29–20,201.63] | 2,035.41 [2,019.51–2,064.63] |
| Hermes `run_agent.py` impact, depth 2, limit 20 | 8,177.40 | 6,086.78 | 8,165.76 [8,119.38–8,216.80] | 6,153.54 [6,072.76–6,166.94] |
| Hermes `execute` context in `agent/relay_tools.py` | 70.80 | 69.88 | 68.55 [67.69–69.11] | 67.86 [66.15–68.91] |

The reference query's latency fell **89.9%**; impact fell **24.6%**. The small differences on the other three workloads do not establish a meaningful speedup. The returned eight Flask callers, twenty Hermes reference rows, both context responses, and bounded impact response are unchanged. Hermes references still contain two supported rows and eighteen candidates. Impact still returns twenty test rows and twenty impacted-symbol rows and reaches the existing discovery ceiling. Output equality does not establish complete graph coverage or additional inference accuracy.

## CPU, I/O, and memory

Linux `/proc` counters were captured around every measured request, with RSS sampled every 250 ms. The following are medians of the five scoped Hermes reference requests.

| Per-request measurement | Before | After |
|---|---:|---:|
| CPU time | 20.04 s | 2.03 s |
| CPU utilization, 100% = one logical core | 99.8% | 99.5% |
| Bytes read from storage | 0 | 0 |
| Bytes returned by read system calls | 40.07 GiB | 315.10 MiB |
| Read system calls | 10,504,301 | 80,689 |
| Sampled peak RSS | 274.01 MiB | 260.40 MiB |

The query still occupies roughly one core while running, but uses about 90% less CPU time. Cached-read traffic fell 99.2%. All ten reference requests recorded zero storage reads and zero major faults. The byte count measures repeated cached reads, excludes memory-mapped accesses, and is not unique content read from disk. Faster storage was not the remedy for this warm workload.

Retained Hermes RSS after all workloads was **26.69 → 26.91 MiB**; PSS was **21.68 → 21.88 MiB** and anonymous memory **18.79 → 19.11 MiB**. Across the five reference repetitions, candidate retained RSS went from 26.38 to 26.43 MiB. There is no material retained-memory increase in this short run. Peak RSS includes the existing 256 MiB Unix database mapping, which is disabled on Windows. Windows performance and long-duration memory growth were not measured.

## Index and startup cost

| Corpus | Database before | Net database growth | Space occupied by new indexes |
|---|---:|---:|---:|
| Flask | 13,201,408 bytes | 233,472 bytes | 233,472 bytes |
| Hermes | 1,821,097,984 bytes | 36,831,232 bytes (35.12 MiB, 2.02%) | 38,899,712 bytes (37.10 MiB) |

Hermes reused 2,068,480 bytes of existing free pages. This explains why the allocated index space exceeds the database's net growth. The initial experimental estimate of about 37 MiB described allocated space.

A separate fresh-copy probe called the actual release `ensure_fts_index` with bundled SQLite 3.53.2. The first upgrade took **780.04 ms**; calling it again on the ready database took **0.08 ms**. This single warm-cache observation measures the initializer only, excluding connection open/close, extraction, reconciliation, and MCP transport. It is separate from the request timings above.

## SQL evidence and regression guard

Single direct-query probes, separate from the paired MCP measurements, preserve the complete core results and show where the work disappeared:

| Stage | Before execution steps | After execution steps | After elapsed time |
|---|---:|---:|---:|
| Pending-call matching | 281,044,902 | 50,874,048 | 1,574 ms |
| Identifier matching | 79,214,167 | 10,324,194 | 342 ms |
| Impact traversal | 195,802,088 | 158,413,789 | 5,833 ms |

Sort counts remain unchanged in these selected probes. Indexes remove the dominant repeated lookup work; substantial recursive inference work remains in impact. Two seconds for this reference query and six seconds for impact are still slow. This closes the missing-index fix, while the broader priority 1 latency and inference work remains open.

The new `ready_search_indexes_gain_bounded_reference_lookups` test upgrades an already-ready search database, initializes twice, verifies exact returned rows, and bounds each hot lookup to fewer than 100 SQLite execution steps. It failed at **6,012 steps** before the type index and **10,018 steps** before the site index. It now passes without a wall-clock threshold.

## Verification and identities

| Gate | Result |
|---|---|
| Linux `cargo test --workspace --locked` | 546 passed |
| Linux format, Clippy with warnings denied, release build | Passed |
| Agent-guidance and skill-copy equality; whitespace check | Passed |
| Windows/NTFS database tests, including new regression | 15 passed |
| Windows/NTFS affected reference, impact, Qt, and FTS integration tests | 136 passed |
| Windows/NTFS CLI/MCP receiver-label checks | 2 passed |
| Frozen-corpus paired output comparisons | All five workloads identical |

- Before source: `f804a790b86fd715207a52feb6c52f1d704db010`; binary SHA-256 `11518ea54d1cdf2a031bbd0ae3504f0315339d377a6d40f5516057661687279d`.
- Verified source: `0551d3180285466fd4c7fa88bc9f6526353916a0`; binary SHA-256 `98e8cf18c019018ab1a47ba0e0fa3607df0ead8d0731b4fc5aeb79a611f7cacd`.
- Flask source: `d73fa1cdcbd8b1465c151db8924ba58b1dd14e35`, 235 indexed files and 6,525 symbols.
- Hermes source: `8706517544bcc3f41f3b6521725f80b349ead3c7`, 12,788 indexed files and 988,219 symbols.
- Same Fedora 44 / i9-12950HX machine, Rust/Cargo 1.97.1, extractor 3.6.3, and bundled SQLite 3.53.2 as the preceding report. The JSON records corpus hashes and exact commands.

The Linux gate ran on the changes subsequently committed as `0551d31`; Windows and the benchmark ran with that commit clean. Later changes are reports and memory. Work remains local in `.worktrees/receiver-reference-reliability` on `fix/receiver-reference-reliability`. No push or release occurred. Complete-task token and context savings remain unmeasured.

## Reproduce

Restore `reproduction.files` from the data JSON to their recorded relative paths. Build the before commit separately and place its executable at `target/receiver-query-latency/before-code-kb`; build the verified candidate in the task checkout. Prepare the frozen corpus exports and indexes using the preceding report's instructions. Update paths and rebuilt hashes in `measure-setup.json` to describe the actual artifacts. Preserve fresh paired copies; do not use the experimental databases with rejected indexes.

```sh
python3 target/receiver-query-latency/measure.py \
  --candidate-binary target/release/code-kb --run-id repeat-01
```

Use a new run ID each time. The driver reuses the existing persistent MCP client, records first responses separately, enforces a 120 s request deadline and 300 s readiness deadline, and saves all outcomes. The JSON also includes the small release-linked upgrade probe and its build/run commands. No permanent benchmark framework was added.
