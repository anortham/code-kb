# Planted-bug replay baseline: code-kb 2.9.0 and `feat/test-reach`

The planted-bug replay (`scripts/bug_replay.py`, `evals/bug-replay/README.md`) scored three
binaries on 2026-10-22, all with julie-extract 3.9.1:

- the 2.9.0 release (commit `04a23c4`, binary `f3b67583`);
- the warnings build: open-end and depth warnings (commit `33dc270`, binary `6bd60606`);
- the reach build: the level walk, the JavaScript receiver step, and diff seeds with callers
  reports (binary `35231489`).

The truth is 96 bugs that broke at least one test: cobra 39 (22 dev, 17 holdout), express 31
(15, 16), and flask 26 (10, 16). Each answer is `blast_radius --json --limit 200` at the default
depth, 2. The symbol seed is the function around the bug; the diff seed is the uncommitted bug.

## Marks

The plan (`docs/plans/2026-10-22-test-reach.md`) set two marks for the warnings. An answer that
"lists tests" names at least one test and is not cut by `limit` or the 200-row ceiling.

| Half | Mark | 2.9.0 | Warnings build | Reach build |
|---|---|---|---|---|
| dev | no silent miss in any repository | 28 of 44 | 0 of 45 | 0 of 47 |
| dev | needless warnings at most 20% of the answers that list tests | none warn | 7 of 45 (16%) | 5 of 47 (11%) |
| holdout | no silent miss in any repository | 32 of 46 | 0 of 43 | 0 of 58 |
| holdout | needless warnings at most 20% of the answers that list tests | none warn | 10 of 43 (23%) | 9 of 58 (16%) |

- A silent miss lists tests, misses a failing test, and does not warn. A needless warning misses
  no failing test.
- Two fixes came from holdout cases, so neither holdout PASS is a clean holdout result:
  - The warnings build first gave 1 silent miss. The Cobra test
    `TestFlagCompletionConcurrentRegistration` calls the target through `cmd := rootCmd` inside
    `go func() {…}()`. The callers report labels the call `likely`, and the fallback dropped the
    report's tests when the walk had found other tests. The fix adds them after the walk's tests.
  - Diff seeds without callers reports gave 3 silent misses (1 Cobra, 2 Express holdout). Symbol
    answers for the same functions add the report's tests and did not miss. The fix gives diff
    answers the same step.
- Every needless warning is true. An open end that it names has no indexed caller, and a depth line
  counts tests that a depth-5 walk lists. The bug did not break those tests.

## Reach build changes

- **Level walk.** The walk finds callers one level at a time and expands each symbol once, where
  the recursive query expanded a symbol once for each depth at which a path reached it. On 300
  calls on Flask, Cobra, code-kb, Gson, and Newtonsoft.Json the answers are the same, except 2
  whose depth-5 count is no longer capped at 200 tests. The depth-5 count and the open-end check
  reuse the walk.
- **JavaScript receiver step.** julie stores `res.send()` only as an identifier. The walk now
  follows such a call when its receiver names the object the target is assigned on
  (`res.send = function`), or is `this` in a function assigned on that object, and no other
  definition is assigned on an object of that name. Express holdout symbol answers missed 57% of
  the failing tests, against 71%.
- **Diff seeds.** A diff answer walked from every symbol of each changed file. It now walks from
  the innermost function, method, or type that holds each changed line, and from the whole file
  only when a hunk touches module-level lines. It adds the callers-report tests of each changed
  function, as symbol answers do, when at most 5 functions changed.

## Holdout half

| Repo | Seed | Bugs | Failing tests missed (warnings → reach) | Median listed (warnings → reach) | Lists tests | Silent misses | Needless |
|---|---|---|---|---|---|---|---|
| cobra | diff | 17 | 84/134 (63%) → 114/134 (85%) | 63% → 0% | 11 | 0 | 1 |
| cobra | symbol | 16 | 112/132 (85%) → 112/132 (85%) | 0% → 0% | 10 | 0 | 1 |
| express | diff | 16 | 317/317 (100%) → 181/317 (57%) | 0% → 3% | 11 | 0 | 2 |
| express | symbol | 16 | 224/317 (71%) → 181/317 (57%) | 1% → 3% | 11 | 0 | 2 |
| flask | diff | 16 | 184/428 (43%) → 86/428 (20%) | 54% → 0% | 8 | 0 | 1 |
| flask | symbol | 16 | 221/428 (52%) → 221/428 (52%) | 0% → 0% | 7 | 0 | 2 |

## Dev half

