# Toolkit efficiency confirmation result

The shorter initialize instructions are rejected. All 36 patches were independently accepted, but the candidate failed both frozen efficiency comparisons. The original server instructions and exact handshake assertion are restored byte-for-byte to `f290fdca44cec942ae6f50265e21da9f37fca836`. The trial remains available as [candidate.patch](candidate.patch), source commit `3dc4d5d8c8668740bfb9b3a74b95facd1f2bdaf3`, its preserved executable, and [disposition record](candidate-disposition.json). No performance optimization is retained and no whole-task savings claim is established.

## Fixed comparisons

Six new tasks ran twice per arm, in 12 matched sets: 36 actual model trials. Every arm accepted 12/12 patches, including 2/2 for each individual task. Protected-input violations and source-review rejections: zero. All required token and index-inclusive timing fields are present for all 36 trials. The sample and thresholds were frozen before exposure in the [protocol](../protocol.md).

| Metric | Native | Current code-kb | Candidate |
|---|---:|---:|---:|
| Accepted patches | 12/12 | 12/12 | 12/12 |
| Total input tokens | 3,758,042 | 6,518,315 | 7,461,801 |
| Cached input tokens | 3,430,656 | 5,969,408 | 6,948,864 |
| Uncached input tokens | 327,386 | 548,907 | 512,937 |
| Output tokens | 55,665 | 61,008 | 62,794 |
| Shell calls | 168 | 150 | 154 |
| MCP calls | 0 | 114 | 114 |
| All tool calls | 168 | 264 | 268 |
| Trace errors (supplemental) | 27 | 38 | 40 |
| Agent seconds | 1,434.958 | 1,709.523 | 1,894.106 |
| Index-inclusive seconds | 1,434.958 | 1,737.845 | 1,917.640 |
| Cold-index seconds | N/A | 28.321698 | 23.533404 |

| Gate | Quality | Total input change | Uncached input change | Index-inclusive time change | Result |
|---|---|---:|---:|---:|---|
| Candidate vs current | Pass, overall and every task | +14.47% | -6.55% | +10.35% | Fail |
| Candidate vs native | Pass, overall and every task | +98.56% | +56.68% | +33.64% | Fail |

Retention required total and uncached input each at most 95% of current, time at most 105%, and no quality regression. Only uncached input met its efficiency threshold. The separate native savings gate uses the same limits and also failed. Five percent is a practical threshold, not statistical significance. These are observed totals from a small sample; they do not isolate the causal effect of shortening instructions.

Agent time includes the edit session; index-inclusive time adds the measured missing-index scan for code-kb. Native indexing is not applicable. Preparation, dependency setup, grading and independent public-test replays are outside these timings for every arm. Cold means no project index; operating-system and provider caches were not flushed. Shell calls are the runner's `command_count`, not the number of commands chained inside a shell invocation. Both code-kb arms made 114 MCP calls. Error counts are retained descriptive trace fields, not an added gate. Billing is unavailable. An [independent audit and guarded recomputation](analysis-verification.json) reproduce the final metrics byte-for-byte.

## Every matched set

Each cell gives the actual ordinal and manifest schedule slot; the native result is reused by reference for both comparisons, never dispatched twice. All three results in every row are accepted. Full hashes, all metrics, source reviews and missing-field lists are in [metrics.json](metrics.json) and [trial-metrics.csv](trial-metrics.csv).

| Matched set | Native ordinal/slot | Current ordinal/slot | Candidate ordinal/slot | Accepted N/C/K |
|---|---|---|---|---|
| flask-config-r1 | 1 / current:1 | 2 / current:2 | 3 / candidate:2 | Yes / Yes / Yes |
| express-route-r1 | 4 / current:3 | 5 / current:4 | 6 / candidate:4 | Yes / Yes / Yes |
| minijinja-config-r1 | 7 / current:5 | 8 / current:6 | 9 / candidate:6 | Yes / Yes / Yes |
| hcl-config-r1 | 10 / current:7 | 11 / current:8 | 12 / candidate:8 | Yes / Yes / Yes |
| commons-lang-r1 | 13 / current:9 | 14 / current:10 | 15 / candidate:10 | Yes / Yes / Yes |
| nodatime-r1 | 16 / current:11 | 17 / current:12 | 18 / candidate:12 | Yes / Yes / Yes |
| flask-config-r2 | 21 / current:14 | 20 / current:13 | 19 / candidate:13 | Yes / Yes / Yes |
| express-route-r2 | 24 / current:16 | 23 / current:15 | 22 / candidate:15 | Yes / Yes / Yes |
| minijinja-config-r2 | 27 / current:18 | 26 / current:17 | 25 / candidate:17 | Yes / Yes / Yes |
| hcl-config-r2 | 30 / current:20 | 29 / current:19 | 28 / candidate:19 | Yes / Yes / Yes |
| commons-lang-r2 | 33 / current:22 | 32 / current:21 | 31 / candidate:21 | Yes / Yes / Yes |
| nodatime-r2 | 36 / current:24 | 35 / current:23 | 34 / candidate:23 | Yes / Yes / Yes |

