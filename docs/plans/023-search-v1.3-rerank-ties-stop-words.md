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
2. Label review of the 28 development-3 misses (eval directory only).
3. Lead: apply labels, run v1.2.0 and the new build on regression,
   development, development-2, development-3; record; branch gate.
4. `acceptance-3.json` once, summary only; owner decides on the release.

## Verification

- Worker: `cargo test -p code-kb-core --lib -- queries::tests` and
  `cargo test -p code-kb-core --test search_corpus_test`.
- Lead: branch gate (fmt, clippy with warnings denied, workspace tests,
  plugin tests, AGENTS.md byte check), `runner.py` on the four tuning sets
  for both binaries, p50 and RSS, and the one acceptance-3 run.
- Security scope: none declared.
