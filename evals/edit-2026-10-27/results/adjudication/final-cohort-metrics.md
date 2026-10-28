# Final cohort metrics

The 24 actual model trials have 22 accepted patches: native 10/12 and code-kb 12/12.
The four original TypeScript setup stops remain separate historical outcomes;
only those previously unexposed cells received supplemental calls.

| Measure | Native | code-kb |
| --- | ---: | ---: |
| Accepted/model trials |10/12|12/12|
| Input tokens including cache |3,883,381|8,506,310|
| Cached input tokens |3,500,160|7,833,216|
| Uncached input tokens |383,221|673,094|
| Output tokens |48,493|66,228|

Across eleven complete pairs, code-kb minus native indexing-inclusive time averages
66.665252 seconds, with median 31.826503 seconds. The slot 15 pair is excluded
because recovered agent/index times are unavailable.

Original strict grades remain 13/20; effective source/behavior acceptance is
18/20 for the original cohort and 4/4 for the supplemental TypeScript cohort.
Both rejected native patches omit Pure annotations in the same C# case.
This measures the complete workflow and guidance, with no demonstrated
task-wide token saving or global routing change. See the [report](../report.md)
and [machine-readable metrics](final-cohort-metrics.json) for evidence.

The [supplemental trace review](../trace-review/typescript-v2/typescript-v2-traces.md)
records 59 MCP calls, seven caps, four expected ambiguity selections, two body
misses and one recovered freshness refusal. Six shell actions remain mixed or
unknown. No supplemental structural fallback or implementation repair was
confirmed. Original repair uncertainties are retained in the JSON.
