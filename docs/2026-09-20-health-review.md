# Project health review, 2026-09-20

The extraction/query split is sound, and targeted checks found no current bridge failure.
The strongest findings concern bounded results, truthful telemetry, release enforcement,
and the memory cost of the plugin launcher. More tools or another indexing architecture
would not address these findings.

Reviewed code-kb `main` at `49af5eb` and julie-extractors `main` at `475b44d`.
Both checkouts started clean. The audit used five independent investigations, source
review, disposable fixtures, local SQLite telemetry, and live process measurements.
This is a sampled engineering review, not certification of every language or platform.

## Telemetry evidence

The pre-audit cutoff is `2026-09-20 11:38:00 UTC`. The database contains 1,818 calls
from September 13 through September 19, across eight workspaces and twelve recorded
versions. Of those calls, 35 failed, giving a 98.07% recorded success rate.

This history does **not** measure the current release. The source is v1.1.3, while
`target/release/code-kb` and the installed symlink used for live measurements are
v1.1.1. Only two pre-audit calls report v1.1.1; none report v1.1.3.

| Tool | Median | p95 | p99 | Maximum |
|---|---:|---:|---:|---:|
| Skeleton | 1 ms | 10 ms | 86 ms | 5,124 ms |
| Outline | 3 ms | 1,350 ms | 2,318 ms | 2,318 ms |
| Blast radius | 2 ms | 32 ms | 1,026 ms | 1,026 ms |
| Symbol body | 1 ms | 2 ms | 37 ms | 78 ms |
| Context | 8 ms | 48 ms | 51 ms | 51 ms |
| References | 1 ms | 2 ms | 10 ms | 44 ms |

Long calls may include index preparation or freshness checks. The stored timing does
not separate those phases, so the data cannot establish their cause. A mean alone
hides both the fast common case and the slow tail.

Most recorded errors involved absent or ambiguous symbol names. Older errors also
include retired tool names and directory arguments that current code accepts. The
julie-extractors workspace has 77 calls and nine errors, mostly exploratory name
lookups. These are historical observations, not reproduced current bridge defects.

Every historical call records `result_count = 1`, and none records `empty`. That
field counted MCP text blocks rather than query results. Consequently, success rate
could not distinguish a useful answer from a successful search returning no matches.
The token-savings estimate also fabricated a baseline of three times the response
size when no source-file size was available. Historical savings totals remain estimates
with that limitation; they are not measured tokenizer counts or proven cost savings.

## Repairs from this review

| Priority | Finding | Repair |
|---|---|---|
| High | An oversized unsigned result limit could wrap to a negative SQLite `LIMIT`, removing the bound. | Shared 0–200 validation in core queries, MCP input handling and schemas, and CLI arguments. Qualified lookups honor zero. |
| Medium | Qualified lookup bypassed requested symbol-kind and test filters. | Consolidated lookup in the shared query path, retaining ambiguity/error propagation. |
| Medium | Facts and literals each consumed the full requested limit, returning up to twice that limit. | Both interfaces now share the requested allowance between the two result kinds. Literal-query failures are propagated. |
| Medium | Telemetry counted text blocks and invented savings baselines. | Query collections report logical counts; known zero results are recorded as successful empty answers. Old rows remain explicitly unknown. Unknown savings baselines contribute zero. |
| Medium | Compact search and lookup output hid when the requested limit was reached. | Added conservative cap notices. Blast-radius guidance explicitly identifies the CLI JSON escape hatch. |
| High | Tag or manual release workflows could bypass the documented green-CI requirement. | The release workflow now requires successful CI for the exact commit and matching release/manifests versions before builds and publication. |
| Low | The benchmark labeled fresh CLI processes as warm queries and omitted its first-call timing and binary version. | Corrected measurement labels, exposed first-call timings and actual binary version, and rejected invalid iteration counts. |
| Low | The site demonstrated an invalid Cargo command with two positional test filters. | Split the example into two valid commands. |

These are local repairs to existing contracts. No extractor or artifact-schema upgrade, new MCP
tool, parser, background service, or dependency was added. Larger architecture changes
remain separate recommendations below.

Primary implementation locations:

- [Query bounds](../crates/code-kb-core/src/queries.rs),
  [MCP handling](../crates/code-kb-cli/src/mcp/server.rs),
  [CLI handling](../crates/code-kb-cli/src/main.rs), and
  [compact output](../crates/code-kb-core/src/formatters.rs).
- [Telemetry storage and summaries](../crates/code-kb-core/src/telemetry.rs) and
  [internal response metadata](../crates/code-kb-cli/src/mcp/protocol.rs).
- [Release gate](../.github/workflows/release-binaries.yml),
  [its regression test](../tests/plugin/plugin-manifests.test.cjs), and
  [benchmark](../scripts/benchmark_quality.py).

## Extractor bridge

The pin, restored extractor, and development extractor examined here are all v3.1.1.
A real facts-level scan produced schema v7 and artifact contract v4. Scan, incremental
update, extraction level preservation, freshness checks, and syntax rejection passed
targeted checks across both repositories.

