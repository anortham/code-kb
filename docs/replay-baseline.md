# Edit replay baseline: code-kb 2.7.0

The phase 3 replay harness (`scripts/edit_replay.py`, `evals/replay/README.md`) scored the 2.7.0
release binary (with julie-extract 3.8.0) on 2026-10-19. This file holds aggregate counts only.
Session logs stay outside the repository.

The phase 3 branch build (commit 7950b58, julie-extract 3.8.1) was scored on 2026-10-20, and the
section "Phase 3 build" at the end puts its numbers next to the baseline. The Java holdback truth
was corrected on 2026-10-19 (a bare `assertEquals` that a test inherits from JUnit's `TestCase` is
not a use of a project method): 99 to 89 dev sites and 151 to 129 holdout sites. The baseline tables
below keep the numbers of the first scoring; the phase 3 section rescores the baseline with the
corrected truth.

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

## Phase 3 build

The same harness and cases, with the phase 3 build (commit 7950b58). The baseline columns are the
2.7.0 runs rescored with the corrected Java truth and with every outside-row verdict merged so far.
Fixes were tuned on dev only; the holdout halves were scored once, with this build.

| Set | Half | Mark | 2.7.0 | Phase 3 build |
|---|---|---|---|---|
| Clean commits | dev | Stop hook false blocks <= 2% | PASS, 1 of 56 | PASS, 0 of 56 |
| Clean commits | holdout | same | FAIL, 1 of 49 (2.04%) | FAIL, 1 of 49 (2.04%) |
| Holdback | dev | catch share >= 80% in each language with 10+ truth sites | 3 of 7 pass | 6 of 7 pass (not C#) |
| Holdback | dev | judged outside rows true >= 90% | 43 of 76 (57%) | 53 of 77 (69%) |
| Holdback | holdout | catch share >= 80% in each language with 10+ truth sites | 2 of 7 pass | 4 of 7 pass (C#, Go, Rust, TypeScript) |
| Holdback | holdout | judged outside rows true >= 90% | 81 of 90 (90%) | 90 of 96 (94%) |
| Sessions | dev | verified / candidates | 1 of 3, 2 of 37 | 1 of 3, 2 of 37 |
| Sessions | holdout | verified / candidates | 0 of 1, 5 of 47 | 0 of 1, 5 of 47 |

Holdback catch share by language:

| Language | dev truth | dev 2.7.0 | dev phase 3 | holdout truth | holdout 2.7.0 | holdout phase 3 |
|---|---|---|---|---|---|---|
| C# | 42 | 26.2% | 54.8% | 76 | 82.9% | 82.9% |
| Go | 35 | 97.1% | 97.1% | 40 | 75.0% | 82.5% |
| Java | 89 | 82.0% | 82.0% | 129 | 51.9% | 58.9% |
| JavaScript | 23 | 78.3% | 95.7% | 105 | 49.5% | 33.3% |
| Python | 50 | 18.0% | 84.0% | 69 | 78.3% | 72.5% |
| Rust | 18 | 83.3% | 83.3% | 33 | 93.9% | 93.9% |
| TypeScript | 139 | 10.1% | 94.2% | 621 | 46.5% | 80.4% |
| All | 396 | 43.9% | 85.9% | 1073 | 54.6% | 73.3% |

- JavaScript and Python fall on the holdout half: 17 and 4 truth sites that 2.7.0 listed are not
  listed. The holdout misses labeled `site excluded` grow from 43 to 60. No dev case lost a listed
  site, so the dev half does not show the cause. Finding it means reading holdout cases, which the
  plan leaves to the owner.
- Holdout misses by cause, 2.7.0 to phase 3: shape not seen 266 to 67, site not indexed 113 to 113,
  import not listed 51 to 9, site excluded 43 to 60, site labeled unresolved 10 to 10, cut by a
  limit 5 to 11, fits the new shape 8 to 2, other 13 to 14.
- Judged outside rows that are false on dev: C# `Equals` calls that bind another overload by
  argument type, C# uses of a library type that `#if` swaps in for a project type, and JavaScript
  reads of a property that is assigned a value (`res.locals`). On the holdout half, 6 of 96 judged
  rows are false (2 C#, 4 JavaScript).
- Check time on the clean set: dev p50 3.10 s and p95 11.54 s, holdout p50 3.08 s and p95 9.47 s.
  Two runs of the 2.7.0 binary on dev gave p50 2.59 s and 2.95 s, so the spread between runs is as
  large as the change.

## Holdout drop fix

The owner chose on 2026-10-20 to spend the holdout half to find the JavaScript and Python drop.
From here on both halves are tuning data; a later phase needs new holdout cases.

Two commits caused all of it:

- zod `6e74f877` deletes compiled `.js` files that sit beside their `.ts` sources. `src/types.js`
  imports `./PseudoPromise`, and the resolver tried `.ts` before `.js`, so the uses were excluded.
  2.7.0 listed 17 of them only through the method `PseudoPromise.all`; phase 3 made the removed
  variable `PseudoPromise` a shape change, and members of a removed parent fold into it.
- flask `662c2457` deletes `_compat.py`. The removed-binding rule raised its candidates from 3 to
  21, three of them second copies of one name (`text_type` in both branches of `if PY2:`), and the
  cap of 10 symbols cut `implements_to_string` and `with_metaclass`.

The fix: a JavaScript importer tries the JavaScript extensions first, and the old file of a
removed symbol takes its own place in that order; a binding defined twice in one scope reports
once; the cap is 30 symbols.

| Language | dev phase 3 | dev fix | holdout phase 3 | holdout fix |
|---|---|---|---|---|
| C# | 54.8% | 54.8% | 82.9% | 84.2% |
| Go | 97.1% | 97.1% | 82.5% | 82.5% |
| Java | 82.0% | 82.0% | 58.9% | 58.9% |
| JavaScript | 95.7% | 95.7% | 33.3% | 78.1% |
| Python | 84.0% | 84.0% | 72.5% | 78.3% |
| Rust | 83.3% | 83.3% | 93.9% | 93.9% |
| TypeScript | 94.2% | 95.7% | 80.4% | 80.7% |
| All | 85.9% | 86.4% | 73.3% | 78.4% |

- Clean set: dev 0 of 56 and holdout 1 of 49 false blocks, as before; no block changed.
- Judged outside rows: dev 53 of 77 and holdout 90 of 96 true, as before.
- Check time: holdback p95 7.05 s to 7.83 s; clean holdout p95 9.47 s to 7.94 s. The slowest
  clean case (Newtonsoft.Json `bf2e2a78`, 80 changed symbols) took 19.32 s with the cap of 10 and
  18.95 s with 30, so its time is the scan, not the symbols.

## julie 3.9.0 build

The release build (commit 74c1c6f, with julie-extract 3.9.0 at the `references` level, julie
commit 683a26d5) against the holdout-fix build (adec8ee, which ran with julie-extract 3.8.0 at the `full` level), on 2026-10-20.

| Set | Half | Holdout fix | julie 3.9.0 |
|---|---|---|---|
| Clean commits | dev | 0 of 56 | 0 of 56 |
| Clean commits | holdout | 1 of 49 (2.04%) | 1 of 49 (2.04%) |
| Holdback: truth sites listed | dev | 342 of 396 (86.4%) | 342 of 396 (86.4%) |
| Holdback: truth sites listed | holdout | 841 of 1073 (78.4%) | 840 of 1073 (78.3%) |
| Holdback: judged outside rows true | dev | 53 of 77 | 53 of 59 |
| Holdback: judged outside rows true | holdout | 90 of 96 | 90 of 96 |
| Sessions: rebuilt cases caught | dev + holdout | 8 of 88 | 8 of 88 |

- The build adds no outside row and drops 60: 57 JavaScript dev rows, where julie now writes a
  property for a value assigned on a named object (`res.locals = …`), and 3 C# rows, where the `#if`
  rule makes a use of `ConcurrentDictionary` `unresolved`. The dropped judged rows were all false
  (16 JavaScript, 2 C#); no true row was lost.
- The one lost holdout site is express `f41d09a3cf05`: the commit replaces the method
  `Router.prototype.middleware` with `self.middleware = self.handle.bind(self)` in the constructor.
  julie 3.9.0 writes that property, so the test's `router.middleware` still names a definition and
  is `unresolved` for the removed method. The name is still defined, so the label is right; the
  author changed the line because the test was rewritten.
- Holdback check time: p50 1.12 s to 1.02 s, p95 7.80 s to 5.63 s. Clean set: dev p50 2.16 s and
  p95 10.54 s, holdout p50 2.03 s and p95 6.98 s.
- The corpus check then found that a new property row owned the calls in its value and that a
  chained assignment (`req.get = req.header = function header()`) made a property the parent of
  the method. julie 683a26d5 fixes both; the replay counts above are equal before and after it.
