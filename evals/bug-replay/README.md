# Planted-bug replay

`scripts/bug_replay.py` checks the tests that `blast_radius` lists against the tests that a real
bug breaks. It also checks that an answer that misses a failing test says so. It runs no model.

## Truth (`<repo>.jsonl`)

```
scripts/bug_replay.py truth --repo NAME [--commits 50] [--tries 4] [--start REV]
```

- `truth` walks back from the pinned commit of the project in `evals/reach/corpus.json`, and keeps
  the commits that change a source file.
- For each commit, it plants one bug on a source line that the commit added: one operator flip,
  as in Miller's 2026-10-15 replay (`==` to `!=`, `and` to `or`, `True` to `False`, `return x` to
  `return None`, and so on). It runs the full test suite with and without the bug. The tests that
  fail only with the bug are the truth. It tries up to `--tries` bugs per commit.
- Each row has an `outcome`: `detected` (some test failed), `undetected`, `no-mutant` (no line to
  flip), or `broken` (the suite did not run). A new `truth` run drops the `broken` rows and runs
  those commits again. Only `detected` rows are scored.
- The suites run in worktrees under `~/.cache/code-kb-accuracy/bug-replay/trees/<repo>`, made from
  the clones in `~/.cache/code-kb-accuracy/phase3/repos/<repo>`.

| Repo | Runner | Test key |
|---|---|---|
| flask | pytest through `uv run --frozen` (the tree's `.venv` for a commit with no `uv.lock`) | file and `Class::test` |
| cobra | `go test -json -count=1 ./...`; a build failure is a broken run | file and top-level `TestX` |
| express | mocha from the `test` script of `package.json`, with the JSON reporter; `npm install` runs again when `package.json` changes | file and test title |

A test with parameters (`test_x[a]`) or subtests (`TestX/sub`) counts once, and it fails when one
of its cases fails. Flask commit `954f5684` stays `broken`: its code does not import with its own
locked packages.

## Run and score

```
scripts/bug_replay.py run --code-kb PATH --out DIR [--repo NAME] [--split dev|holdout|all] [--depth N]
scripts/bug_replay.py score --run DIR [--split dev|holdout|all] [--json]
```

- `run` checks out each commit in a detached worktree under `<out>/trees/` and records
  `code-kb blast-radius --json --limit 200` for two seeds: the innermost function, method, or type
  that holds the bug line (`--symbol-id`), and the git diff with the bug applied (no target). The
  depth is the binary's default, or `--depth`. The run records the binary's SHA-256 fingerprint, as `edit_replay.py`
  does; a rebuilt binary needs a new output directory.
- `score` prints one row per repository and seed:
  - the failing test files and tests that the answer left out, and the bugs whose failing files
    it left out entirely;
  - empty answers (no test, so the text says to run the full suite) and cut answers (`limit` or
    the 200-row ceiling cut the list, and the text says so);
  - the median share of the suite that the answer lists;
  - a silent miss: the answer lists tests, is not cut, misses a failing test, and gives no warning;
  - a needless warning: the answer warns and misses no failing test.
- An answer warns when it has `open_ends`, or when it lists tests and `tests_past_depth` is above
  0. Answers from a binary without these fields never warn: its text said nothing in those cases.

## Split

A commit is dev when `int(sha[:8], 16) % 2 == 0`, else holdout. Tune rules on dev only. The
results are in `docs/bug-replay-baseline.md`.

## Unit tests

```
cd scripts && python3 -m unittest test_bug_replay
```
