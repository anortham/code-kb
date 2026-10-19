# Edit replay baseline: code-kb 2.7.0

The phase 3 replay harness (`scripts/edit_replay.py`, `evals/replay/README.md`) scored the 2.7.0
release binary (with julie-extract 3.8.0) on 2026-10-19. This file holds aggregate counts only.
Session logs stay outside the repository.

## Marks

| Set | Half | Mark | Result | Numbers |
|---|---|---|---|---|
| Clean commits | dev | Stop hook false blocks <= 2% of shape commits | PASS | 1 of 56 (1.8%) |
| Clean commits | holdout | same | FAIL | 1 of 49 (2.04%) |
| Holdback | dev | catch share >= 80% in each language with 10+ truth sites | FAIL | 2 of 7 languages pass (Go, Rust) |
| Holdback | dev | judged outside rows true >= 90% | FAIL | 28 of 50 (56%) |
| Holdback | holdout | catch share >= 80% in each language with 10+ truth sites | FAIL | 2 of 7 languages pass (C#, Rust) |
| Holdback | holdout | judged outside rows true >= 90% | FAIL | 41 of 50 (82%) |
| Sessions | dev | verified: >= 8 of 9 rebuilt cases caught | FAIL | 1 of 3 rebuilt |
| Sessions | dev | candidates: >= 70% of rebuilt cases caught | FAIL | 2 of 37 (5.4%) |
| Sessions | holdout | verified | FAIL | 0 of 1 rebuilt |
| Sessions | holdout | candidates | FAIL | 5 of 47 (10.6%) |

Over both halves, the Stop hook blocked 2 of 105 clean commits falsely (1.9%).

## Clean commits

105 commits, 15 from each corpus project, 56 dev and 49 holdout. Every commit changes the shape of
a symbol, and the author's commit is the truth. No case was an error.

| Half | Hook blocks | False | Listed blocks | False | Check time p50 / p95 |
|---|---|---|---|---|---|
| dev | 1 | 1 | 13 | 13 | 2.59 s / 10.47 s |
| holdout | 2 | 1 | 12 | 11 | 2.25 s / 7.44 s |

A listed block is any `proven` or `likely` row outside the diff, which was the Stop rule before 2.7.0.
The 2.7.0 rule skips most of the false listed blocks:

| Why the 2.7.0 Stop rule skips a false listed block | dev | holdout |
|---|---|---|
| `signature changed` rows only | 8 | 3 |
| `likely` and `signature changed` rows | 2 | 2 |
| `likely` rows only | 1 | 4 |
| a JavaScript parameter change | 1 | 1 |
| not skipped: a `proven` row (a false hook block) | 1 | 1 |

The dev false hook block: a TypeScript class method was removed, and a module function with the same
name stays in the same file. The check proved 95 calls of the function as sites of the removed method.

## Holdback

219 commits change a definition in one file and its uses in other files (92 dev, 127 holdout). The
harness replays only the defining file and asks which changed uses the check lists.

| Language | dev truth | dev caught | dev share | holdout truth | holdout caught | holdout share |
|---|---|---|---|---|---|---|
| C# | 42 | 11 | 26.2% | 76 | 63 | 82.9% |
| Go | 35 | 34 | 97.1% | 40 | 30 | 75.0% |
| Java | 99 | 73 | 73.7% | 151 | 67 | 44.4% |
| JavaScript | 23 | 18 | 78.3% | 105 | 52 | 49.5% |
| Python | 50 | 9 | 18.0% | 69 | 54 | 78.3% |
| Rust | 18 | 15 | 83.3% | 33 | 31 | 93.9% |
| TypeScript | 139 | 14 | 10.1% | 621 | 289 | 46.5% |

A few commits hold many truth sites, so the shares move a lot between the halves (one dev
TypeScript case holds 94 of the 139 sites).

Dev misses by cause, most first:

| Cause | dev misses | Main source |
|---|---|---|
| shape not seen | 134 | TypeScript 118: a removed constant or type alias, which the check does not compare |
| fits the new shape | 24 | the check drops a call that the new shape accepts; the author changed it anyway |
| site labeled unresolved | 19 | C# 12, Python 7 |
| site not indexed | 18 | Python 14: pytest fixture parameters |
| site excluded | 15 | Python 12: a fixture parameter excluded as another definition |
| import not listed | 13 | Java 9, Rust 3: import lines that do not name the symbol's file |
| other | 6 | |
| not a call | 3 | a function passed as a value |

Judged outside rows (rows of the changed symbol that are not truth) that are false, on dev: C#
`Equals` calls that bind to another overload by argument type but are `proven`; Java `assertEquals`
calls inherited from an unindexed base class, listed `likely`; JavaScript property reads such as
`res.locals`, listed `likely` for a function with that name.

## Sessions

231 missed-site events from the owner's Claude Code and Codex logs (122 dev, 109 holdout). The
harness rebuilds the tree just before the failing check from the session's own edits.

| Half | Cases | Rebuilt | Caught | Not rebuilt: shell edit | git state | no start commit | no site outside the edits |
|---|---|---|---|---|---|---|---|
| dev | 122 | 40 | 3 | 37 | 20 | 13 | 12 |
| holdout | 109 | 48 | 5 | 21 | 11 | 20 | 9 |

Dev misses of rebuilt cases: shape not seen 15, fixed file not listed 13, other 5, site not indexed
2, cut by a limit 2. The candidate groups come from the 2026-10-17 research without a hand check, so
many of their failures are not shape changes at all. Only 4 of the 24 verified cases rebuild.

## Notes

- The harness itself had errors in its first runs (truth from mirror copies, block comments,
  parameter renames, multi-line imports; sessions rooted in the wrong checkout). The fixes are in
  the harness commits. During those fixes the holdback holdout tables were printed as counts; no
  rule used them, and no holdout row was read.
- The clean-set result is a question for the owner before slice 2: the holdout half is 0.04 points
  over the mark, from one false block.
