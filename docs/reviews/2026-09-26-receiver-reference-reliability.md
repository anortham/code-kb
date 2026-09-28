# Receiver reference reliability

**Follow-up:** [Two lookup indexes reduce the 20-second query to 2 seconds](2026-09-27-reference-query-latency.md). The measurements and next-step hypotheses below describe the earlier correctness slice.

The receiver fix removes the reproduced wrong matches and labels unresolved member references as candidates. It also adds latency. The difficult Hermes reference query takes **20.50 s median, 20.72 s p95**, against **18.52 s and 18.79 s** before the change. Performance work remains open.

The [data file](2026-09-26-receiver-reference-reliability-data.json) contains every timing sample, first responses, correctness labels and results, memory readings, CPU/I/O samples, SQL, query plans, source identities, verification ledger, and exact diagnostic drivers. Measurements finished September 27 UTC, September 26 locally.

## What changed

- Share local-variable and fixture builder evidence between pending calls and member-reference matching. Respect source modules, assignment order, fixture scope, and nearest inherited overrides.
- Omit a member reference when its receiver identifies a different applicable method. An imported base-class instance cannot reference a subclass override merely through a shared method name.
- Return insufficient or conflicting evidence as `member_access (candidate)`, including unknown factories and ambiguous bindings. No control-flow analyzer was added.
- Classify and deduplicate before limiting identifier results. Supported rows come first. Candidate-only references cannot enter context's reference-based related-test stage or create impact edges.
- Preserve the public models, CLI/MCP interfaces, extractor 3.6.3, schema v7, and separate field, property, type-usage, and QML rules. Production changes are in `crates/code-kb-core/src/queries.rs`.

## Fixed comparison

| Item | Identity |
|---|---|
| Baseline source | `7ceb734be947aced7cedf1ea179fbce3c9987c88` |
| Verified candidate source | `f804a790b86fd715207a52feb6c52f1d704db010` |
| Baseline binary SHA-256 | `e27a9ac693034d8c54e8155c1dbb0efe332cdf58e2660d33c30c48add8c1b043` |
| Candidate binary SHA-256 | `11518ea54d1cdf2a031bbd0ae3504f0315339d377a6d40f5516057661687279d` |
| Extractor | `3.6.3`, SHA-256 `688e84e627f9bc7ebb7d50fb46fc31da868e50170b5613902898aa23fc49c4e4` |
| Query engine | Bundled SQLite `3.53.2` |
| Machine | Fedora 44, Linux `7.2.7-200.fc44.x86_64`, i9-12950HX, 24 logical CPUs, about 64 GiB RAM |
| Build | Rust and Cargo `1.97.1`, release profile, locked dependencies |

| Frozen corpus | Source commit | Indexed files | Symbols | Database bytes |
|---|---|---:|---:|---:|
| Flask | `d73fa1cdcbd8b1465c151db8924ba58b1dd14e35` | 235 | 6,525 | 13,201,408 |
| Hermes | `8706517544bcc3f41f3b6521725f80b349ead3c7` | 12,788 | 988,219 | 1,821,097,984 |

Both arms received fresh copies of the same index and tracked corpus export. Original repositories were left alone, including Flask's unrelated untracked files. Each MCP request supplied its export's absolute `project_root`.

Hermes reproduces the earlier reported symbol population. The historical source revision, exact `get`/`execute` selections, and blast target were not all recorded. These measurements establish a new fixed comparison. They do not reconstruct that historical workload exactly.

The name-only `get` and `execute` calls return ambiguity errors listing 25 candidates. Separate scoped probes select `_Lazy.get` in `agent/relay_runtime.py` and `execute` in `agent/relay_tools.py`. `run_agent.py` is the newly fixed blast target.

## Latency and returned results

One persistent MCP session per arm and corpus separated startup from requests. The driver recorded the first response, discarded one additional warmup, and collected 20 requests per workload and arm with alternating order. Requests had a 120 s deadline, readiness a 300 s deadline. Indexing notices were recorded separately. The p95 is the existing helper's nearest-rank percentile. No build or test ran during the paired measurements.

All timings below are milliseconds. The first request is not a cold-cache measurement. OS caches were already warm, and workload order is recorded in the data.

