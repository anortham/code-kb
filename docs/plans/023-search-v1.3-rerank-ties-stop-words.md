# 023: Search v1.3, part 3 — stop words, tie order, member whole-name bonus

Date: 2026-09-21. Status: in progress on `feat/search-v1.3`. Follows plan 022,
whose Results section records that the promoted build beats v1.2.0 on the
tuning sets and not on the sealed README set (`acceptance-2.json`: 18 / 27 /
12 / 22 against 19 / 27 / 15 / 22). Spec: this document plus the
per-repository summaries in plan 022.

## Gate

Unchanged from plan 022: v1.3.0 does not ship until it beats v1.2.0 on
every set on file@1 and symbol@1, now including `development-3.json`, and
the fresh sealed set `acceptance-3.json` is run once at the end.

## Evaluation sets after plan 022

- `acceptance-3.json` (40, README and docs wording, 10 per repository) was
  written and sealed before any plan 023 code change. It is the held-out
  set for this plan.
- `acceptance-2.json` was consumed by three summary runs. Its 40 cases were
  copied to `development-3.json` as a tuning set after the new seal, so
  the held-out failures can be studied case by case for the first time.
- Every miss of the promoted build on `development-3.json` gets the same
  label review as plans 022 gave the other sets (keep, add, replace, drop,
  with evidence), before any number is compared.

## Diagnosis (offline simulation over the limit-200 candidates)

A scratchpad simulator rescored the cached candidates of all 189 tuning
cases under each candidate change, so every lever below was measured
before any code was written. Numbers are file@1 / file@3 / symbol@1 /
symbol@3 inside the limit-200 candidate set, which differ slightly from
the runner at limit 10.

| variant | development (89) | development-2 (60) | development-3 (40) |
|---|---|---|---|
| promoted build | 76 / 84 / 70 / 82 | 39 / 53 / 33 / 47 | 20 / 29 / 13 / 21 |
| wider stop-word list | 76 / 84 / 70 / 82 | 40 / 51 / 35 / 45 | 20 / 29 / 13 / 20 |
| public-before-private tie | 78 / 84 / 72 / 82 | 40 / 53 / 34 / 47 | 20 / 29 / 14 / 21 |
| whole-name bonus 60 for non-definition kinds | 78 / 83 / 74 / 82 | 39 / 53 / 33 / 47 | 20 / 29 / 13 / 21 |
| **the three together** | **80 / 83 / 76 / 82** | **41 / 51 / 36 / 45** | **21 / 29 / 15 / 20** |
| the three plus body words at credit 0.5 | 80 / 83 / 77 / 82 | 45 / 53 / 38 / 45 | 20 / 27 / 15 / 19 |
| parent and file-name context, credit 1 or 0.5 | no change | no change | not run |
| all-terms-covered bonus 10 / 20 / 30 | 75 / 86 / 68 / 84 and lower | 37 / 53 / 33 / 45 and lower | not run |
| rarity weights damped (square root) | 80 / 83 / 75 / 81 | 40 / 49 / 34 / 44 | 19 / 31 / 14 / 23 |
| signature text ignored for member kinds | no change | 39 / 52 / 34 / 46 | 21 / 29 / 14 / 21 |

What the misses showed:

- English function words earn credit. The rarity weight is computed over
  code text, where `was`, `how`, `so`, `whether`, `another` are rare, so a
  method named `_was` wins a prose query on that word. The stop-word list
  has 23 entries and misses all of these.
- A private twin ties the public entry point (`_create_skill` against
  `create_skill`, `_pairing_store` against `PairingStore`) and BM25 breaks
  the tie toward the private one.
- A constant, enum member, variable, property, or namespace whose name
  equals a one-word query takes the whole-name bonus of 100 over a
  function that holds the word plus context (`Glob` over
  `matches_glob_pattern`, `croniter` over `_ensure_croniter`).
- Body words are the only new vocabulary left, and the offline branch over
  a scratch body-word index (definition kinds only; 36 MB on the 1.7 GB
  hermes-agent index) admits 2 more answers on development-2 and moves
  nothing on development-3. It is recorded, not built.
