# One-call answers: edit-study measurement

The code-kb arm of the six 2026-10-27 study cases ran again, two repeats each, with
the one-call answers, julie 3.11.0, and the new guidance. [protocol-v3.md](protocol-v3.md)
records each change to the method.

## Pass mark

| Check | Limit | Result | Verdict |
| --- | ---: | ---: | --- |
| MCP calls per trial | at most 16.47 | 11.17 (134 in total; 2.11.x 247) | pass |
| Acceptance | 12/12 | 12/12 | pass |
| Input tokens, cached included | at most 8,506,310 | 8,566,530 | fail (0.7% over) |

## Other measures

| Measure | 2.11.x | This run |
| --- | ---: | ---: |
| Shell commands | 181 | 177 |
| Output tokens | 66,228 | 63,016 |
| Input tokens without cache | 673,094 | 547,458 |
| Cached input tokens | 7,833,216 | 8,019,072 |
| Agent plus index time, 11 timed trials | 1,692 s | 1,505 s |

Per-trial numbers are in [results/metrics.json](results/metrics.json). MCP calls come
from `trace_metrics.code_kb_calls`, the same field that gave the 2.11.x count.

## Limits

- Two repeats per case. The token miss is smaller than the spread between repeats of one case.
- No native arm ran, so a change in the model between October 27 and 31 cannot be ruled out.
- The engine, the extractor, and the guidance all changed, so this measures the whole workflow.
- Codex CLI 0.164.0 is the pinned executable from the cache, as in the frozen study.