| Workload | Baseline first | Candidate first | Baseline median / p95 | Candidate median / p95 |
|---|---:|---:|---:|---:|
| Flask `FlaskCliRunner.invoke` callers, limit 200 | 6.63 | 18.88 | 4.53 / 6.47 | 15.20 / 16.89 |
| Flask context for the same method | 9.85 | 26.82 | 9.81 / 10.53 | 26.35 / 27.90 |
| Hermes name-only `get` callers, limit 20, ambiguity error | 172.80 | 1,055.49 | 9.19 / 10.55 | 9.32 / 10.98 |
| Hermes name-only `execute` context, ambiguity error | 33.77 | 10.80 | 8.83 / 9.64 | 8.88 / 10.17 |
| Hermes `run_agent.py` blast, depth 2, limit 20 | 7,012.73 | 8,384.93 | 6,168.73 / 6,255.18 | 8,266.32 / 8,470.98 |
| Hermes scoped `_Lazy.get` callers, limit 20 | 19,296.29 | 21,676.22 | 18,520.83 / 18,794.16 | 20,504.31 / 20,720.62 |
| Hermes scoped `execute` context | 115.73 | 117.96 | 67.16 / 68.48 | 73.47 / 75.70 |

There were **200 successful requests, 80 expected ambiguity errors, and zero timeouts** among the 280 measured requests. Error-row timings describe errors, not successful retrieval. Each successful arm/workload had a stable output hash across its 20 samples.

| Output comparison | Observation |
|---|---|
| Flask callers | 9 rows became 8. Only the incorrect `invoke` member reference at `tests/test_cli.py:460` disappeared. |
| Flask context | Identical body, one dependency signature, and four related tests. |
| Hermes scoped callers | Same 20 returned sites. Two supported member references moved first; the other 18 now carry the candidate label. |
| Hermes scoped context | Identical body, five dependency signatures, and five related tests with a cap notice. |
| Hermes blast | Identical bounded text: 20 of 115 likely tests, 20 of 98 impacted symbols. The 200-row discovery ceiling was reached. Equality does not prove full graph coverage. |

The candidate increases median scoped-reference latency by 10.7% and this blast workload by 34.0%. Fewer wrong matches are not evidence of a speedup.

## CPU and disk I/O during the slow query

At the owner's request, a separate probe sampled the live MCP process through Linux `/proc` every 250 ms. It reused the binaries, corpus copies, tool arguments, and request deadline above. After a completed warmup per arm, it measured three alternating requests per arm. These supplementary timings are separate from the 20-sample results.

| Per-request measurement, median of three | Baseline | Candidate |
|---|---:|---:|
| Query wall time | 18.19 s | 20.26 s |
| User CPU time | 13.42 s | 15.57 s |
| Kernel CPU time | 4.72 s | 4.66 s |
| CPU utilization, 100% = one logical core | 99.7% | 99.7% |
| Bytes read from storage | 0 | 0 |
| Bytes returned by read system calls | 39.29 GiB | 40.07 GiB |
| Read system calls | 10.30 million | 10.50 million |
| Peak resident memory | 274.8 MiB | 277.9 MiB |

All six requests recorded zero storage reads and zero major page faults. Storage writes were only 56–76 KiB per request. The active query thread accounted for essentially all process CPU time. On this 24-logical-CPU machine, one saturated core is about 4.2% of total CPU capacity.

The read-call byte count is repeated data access through the OS cache, not unique database content or disk traffic. It also excludes accesses satisfied through memory mapping. These warm runs show CPU and cached-read overhead. They provide no evidence that faster storage would fix the 20-second query, and they do not measure cold-storage performance.

## Memory

Live server measurements from the paired run are MiB. The JSON retains the helper's `*_mb` keys, which divide KiB by 1024.

| After warmup → after 20 requests | Baseline RSS / PSS / anonymous | Candidate RSS / PSS / anonymous |
|---|---|---|
| Flask callers | 14.57→14.79 / 12.26→12.48 / 6.80→7.02 | 16.80→17.00 / 14.37→14.57 / 9.01→9.21 |
| Flask context | 14.96→15.01 / 12.65→12.70 / 7.19→7.24 | 17.16→17.20 / 14.73→14.77 / 9.31→9.34 |
| Hermes blast | 25.38→25.62 / 20.31→20.55 / 17.56→17.80 | 26.65→25.21 / 21.59→20.16 / 18.86→17.42 |
| Hermes scoped callers | 25.62→25.62 / 20.55→20.55 / 17.80→17.80 | 26.62→26.66 / 21.56→21.60 / 18.83→18.87 |

End-of-run retained RSS is about 1.04 MiB higher on Hermes and 2.19 MiB higher on Flask. The repeated scoped query does not show sustained per-request growth over this short run. The added predicates and intermediate SQL work increase memory demand; exact allocator attribution for the retained difference was not measured. No repository graph or persistent cache was introduced.