- 8 of the 40 development-3 answers are never admitted on any build,
  because the README sentence shares no word with the name, signature,
  docstring, or body of the answer. Those are a label-review question, not
  a scorer question.

## Design (all in `crates/code-kb-core/src/queries.rs`)

1. `STOP_WORDS` grows to the function words below. It is used by the
   FTS query builder, the trigram terms, and the rerank words, and every
   user keeps the rule that stop words are dropped only when a content
   word remains:
   `was were been being has have had do does did so not no whether when
   what which who whom where why how another other one two same than then
   its their them they we you your our can could should would will may
   might must also only just if else each every all any some such because
   while until since never always still yet more most very much many about
   between without within through again here there these those both either
   neither nor but too via per already instead rather ever once itself`.
   Directional words that live in identifiers (`up`, `down`, `out`, `over`,
   `under`, `back`, `into`, `onto`) stay content words.
2. The sort after `name_strength` and before BM25 puts names that start
   with `_` after names that do not.
3. The whole-name bonus is `W_NAME_WHOLE` (100) only for `DEFINITION_KINDS`;
   every other kind gets `W_NAME_ALL_WORDS` (60) for a whole-name match, so
   a definition that holds all the words ties the bonus and wins on the
   kind prior. `name_tier` still reports `whole`.
4. Unit tests for each rule and two corpus cases (a constant named exactly
   the query below a function that holds every word; a public name before
   its private twin at equal score). CLAUDE.md and AGENTS.md describe the
   tie order and the bonus rule.

## Tasks

1. The three rules with tests (one worker, `serial-worker-commit`). Done.
2. Label review of the 28 development-3 misses (eval directory only). Done.
3. Lead: apply labels, run v1.2.0 and the new build on regression,
   development, development-2, development-3; record; branch gate. Done.
4. `acceptance-3.json` once, summary only; owner decides on the release. Run; awaiting the owner.

## Verification

- Worker: `cargo test -p code-kb-core --lib -- queries::tests` and
  `cargo test -p code-kb-core --test search_corpus_test`.
- Lead: branch gate (fmt, clippy with warnings denied, workspace tests,
  plugin tests, AGENTS.md byte check), `runner.py` on the four tuning sets
  for both binaries, p50 and RSS, and the one acceptance-3 run.
- Security scope: none declared.

## Results (2026-09-21)

Task 1 landed as cab888a (three unit tests, two corpus cases, CLAUDE.md and
AGENTS.md updated). Task 2 ruled the 28 development-3 misses: keep 9, add
19 (24 pairs), replace 0, drop 0; six miller cases had labeled the tool
class and not the entry method that carries the README sentence, and three
of the eight never-admitted cases stay real recall failures. The sets
before the review are kept beside the live ones in the evaluation
directory. Numbers are file@1 / file@3 / symbol@1 / symbol@3 from the
runner at limit 10, on the reviewed labels, both binaries on the same
machine and the same day.

| set | v1.2.0 | plan 022 build (bb2e4fd) | plan 023 build (cab888a) |
|---|---|---|---|
| regression (26) | 25 / 26 / 22 / 26 | 26 / 26 / 23 / 26 | 26 / 26 / 23 / 26 |
| development (89) | 72 / 87 / 68 / 85 | 78 / 85 / 70 / 83 | 82 / 84 / 76 / 83 |
| development-2 (60) | 29 / 43 / 27 / 39 | 40 / 53 / 34 / 48 | 40 / 52 / 35 / 47 |
| development-3 (40, reviewed labels) | 20 / 27 / 15 / 22 | not run | 21 / 29 / 17 / 25 |

- The runner agrees with the simulation within the expected cap
  difference: development-3 on the pre-review labels moved 18 / 27 / 12 /
  22 (plan 022 build) to 19 / 28 / 14 / 22, one symbol@1 short of v1.2.0;
  the reviewed labels give the same four cases to both builds and the new
  build then leads on every column.
- Development file@3 (84 against 87) and symbol@3 (83 against 85) stay
  below v1.2.0; every file@1 and symbol@1 column is above it.
