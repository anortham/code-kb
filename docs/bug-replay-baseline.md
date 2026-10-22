# Planted-bug replay baseline: code-kb 2.9.0 and `feat/test-reach`

The planted-bug replay (`scripts/bug_replay.py`, `evals/bug-replay/README.md`) scored two binaries
on 2026-10-22, both with julie-extract 3.9.1:

- the 2.9.0 release (commit `04a23c4`, binary `f3b67583`);
- the `feat/test-reach` build with the open-end and depth warnings (binary `6bd60606`).

The truth is 96 bugs that broke at least one test: cobra 39 (22 dev, 17 holdout), express 31
(15, 16), and flask 26 (10, 16). Each answer is `blast_radius --json --limit 200` at the default
depth, 2. The symbol seed is the function around the bug; the diff seed is the uncommitted bug.

## Marks

The plan (`docs/plans/2026-10-22-test-reach.md`) set two marks for the warnings. An answer that
"lists tests" names at least one test and is not cut by `limit` or the 200-row ceiling.

| Half | Mark | 2.9.0 | Branch | Result |
|---|---|---|---|---|
| dev | no silent miss in any repository | 28 of 44 | 0 of 45 | PASS |
| dev | needless warnings at most 20% of the answers that list tests | none warn | 7 of 45 (16%) | PASS |
| holdout | no silent miss in any repository | 32 of 46 | 0 of 43 | PASS, after a fix found on this half |
| holdout | needless warnings at most 20% of the answers that list tests | none warn | 10 of 43 (23%) | FAIL |

- A silent miss lists tests, misses a failing test, and does not warn. A needless warning misses
  no failing test.
- The holdout half first gave 1 silent miss. The Cobra test `TestFlagCompletionConcurrentRegistration`
  calls the target through `cmd := rootCmd` inside `go func() {…}()`. The walk cannot type `cmd`,
  but the callers report labels the call `likely`, and the fallback dropped the report's tests
  when the walk had found other tests. The fix adds them after the walk's tests. That fix came from
  a holdout case, so the holdout PASS is not a clean holdout result.
- Every needless warning is true. An open end that it names has no indexed caller, and a depth line
  counts tests that a depth-5 walk lists. The bug did not break those tests. Of the 17 needless
  warnings on both halves, 6 are depth lines only, 7 name open ends only, and 4 have both.

## Holdout half

| Repo | Seed | Bugs | Failing tests missed | Lists tests | Silent misses 2.9.0 | Silent misses branch | Warned (branch) | Needless (branch) |
|---|---|---|---|---|---|---|---|---|
| cobra | diff | 17 | 84/134 (63%) | 7 | 5 | 0 | 16 | 1 |
| cobra | symbol | 16 | 112/132 (85%) | 10 | 9 | 0 | 9 | 1 |
| express | diff | 16 | 317/317 (100%) | 0 | 0 | 0 | 16 | 0 |
| express | symbol | 16 | 224/317 (71%) | 11 | 10 | 0 | 14 | 3 |
| flask | diff | 16 | 184/428 (43%) | 8 | 4 | 0 | 15 | 3 |
| flask | symbol | 16 | 221/428 (52%) | 7 | 4 | 0 | 10 | 2 |

## Dev half

| Repo | Seed | Bugs | Failing tests missed | Lists tests | Silent misses 2.9.0 | Silent misses branch | Warned (branch) | Needless (branch) |
|---|---|---|---|---|---|---|---|---|
| cobra | diff | 22 | 81/136 (60%) | 15 | 10 | 0 | 21 | 4 |
| cobra | symbol | 21 | 121/134 (90%) | 12 | 6 | 0 | 7 | 1 |
| express | diff | 15 | 1101/1101 (100%) | 4 | 4 | 0 | 14 | 0 |
| express | symbol | 13 | 928/1091 (85%) | 6 | 4 | 0 | 12 | 1 |
| flask | diff | 10 | 227/343 (66%) | 5 | 4 | 0 | 10 | 1 |
| flask | symbol | 10 | 338/343 (99%) | 3 | 0 | 0 | 7 | 0 |

"Failing tests missed" is the branch's count. 2.9.0 missed the same tests, except 1 more on the
cobra symbol holdout row and 5 more on the express symbol holdout row.
"Warned" counts every answer with a warning, also an empty or cut one.

## What the warnings do not change

The warnings make an incomplete list say so; they add few tests. The tests that the answers miss
come from three gaps in reach:

- **Depth.** Cobra tests reach most code through `Execute` and `ExecuteC`, more than 2 calls away.
  At depth 2 the symbol answers miss 85% to 90% of the failing tests. The depth line now says how
  many more tests depth 5 lists.
- **Runtime entry points.** Flask tests reach request code through the test client, which calls
  `Flask.__call__`; `with` calls `__enter__` and `__exit__`. No indexed code calls these methods,
  so they are open ends. The symbol answers miss 52% (holdout) and 99% (dev) of the failing tests.
- **JavaScript member calls.** julie stores `res.send()` only as an identifier. The walk follows
  resolved and pending calls, so it reaches no mocha test in Express. Symbol answers list only the
  tests that the callers report ties to the target, and diff answers miss every failing test.

## Truth corrections

- One mocha process runs every Express test file, so an error that a bug throws can fail tests in
  later files. `recheck` ran each failing file alone with the bug and kept the tests that still
  failed. 5 of 33 bugs lost tests: three kept some (506 to 7, 289 to 33, 67 to 14), and two kept
  none and left the set.
- Flask commit `954f5684` cannot import with its own locked packages, so it stays out.

## Cost

The open-end check and the depth-5 probe add 0 to 20 ms per call: on the code-kb index (263 MB),
`get_symbol_by_id` went from 35 to 50 ms and `open_read_only` from 45 to 52 ms (median of 5).
The median replay case (three calls, with the index refresh) took the same time on cobra and
express, and 1.35 s against 1.0 s on flask.
