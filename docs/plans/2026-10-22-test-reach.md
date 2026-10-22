# Test reach: a planted-bug check and an honest test list

Status: approved by the owner on 2026-10-22 ("follow your suggested items"). Branch `feat/test-reach`.

## Why

On 29 planted Flask bugs from Miller's 2026-10-15 replay, `blast_radius` 2.9.0 seeded with the
changed function missed 82% of the failing test files. When it listed tests (10 uncut answers), 8
answers missed a failing test and gave no warning: `RequestContext.push` lists 3 tests, 231 fail.
The walk stops at `wsgi_app`, `__call__`, and `__enter__`, which the test client and `with` call.
Julie and Miller both lost agent trust through answers that looked complete. Evidence:
`~/.code-kb/reports/2026-10-22-julie-miller-review/`.

## Item 4 (done, `e1e0018`)

`blast_radius` lists a changed test, the users of a changed fixture, the tests of a test-file
target, and a test in a changed test file that calls changed code. Git discovery reads paths
relative to the root.

## Item 1: the planted-bug replay (`scripts/bug_replay.py`)

- `truth --repo NAME`: walks back from the corpus commit, plants one bug on a source line each
  commit added (Miller's operator flips), and runs the full suite with and without it. The tests
  that fail only with the bug are the truth. Runners: pytest through `uv run` (Flask), `go test -json`
  (cobra), mocha (Express). Output: `evals/bug-replay/<repo>.jsonl`.
- `run --code-kb PATH --out DIR`: for each bug, in a detached worktree of the corpus clone, records
  `blast-radius --json` for two seeds: the symbol around the bug line, and the git diff with the bug
  applied. The run records the binary fingerprint, like `edit_replay.py`.
- `score --run DIR`: per repository and seed, the failing test files and tests missed, the bugs with
  every failing file missed, empty and cut answers, the median share of the suite listed, and two
  trust counts:
  - silent miss: the answer lists tests, is not cut, misses a failing test, and gives no warning;
  - needless warning: the answer warns and misses no failing test.
- Split: a commit is dev when `int(sha[:8], 16) % 2 == 0`. Tune on dev, report holdout.
- Unit tests: `scripts/test_bug_replay.py` (`cd scripts && python3 -m unittest test_bug_replay`).

## Item 2: say what the test list cannot see

Design from the dev half of the data, not before it. The first candidate: the walk reached a symbol
that no indexed code calls (an "open end") and that is not a test, such as `__call__` or `__enter__`.
Tests that reach the target through it are not visible. Then the answer:

- names up to three open ends with their locations;
- says that the runtime, a framework, or a library calls them, so other tests can reach the target;
- adds a JSON field with the open ends.

Acceptance on the holdout half: silent misses go to 0 on every repository, and needless warnings
stay at or below 20% of the answers that list tests. If the open-end rule cannot meet both, record
the numbers and the rule that came closest, and report that instead of a rule that warns always.

## Verification

- `cargo test --workspace --locked`, clippy, fmt, plugin tests at the branch gate.
- `scripts/bug_replay.py score` before and after item 2, on the same truth, with the numbers in
  `docs/bug-replay-baseline.md`.
- The reach gate is unchanged: item 2 adds text and a field, no label.

## Results (2026-10-22)

Numbers: `docs/bug-replay-baseline.md`. 96 planted bugs in cobra, express, and flask.

- Item 1: `scripts/bug_replay.py` with `truth`, `recheck`, `run`, and `score`, and the truth in
  `evals/bug-replay/`. `recheck` runs each failing mocha file alone, because one mocha process
  runs every Express file and an error can fail tests in later files.
- Item 2, as built:
  - an open end is a reached function, method, or constructor that no caller step of the walk
    leaves; the answer names up to three under `### Open Ends`, and JSON adds `open_ends`;
  - the target is no open end when its callers report has a `proven` or `likely` site;
  - when the walk stops at the depth, a walk to depth 5 counts the tests it adds
    (`tests_past_depth`), and the answer gives the count only when it is above 0;
  - when the callers report supplies the callers, the walk goes on from them one level deeper,
    and the report's tests join a list that the walk started.
- Marks: no silent miss on either half (2.9.0: 28 of 44 dev, 32 of 46 holdout). Needless warnings:
  7 of 45 dev (PASS), 10 of 43 holdout (23%, FAIL). The last holdout silent miss led to the
  report-test fix, so the holdout half is no longer clean for that rule.
- Next gaps in reach, outside this plan: the default depth of 2, runtime entry points such as
  `Flask.__call__`, and JavaScript member calls that julie stores only as identifiers.
