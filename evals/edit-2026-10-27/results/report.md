# Completed-edit comparison

The study is complete: 24 actual model trials across six repositories.
The code-kb workflow produced 12 accepted patches out of 12; native tools
produced 10 out of 12. Code-kb used more tokens and took longer. Both rejected
native patches came from one C# case, so this sample does not establish a
general success-rate improvement.

## Method

Six pinned public repositories cover Python, Rust, Go, TypeScript, Java and C#.
Each case has native and code-kb arms, with two counterbalanced repeats. Both
arms use pinned Codex CLI 0.164.0, `gpt-6.1-sol`, high effort and a 900-second
deadline. The native arm has no code-kb executable or index. Preparation,
agent commands and grading have separate mounts; graders have no provider
credentials or network access. Hidden checks and reference repairs are not
exposed to agent commands. Source review includes independently running the
patches' added tests on fresh public replays.

The original 24 slots yielded 20 model trials and four TypeScript setup stops
before provider exposure. Those stops remain preserved infrastructure outcomes.
The amended protocol selected only those four unexposed cells, in their
original within-case order: native repeat 1, code-kb repeat 1, code-kb repeat 2,
native repeat 2. All four subsequently completed. No completed model trial was
rerun or repaired to improve its result.

The engine and extractor stayed fixed across both cohorts. This measures the
complete workflow, including guidance, rather than the effect of the binary
alone. Preparation, grading and independent source review are outside agent
time; indexing is included in the paired comparison. Slot 15 completed its
model turn before a controller interruption, but its agent and indexing times
are unavailable. Its pair is excluded from timing comparisons.

## Combined results

| Measure | Native | code-kb |
| --- | ---: | ---: |
| Actual model trials | 12 | 12 |
| Source and corrected behavioral acceptance | 10 | 12 |
| Reported input tokens, including cached input | 3,883,381 | 8,506,310 |
| Cached input tokens | 3,500,160 | 7,833,216 |
| Input tokens excluding cached input | 383,221 | 673,094 |
| Output tokens | 48,493 | 66,228 |

Across eleven pairs with complete timings, code-kb took a mean 66.67 seconds
longer including indexing; the median difference was 31.83 seconds. Billing is
unavailable. These measurements do not demonstrate task-wide token savings.
Response-size comparisons against full-file reads measure something different.

Both rejected native patches are NodaTime changes missing required `[Pure]`
annotations. The corresponding code-kb patches include them. All four
TypeScript patches make the same two production changes and pass behavioral
checks, independent source review and their added tests. Those added tests
passed 59 checks across seven files; protected inputs remained unchanged.

All twelve code-kb trials used MCP, with 247 calls. They also issued 181 shell
commands, including four classified structural fallbacks. Native trials issued
165 shell commands. Command counts describe actions; they are not equivalent
units of work. Literal text searches are allowed. Repair classifications and
uncertain cases remain explicit in the trace receipts.

## Original results and corrections

The original 20-trial cohort had source and corrected behavioral acceptance
of 8/10 native and 10/10 code-kb. Original strict behavioral grades were 7/10
and 6/10 respectively, totaling 13/20. Those grades remain unchanged. Across
its nine timed pairs, code-kb took a mean 78.14 seconds longer; the median
difference was 63.10 seconds.

All four MiniJinja patches pass uniform regrading; three were false negatives
from the help-wording checker. Both code-kb C# patches pass after allowing
additional passing tests above baseline counts. The two native C# patches
still fail the existing protected purity test. No captured implementation was
modified during adjudication.

Versioned cases declare canonical generated outputs precisely. Fresh readiness
passes with source-equivalent seeds and the original dependency hashes. A
modified Rust dependency cache was preserved and restored before the final
readiness freeze. Original and amended manifests, failures, recovery history
and corrected grades remain separate. The supplemental launcher initially
rejected a relative artifact-map path before campaign creation or provider
exposure; the corrected absolute-path invocation completed all four cells.

One supplemental code-kb call refused a fresh-index claim when a non-root
`.codex` watch directory vanished; the next `check_edit` succeeded. The exact
error and recovery are retained. This observation comes from the frozen study
engine, separately from the later watcher runtime correction and its regression
evidence. It is not a silently successful stale answer.

## Runtime and scope

The product contract remains all 42 pinned Julie language IDs. Linux passes
1,906 Rust tests and Windows passes 1,878, including the language conformance
suite, plus Clippy, formatting and locked release builds. The unchanged Python
suite passes 301. The later watcher correction has ten passing Windows
move/delete/recreate repetitions and a live 20-cycle churn probe with 112
handles before and after. The Windows VM is confirmed off. The study binaries
were not replaced by this later runtime build.

The fixture suite checks extraction, navigation, references, impact and refresh
across all 42 IDs. Six selected tasks with two repeats cannot establish agent
performance across that full set. Global routing remains unchanged.

## Direction

Keep the seven-tool interface and the cross-language contract. The next
experiment should test a lighter navigation workflow, using these traces to
identify repeated context and unnecessary calls. Treat these six cases as
regression and development evidence after any tuning. Reserve untouched tasks
for confirmation, including literal/configuration and framework-entry-point
work absent from this sample. Compare accepted edits, uncached and total input,
repair evidence and indexing-inclusive time before changing routing or making
an efficiency claim.

## Evidence

- [Combined metrics](adjudication/final-cohort-metrics.json)
- [Original metrics](adjudication/original-cohort-metrics.json)
- [Uniform corrected grades](adjudication/corrected-regrades.json)
- [Original source reviews](source-review/)
- [Supplemental TypeScript source reviews](source-review/date-fns-v2/)
- [Original MCP trace analysis](trace-review/mcp-traces.md)
- [Supplemental trace analysis](trace-review/typescript-v2/)
- [Original replayable captures](controls/original-attempts/identity.json)
- [Supplemental replayable captures](controls/supplemental-attempts/identity.json)
- [Amended input freeze](../manifest-v2.json)
- [Readiness archive](readiness-v2/archive-identity-v2.json)
- [Current runtime verification](controls/runtime-backport/identity.json)