| Repo | Seed | Bugs | Failing tests missed (warnings → reach) | Median listed (warnings → reach) | Lists tests | Silent misses | Needless |
|---|---|---|---|---|---|---|---|
| cobra | diff | 22 | 81/136 (60%) → 123/136 (90%) | 7% → 0% | 13 | 0 | 1 |
| cobra | symbol | 21 | 121/134 (90%) → 121/134 (90%) | 0% → 0% | 12 | 0 | 1 |
| express | diff | 15 | 1101/1101 (100%) → 916/1101 (83%) | 0% → 1% | 8 | 0 | 1 |
| express | symbol | 13 | 928/1091 (85%) → 944/1091 (87%) | 0% → 0% | 8 | 0 | 1 |
| flask | diff | 10 | 227/343 (66%) → 338/343 (99%) | 56% → 0% | 3 | 0 | 1 |
| flask | symbol | 10 | 338/343 (99%) → 338/343 (99%) | 0% → 0% | 3 | 0 | 0 |

- "Median listed" is the share of the suite that the median answer lists. "Lists tests",
  "Silent misses", and "Needless" are the reach build's counts.
- 2.9.0 missed the same tests as the warnings build, except 1 more on the cobra symbol holdout
  row and 5 more on the express symbol holdout row.
- The Cobra and Flask diff rows miss more failing tests than before. A whole-file walk listed 54%
  to 63% of the Flask and Cobra holdout suites, so it held many failing tests by chance. The
  narrow walk misses what the symbol answer for the same function misses, and those answers are
  empty or warn: none of them is a silent miss.
- The express symbol dev row misses 16 more tests. `app.set` reached 441 tests through the callers
  report, many through name matches of supertest's own `.set()`. The walk now ties `app.set` to
  its real callers and stops at `express()`, which the index does not tie to `createApplication`;
  the answer warns that the walk stopped at depth 2.

## What the warnings do not change

The warnings make an incomplete list say so; they add few tests. The tests that the answers miss
come from these gaps in reach:

- **Depth.** Cobra tests reach most code through `Execute` and `ExecuteC`, more than 2 calls away.
  At depth 2 the symbol answers miss 85% to 90% of the failing tests. The depth line says how
  many more tests depth 5 lists.
- **Runtime entry points.** Flask tests reach request code through the test client, which calls
  `Flask.__call__`; `with` calls `__enter__` and `__exit__`. No indexed code calls these methods,
  so they are open ends. The symbol answers miss 52% (holdout) and 99% (dev) of the failing tests.
- **JavaScript module calls.** Express tests build the app with `express()`, a call of the binding
  that `require('..')` returns. The index does not tie it to `createApplication`, the function that
  `module.exports` names, so walks that reach `createApplication` stop there.

## Truth corrections

- One mocha process runs every Express test file, so an error that a bug throws can fail tests in
  later files. `recheck` ran each failing file alone with the bug and kept the tests that still
  failed. 5 of 33 bugs lost tests: three kept some (506 to 7, 289 to 33, 67 to 14), and two kept
  none and left the set.
- Flask commit `954f5684` cannot import with its own locked packages, so it stays out.

## Cost

`scripts/blast_perf.py` runs two binaries in turn on 30 targets of each corpus project (the
symbols with the most calls to their name, a seeded sample of functions and methods, and the
largest files), three times each, and reports the wall time and the peak memory of each call.
2.9.0 against the reach build, `--depth 2`:

| Repo | 2.9.0 p50 / p95 / max (s) | Reach build p50 / p95 / max (s) |
|---|---|---|
| flask | 0.053 / 0.320 / 0.329 | 0.063 / 0.318 / 0.327 |
| express | 0.046 / 0.135 / 0.321 | 0.044 / 0.127 / 0.135 |
| zod | 0.081 / 0.279 / 0.342 | 0.088 / 0.282 / 0.289 |
| gson | 0.153 / 2.537 / 4.373 | 0.218 / 1.982 / 2.905 |
| cobra | 0.038 / 0.099 / 0.346 | 0.047 / 0.115 / 0.268 |
| newtonsoft-json | 0.166 / 3.375 / 3.983 | 0.186 / 1.992 / 2.585 |
| code-kb | 0.040 / 0.100 / 0.110 | 0.045 / 0.109 / 0.118 |

- The warnings build was slower: Newtonsoft.Json p95 11.7 s and Gson p50 0.55 s, because the
  depth-5 count ran a second walk from the start and the open-end check ran one query for each
  reached symbol. The level walk removed both costs.
- The median rises by 5 to 65 ms, the depth-5 count where a caller lies past the depth. p95 and
  the slowest calls fall on the large projects.
- Peak memory per call is the same within 1 MiB on every project. A live `serve` process on
  Newtonsoft.Json kept 18 MB after the heaviest calls with both builds; its peak rose from 66 to
  73 MB.
- The median replay case (three calls, with the index refresh) took 1.1 s on cobra, 0.9 s on
  express, and 1.05 s on flask, against 1.4, 0.9, and 1.35 s with the warnings build.