The disposable scan covered 113 files, including six unsupported files, and produced
6,475 symbols, 3,007 identifiers, 1,271 type facts, 184 literals, and 1,827 structural
facts. Source regions, type-argument usages, complexity metrics, and annotations were
empty at the facts level, as intended. The single scan took 1.20 seconds; this is a
small-repository observation, not a scaling benchmark.

The remaining 13,213 `reference_sites` rows are not evidence of an easy deletion.
Other fact tables reference them. Removing that storage requires an intentional
schema change, and this review found no reason to force one now.

Build-time discovery stops at the first existing extractor and requires the pin;
runtime discovery searches candidates for a pinned match. This is a development
ergonomics difference, but the stricter build guard can be intentional. It did not
break the tested bridge and was left in place.

## Measured performance and memory

Measurements used Linux and Node v22.23.2. The baseline index held 111 files and
approximately 1.5 MiB of source in a 20 MiB database. Each lifecycle sample initialized
MCP and performed one lookup followed by six identical warm lookups. Four direct and
four launcher samples were collected for both the installed v1.1.1 binary and a fresh
v1.1.3 build in `target/health-review`. The installed binary was not replaced.

| Process | Retained PSS | Retained RSS | Observation |
|---|---:|---:|---|
| Installed v1.1.1 server, sharing executable pages with existing sessions | 12.0–12.1 MiB | 18.4–18.5 MiB | Warm lookup empirical p95 2.41 ms |
| Isolated v1.1.3 audit build | 17.66–17.76 MiB | 19.61–19.77 MiB | Final warm lookup empirical p95 2.72 ms |
| Node launcher parent | About 49.8 MiB | About 53 MiB | Remains alive for the session |
| v1.1.3 server plus launcher | About 67.5 MiB | Not used for the comparison | Combined process-tree PSS |

PSS apportions shared resident pages among processes. RSS counts each process's
resident pages without that apportionment. To control for existing sessions sharing
the old executable, the old v1.1.1 binary was copied to a fresh temporary file and
measured again. It retained 17.64 MiB PSS, versus 17.66 MiB for the isolated v1.1.3
binary. This does **not** demonstrate a memory regression between versions.

The launcher sample preceded the final qualified-lookup guard adjustment. Final direct
samples confirmed the same memory footprint; launcher code did not change.

Both isolated processes exceed the stated full-process sub-15 MB budget. Approximately
12.5 MiB is anonymous/runtime memory and 5.16 MiB is resident executable mapping.
Counting only anonymous memory would change the meaning of the published claim;
this review does not redefine the requirement. The launcher added about 34 ms to
startup and roughly 50 MiB of memory. SQLite tuning cannot remove that parent process.

A process-replacement approach is worth evaluating separately. Node's
[`process.execve`](https://nodejs.org/download/release/latest-jod/docs/api/process.html#processexecvefile-args-env)
is experimental in the measured runtime and unavailable on Windows. It is not a
drop-in replacement for the project's Node 18 and Windows support requirements.

The existing benchmark creates a fresh CLI process for each sample. Its cache-warm
CLI timings and exited-process peak RSS cannot prove persistent MCP latency, retained
memory, launcher process-tree memory, or large-repository performance. Its six
handpicked query checks are useful smoke tests, not a retrieval accuracy benchmark.

## Recommended follow-up priorities

1. Measure and reduce launcher memory while preserving Windows and supported Node
   versions. Settle the memory metric explicitly before changing any advertised budget.
2. Add version-aware telemetry summaries with percentiles and separate startup,
   reconciliation, and query time. Preserve unknown measurements as unknown.
3. Add a repeatable persistent-MCP benchmark and an opt-in large disposable corpus.
   Include changed-file reconciliation and extractor child memory, not just reads.
4. Add a small cross-language answer-quality corpus with expected callers, absent
   matches, ambiguous names, and predicted tests. Passing requests alone cannot measure
   the accuracy of name-based reference resolution.

The existing best-effort reference resolution remains a deliberate limitation.
Predicted tests are suggestions; they do not prove that omitted tests cannot be affected.
No evidence here justifies an in-memory graph, embeddings, a daemon, or extra MCP tools.

## Verification and source state

- `cargo test --workspace --locked`: 247 passed, zero failed across 19 targets.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- `node --test tests/plugin/*.test.cjs`: 16 passed, zero failed.
- Agent-document and plugin-skill synchronization checks: passed.
- Targeted extractor/bridge contract, freshness, and syntax checks: passed.
- Release-gate mocks exercised successful, failed, pending, and missing CI, version
  mismatches, and annotated-tag commit resolution. No release was attempted.
- The benchmark completed three iterations per case, emitted the measured binary
  version and first-invocation timings, and rejected zero iterations with exit code 2.
- Final `git diff --check`: passed.

The combined run initially caught stale formatter expectations and a collapsible-if
lint. Both were corrected before the successful final run; no behavior assertion was
removed or weakened.

All repairs are in the original `/home/murphy/source/code-kb` checkout on `main`.
No alternate worktree was created. `/home/murphy/source/julie-extractors` remains clean
at `475b44d`. The installed symlink and normal release binary still point to v1.1.1;
the audit's v1.1.3 build is isolated under ignored `target/health-review`.

Windows and macOS execution was not repeated locally during this review. The release
gate now requires the existing cross-platform CI result before publication. Memory
measurements above are Linux-only and limited to the stated workload.