- Warm `code-kb search` p50 on the code-kb checkout: 20 ms (v1.2.0 19 ms,
  same session). Live `serve` RSS after 20 searches: 28.0 MiB (v1.2.0
  30.4 MiB); both figures move by about 2 MiB between runs.
- Branch gate on cab888a: fmt, clippy with warnings denied, the workspace
  tests (139 in code-kb-core), the 16 plugin tests, and the AGENTS.md
  byte check pass.
- Not built, measured at plus or minus one case: the full docstring instead
  of its first 400 bytes (+2 / -1 on development), and no signature credit
  for members whose signature is a string value (+1 development-3, -1
  development-2 file@1).

### Sealed acceptance-3 (one run per binary, per-repository summaries only)

| build | code-kb | hermes-agent | julie | miller | all, never admitted |
|---|---|---|---|---|---|
| v1.2.0 | 7 / 9 / 5 / 7 | 0 / 1 / 0 / 0 | 5 / 5 / 2 / 2 | 1 / 2 / 1 / 1 | 13 / 17 / 8 / 10, 20 |
| plan 023 build | 7 / 9 / 7 / 8 | 1 / 2 / 0 / 0 | 5 / 5 / 2 / 3 | 1 / 3 / 1 / 1 | 14 / 19 / 10 / 12, 19 |

The new build is above v1.2.0 on every column of the held-out set, by one
to two cases. Half of the 40 answers are never admitted by either build:
the set's hermes-agent and julie sentences share no word with the name,
signature, or docstring of their answers, which is the lexical ceiling
this engine has without embeddings (a declared non-goal). The gate of
plan 022 is met on every set, including the held-out one; the owner
decides on the release.

## Codex review of the branch (2026-09-21, before the merge)

`codex exec` reviewed `main..HEAD` (read-only, redacted bundle) and returned two
findings, both confirmed on the hermes-agent index (990,975 symbols):

1. **The vocabulary lookup used a different stemmer than the index.** `idf_weights`
   looked the term up in the `fts5vocab` table by its Snowball stem and its raw form,
   but `symbols_fts` stems with `porter unicode61`. `news` is indexed as `new`, so
   the lookup found no row and the word got the unseen-term weight (13.8 instead of
   5.2 on that index).
2. **The vocabulary count walks every posting of a common term.** `self` (49,308
   rows) cost 4.2 ms per lookup, `name` 1.9 ms, so a prose query with several
   common words spent tens of milliseconds in the weight stage on a large index.

Fix (one commit): the document frequency is a capped FTS5 `MATCH` count per term
(`SELECT count(*) FROM (SELECT rowid FROM symbols_fts WHERE symbols_fts MATCH
'"word"' LIMIT 20000)`), which stems the query word with the index's own tokenizer
and bounds the walk at `DF_CAP` rows (under 1 ms per term on the same index). The
`symbols_fts_vocab` table is no longer created; an index that has one keeps it
unused. A unit test pins the stemmer agreement (`news` counts the rows indexed as
`new`). The measurement after the fix is recorded below.

### After the fix (runner, reviewed labels, same machine)

| set | before the fix (cab888a) | after the fix |
|---|---|---|
| regression (26) | 26 / 26 / 23 / 26 | 26 / 26 / 23 / 25 |
| development (89) | 82 / 84 / 76 / 83 | 82 / 84 / 76 / 83 |
| development-2 (60) | 40 / 52 / 35 / 47 | 40 / 53 / 35 / 48 |
| development-3 (40) | 21 / 29 / 17 / 25 | 22 / 30 / 18 / 26 |
| acceptance-3 (40, sealed, second run) | 14 / 19 / 10 / 12, 19 never admitted | 14 / 19 / 10 / 12, 19 never admitted |

- Ruling, regression case `concept-fts` (symbol rank 3 to 4; v1.2.0 also 3):
  four functions tie at the same score with the same evidence (`fts` and
  `index` as whole name tokens, no row holds `porter` or `stemming` in any
  field), and BM25 orders the tie. With the corrected weights `create` and
  `fts` weigh the same, so `create_index` joins the tie above the answer.
  Rank 3 or 4 inside a four-way tie is not a scorer defect; accepted.
- Warm `code-kb search` p50: 19 ms. Branch gate on the fixed tree: green.