The CPU/I/O probe also exposed transient RSS that before/after readings miss. A peak snapshot attributes 255.7 MiB to file-backed pages and 22.2 MiB to anonymous memory in the candidate. The baseline has 255.6 MiB file-backed and 19.2 MiB anonymous. `/proc` confirms a 256 MiB database mapping in both processes, matching Unix `open_read_only`'s existing `mmap_size`. This is different from retaining a 278 MiB Rust symbol graph. Windows disables this mapping; its performance was not benchmarked here.

## SQL diagnosis

A standalone probe links the existing release libraries and invokes the real core query functions. It records SQLite's executed SQL with bound values, statement counters, and `EXPLAIN QUERY PLAN` on a read-only copy. Both the probe and the application use SQLite 3.53.2. Python's system SQLite 3.51.2 was used for copying/counting, not for these plans.

These are single diagnostic observations. They exclude startup reconciliation and MCP transport. Do not subtract them from separate MCP runs to estimate exact protocol overhead.

| Direct-query stage | Baseline | Candidate | Candidate execution steps |
|---|---:|---:|---:|
| Scoped `get`, complete core operation | 18,076.90 ms | 19,921.86 ms | Across several statements |
| Pending-call matching | 15,656 ms | 15,810 ms | 281,044,902 |
| Member-identifier fallback | 2,406 ms | 4,066 ms | 79,214,167 |
| Blast, complete core operation | 6,789.84 ms | 7,997.76 ms | Across several statements |
| Blast recursive walk | 6,680 ms | 7,901 ms | 195,802,088 |

The scoped query examines a common terminal name: 38,001 pending `get` calls and 157 member-access identifiers. Its pending stage produces no returned matches yet consumes about 79% of direct-query time. The identifier stage supplies the 20 rows. The target has only two supported member-reference rows in that bounded result.

The plans use the existing terminal-name and symbol indexes, but repeat correlated builder, module-origin, and inheritance queries. The candidate pending plan contains 38 correlated subqueries and repeated materialization of `built` and `builder_call`. It performs 134,196 sorts. Its identifier stage performs 43,528 sorts. No captured dominant statement constructed an automatic index.

The blast walk also repeats these checks while traversing callers. Its candidate profile records 1,224,058 full-scan steps, 70,082 sorts, and 195.8 million execution steps, versus 1,100,833, 25,466, and 154.3 million before the change. Its plan includes a full `symbols` scan for seed selection. The profile does not assign separate timings to that scan and the recursive checks.

The correctness changes add work, particularly to identifier classification and blast traversal. Repeated receiver resolution is the primary measured cost. This is not a demonstrated missing-index problem.

### Regression caught during measurement

The first candidate took 48.88 s for the direct scoped query; pending matching alone took 44.17 s. SQLite evaluated new module-scope checks for builders whose return types could never enter the indexed-class traversal.

A lazy SQL `CASE` now checks for a usable indexed return class before evaluating that scope predicate. Adding an `AND` filter did not enforce evaluation order. A captured-query trial reduced the pending stage to 17.05 s with the same rows; final production measurements above confirm the large regression is removed. The correction is commit `f804a79`.

Index-order and path-index trials did not help and were discarded. The data distinguishes those single-statement diagnostic trials from complete-query measurements. None introduced a schema change or extra index.

### Next performance step

Prototype reuse of each call site's builder, import-origin, and ancestor evidence within the SQL query. The current query recomputes that evidence when testing both the selected definition and nearer overrides. First target the pending stage's 281 million steps and 134,196 sorts, then the duplicated work in identifier classification and the blast walk.

Keep that work inside SQLite and preserve the existing ambiguity, module, fixture, and nearest-override rules. Compare complete result identities and labels, then repeat the same frozen workloads and CPU/I/O counters. Reject a speedup that loses required rows or hides uncertainty. No speedup from this proposed restructuring has been measured yet.

## Correctness evidence

The CLI comparison covers nine targets across five regression fixtures, with 44 target-row labels: 15 required, 15 forbidden, and 14 unresolved. Sites shared by different targets are distinct labels. This is an implementation regression set, not a held-out estimate of project-wide accuracy.

| Labeled result | Baseline | Candidate |
|---|---:|---:|
| Required rows returned | 15/15 | 15/15 |
| Forbidden rows returned | 14/15 | 0/15 |
| Supported precision, excluding unresolved labels | 15/29, 51.7% | 15/15, 100% |
| Supported recall | 100% | 100% |
| Unresolved rows retained | 14/14 | 14/14 |
| Unresolved rows labeled as candidates | 0/14 | 14/14 |

