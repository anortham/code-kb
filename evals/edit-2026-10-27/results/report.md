# Completed-edit comparison

Status: the original cohort is complete. The four TypeScript trials remain
unexposed while Windows watcher verification is finished.

The original cohort produced more accepted patches with the code-kb workflow,
but used more tokens and took longer. This is evidence about the complete
workflow, including its guidance, rather than the effect of the binary alone.

## Method

Six pinned public repositories cover Python, Rust, Go, TypeScript, Java and C#.
Each case has native and code-kb arms, with two counterbalanced repeats. Both
arms use pinned Codex CLI 0.164.0, `gpt-6.1-sol`, high effort and a 900-second
deadline. The native arm has no code-kb executable or index. Preparation,
agent commands and grading have separate mounts; graders have no provider
credentials or network access. Hidden checks and reference repairs are not
exposed to agent commands.

The original 24 slots yielded 20 model trials and four TypeScript setup stops
before provider exposure. Those stops are infrastructure outcomes, not failed
coding attempts. The amended protocol selects only those four cells in their
original within-case order. Completed model trials are never rerun.

The engine and extractor remain fixed across both cohorts. Dependency setup,
grading and independent source review are outside the reported agent
time; indexing is included in the paired time comparison. Slot 15 completed
its model turn before a controller interruption, but its agent and indexing
times are unavailable. Its pair is excluded from timing comparisons.

## Original cohort

| Measure | Native | code-kb |
| --- | ---: | ---: |
| Actual model trials | 10 | 10 |
| Source and corrected behavioral acceptance | 8 | 10 |
| Original strict behavioral passes | 7 | 6 |
| Reported input tokens, including cached input | 3,350,481 | 7,667,056 |
| Cached input tokens | 3,032,192 | 7,069,568 |
| Input tokens excluding cached input | 318,289 | 597,488 |
| Output tokens | 41,067 | 57,304 |

Across nine pairs with complete timings, code-kb took a mean 78.14 seconds
longer including indexing; the median difference was 63.10 seconds. Billing is
unavailable. These measurements do not demonstrate task-wide token savings.

Both rejected native patches are NodaTime changes missing required `[Pure]`
annotations. The corresponding code-kb patches include them. This difference
is concentrated in one case and does not establish a general success-rate
improvement across languages or repositories.

All ten code-kb trials used MCP, with 188 calls. They also issued 160 shell
commands, including four classified structural fallbacks. Native trials issued
137 shell commands. Command counts describe actions; they are not equivalent
units of work. Literal text searches are allowed, and uncertain classifications
remain explicit in the trace receipts.

## Corrections and evidence

The original strict grades remain preserved. All four MiniJinja patches pass
uniform regrading; three were false negatives from the help-wording checker.
Both code-kb C# patches pass after allowing additional passing tests above the
baseline counts. The two native C# patches still fail the existing protected
purity test. No captured implementation was repaired to improve its result.

The versioned cases declare canonical generated outputs precisely. Fresh
readiness passes with source-equivalent seeds and the original dependency
hashes. A modified Rust dependency cache was preserved and restored before
the final readiness freeze. Original and amended manifests, raw failures,
recovery history and corrected grades remain separate.

- [Original metrics](adjudication/original-cohort-metrics.json)
- [Uniform corrected grades](adjudication/corrected-regrades.json)
- [Source reviews](source-review/)
- [MCP trace analysis](trace-review/mcp-traces.md)
- [Original replayable captures](controls/original-attempts/identity.json)
- [Amended input freeze](../manifest-v2.json)
- [Readiness archive](readiness-v2/archive-identity-v2.json)

The product's language contract remains all 42 Julie language IDs. The fixture
suite checks extraction, navigation, references, impact and refresh across that
set; this small agent study measures six selected cases and cannot establish
agent performance across all 42 languages. Global routing remains unchanged.