## Runtime and input identities

| Runtime | Version | Source commit | Executable SHA-256 |
|---|---|---|---|
| Current code-kb | code-kb 2.12.0 | `f290fdca44cec942ae6f50265e21da9f37fca836` | `d3f913db2025339a3496f85017eeaee98542f78afa2f654aacbdf2cc108221d2` |
| Trial candidate | code-kb 2.12.0 | `3dc4d5d8c8668740bfb9b3a74b95facd1f2bdaf3` | `38674d8ba835960335f4120acb3822a6b7105eb2672e173b0454dce3e0a68e75` |
| Julie, both code-kb arms | julie-extract 3.11.0 | Pinned release; source commit unavailable in the executable receipt | `008372ec7a61f0921ef687d20e24c67547255f603d7dbca08710f8e27bef28bc` |
| Codex, all arms | 0.164.0 | Vendor executable; source commit unavailable | `b93f6811d3f653f0fb83be4050d857fb9abbb111dfa2ce160563327daff7f306` |

All trials used `gpt-6.1-sol`, high effort, a 900-second limit, offline task dependencies and the same sandbox/capture/grading boundaries. Provider weights/backend revision and a sampling seed are unavailable; the model alias does not freeze them. Companion executable, redactor, toolchain, dependency and guidance hashes are bound by [current v2](../manifest-v2.json), [candidate v2](../manifest-candidate-v2.json), their [control records](../control-current-v2.json), and the per-trial records. Preserved runtime paths are in [current identity](../runtime-current.json) and [candidate identity](../runtime-candidate.json).

The one candidate shortened the shared initialize paragraph from 480 to 158 characters. Real stdio initialize/tools-list comparison showed seven identical tool definitions and no other initialize-field change; see [schema comparison](../schema-comparison.json). The [trace finding](../finding.md) distinguishes captured client metadata from a historical repetition projection. Character reduction alone was never a savings measurement.

## Independent acceptance and retained incidents

Each [source-review record](source-review/) binds the unchanged attempt and patch hashes. The lead inspected all production diffs, valid controls and edge cases. Canonical public and hidden checks passed. Existing fresh public replays executed the actual added Python/JavaScript/Rust/Go tests. Java/C# canonical filters omitted their new classes, so separate fresh public-only replays ran all actual added classes: [Java 41 tests](source-review/replay-commons-lang/) and [C# 190 tests](source-review/replay-nodatime/), all passing with no hidden overlay or patch repair. Reference test names were not acceptance conditions. Style observations on immutable model data were unscored.

The first native trial completed once. The next current slot stopped before model exposure because two case metadata files omitted the managed `.code-kb` generated root. [Setup v2](../setup-v2/protocol-amendment.md) adds only that exclusion; [equivalence proof](../setup-v2/equivalence.json) binds identical task prompts, source, tests, protections and native output. All six real preindex paths then passed. The original native control and unexposed stop remain in [initial records](initial-records/); only the unexposed slot resumed. A full `/tmp` quota was resolved by the owner's approved, verified backup-and-symlink relocation. The private `/tmp` prototype was not used for scored trials. The original native timing is retained; no effect of the host incident is inferred.

Four HCL bundles retain authentic `go test ./...` generated executables. Their source provenance, sizes and hashes are recorded without dropping or modifying them. An early analysis import missed three completed Java acceptance records because replay-confirmed note metadata moved fields. The [interim summary](metrics-before-final-acceptance.json) correctly remained inconclusive; the importer was repaired and final aggregation includes all 36 acceptance records. No model attempt, grader, case, threshold or measurement changed.

## Scope and final tree

The six tasks cover Python, JavaScript, Rust, Go, Java and C#. They are new tasks on repositories already used in earlier studies, not unseen repositories or training data. The [corpus](../corpus.json) binds every pin. The historical evaluation trees remain unchanged. This sample cannot establish performance or quality across all 42 supported language IDs.

Only the two source-backed skill clarifications in the [guidance audit](../documentation-audit.md) accompany the evidence. They were made after the timed campaign and are unmeasured; they make no efficiency claim. No tool, schema, reference/index/language contract or Stop behavior changes are retained. Dotenv extraction remains unproven and outside this plan's extractor scope; no support claim was added.

[Archive identities and verified member inventories](archive.json) preserve redacted attempts, patch blobs, seeds, readiness and replay evidence in the task cache, excluding credentials and build/work state. [Verification](../verification.md) records the branch gate and source-state evidence. Work remains local on `toolkit-efficiency`; no push, release or global installation was performed.
