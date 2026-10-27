# Fresh retrieval and test-reach cases

These source-backed task definitions are reserved for a future controlled comparison. They are
separate from the historical reach corpus and bug-replay regressions. No model or paid agent
comparison has run on them.

Give an agent `tasks.json` and the `fixture/` tree. Keep `oracles.json` hidden until grading. Score
whether it identifies the requested implementation, the actual indirect caller, the focused test,
and a runnable focused command when one is present. Record an incorrect test path separately from
a correct full-suite recommendation. Do not count a short retrieval response as a successful task.

The `fixture/` tree is small, intentionally mixed-language source. Run real extraction before
using the cases; `evals/review-2026-10-26/verify-extraction.sh` uses the pinned `julie-extract` at
the `references` level and checks the definitions, static calls, and test calls that extraction
actually records. The Python framework route and the CommonJS handler binding have no extracted
end-to-end call edge from their test entry points. The JavaScript app has an extracted `variable_ref`
to the handler, which does not by itself prove framework dispatch. These are recorded evaluation
gaps, not claims about code-kb output. The script writes only to a temporary directory.

The task inputs cover seven code languages plus a JSON literal/config case. They define cases, not
results. The plan in
`docs/plans/2026-09-23-retrieval-value-and-optional-routing.md` defines the three comparison arms,
repeats, order, and outcome measurements. Copy `run-log-template.csv` into the experiment output
folder and append one row for every run. Leave unavailable metrics blank and explain that in
`metric_availability`; the file contains only a header, not measured results. Paid runs require
separate budget approval.