Tests additionally cover same-name modules, later assignments, mixed known/unknown builders, fixture shadowing and scope, nearest overrides, malformed metadata, limits and deduplication, symbol IDs, context tests, callees, impact, and CLI/MCP labels. A bare `super().invoke` read lacks extractor receiver metadata and remains a candidate; the actual `super().invoke()` call is covered as supported.

## Verification and local state

| Gate | Result on `f804a790b86fd715207a52feb6c52f1d704db010` |
|---|---|
| `cargo test --workspace --locked` | 545 passed |
| `node --test tests/plugin/*.test.cjs` | 19 passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | Passed |
| Release build and formatting | Passed |
| Both agent-guidance and skill-copy comparisons | Passed |
| Windows/NTFS affected core tests | 135 passed |
| Windows/NTFS new CLI/MCP contract tests | 2 passed |

The JSON ledger records exact commands, source and dirty state, timestamps, scopes, and local log paths, including earlier red/green work. The release build ran while the guard was uncommitted; that source was committed as `f804a79` and matches the measured binary. Later report and memory edits do not change the verified source. Temporary tracing lives under ignored `target/`; production has no tracing instrumentation.

Work remains local on `fix/receiver-reference-reliability` in `.worktrees/receiver-reference-reliability`. No push or release was performed. This completes the approved receiver-correctness and diagnosis slice. Priority 1 remains open for latency work and additional inference explanations. No complete-task token or context savings claim follows from these results.

## Reproduce

Use this task source, Rust/Cargo, Node, Python 3, and the pinned extractor. The JSON's `reproduction.files` embeds the exact drivers and labeled fixture files. Restore them under `target/receiver-reference-reliability/`, preserving their relative paths. The driver imports the existing `scripts/benchmark_quality.py` client and does not add a benchmark framework.

1. Export baseline commit `7ceb734be947aced7cedf1ea179fbce3c9987c88` with `git archive` into `target/receiver-reference-reliability/benchmark/build/baseline-source`. Restore its extractor and build with a separate `CARGO_TARGET_DIR` ending in `benchmark/build/baseline-target`.
2. In the task worktree run `bash scripts/restore-julie-extract.sh` and `cargo build --release --locked`.
3. Export the two corpus commits above into `benchmark/corpora/flask` and `benchmark/corpora/hermes`, relative to the artifact directory. Add a `.git` marker so project-root resolution stays in each export. Scan each export using the baseline binary. Use the owned `benchmark/tmp` directory for `TMPDIR`; this session's system temporary-directory quota was nearly full.
4. The embedded setup file records this machine's absolute paths and hashes. For another location or rebuilt binaries, update those recorded identities and the benchmark plan's paths before running. Never present a rebuilt artifact as the recorded binary hash. The driver makes and verifies new paired index copies itself.
5. Run the commands below. Each correctness rerun needs a new run ID. The resource probe uses the recorded benchmark result path; point it at the newly produced result for a new run and keep its output path distinct.

```sh
python3 target/receiver-reference-reliability/benchmark/paired_benchmark.py --candidate-binary "$PWD/target/release/code-kb"
python3 target/receiver-reference-reliability/correctness/measure_correctness.py --baseline target/receiver-reference-reliability/benchmark/build/baseline-target/release/code-kb --candidate target/release/code-kb --extractor .tools/julie-extract --run-id receiver-correctness-repeat
python3 target/receiver-reference-reliability/diagnosis/resource_probe.py
```

For SQL diagnosis, compile the embedded `diagnosis/sql_probe.rs` against each build's release `code_kb_core`, `rusqlite`, and `serde_json` libraries using `rustc --edition=2024 -O -C panic=abort` and that build's `release/deps` search path. The probe accepts `refs DB get agent/relay_runtime.py 20`, `blast DB run_agent.py`, and `explain DB SQL_FILE`. Use a SQLite backup of the frozen artifact for `DB`. The stored SQL has its bound values expanded, and each stored plan identifies the SQLite version used.

Primary raw run: `target/receiver-reference-reliability/benchmark/results/20260927T004352Z.json`, with its adjacent `.events.jsonl` journal. Correctness, SQL diagnostics, and resource samples are under the same artifact root. The checked-in data retains all measured outcomes and embeds the material needed to recreate those ignored artifacts.
