# 022: Search v1.3, part 2 — distinct-term scoring

Date: 2026-09-21. Status: approved by the owner ("the goal is to make search
better"), in progress on `feat/search-v1.3`. Follows plan 021, whose Results
section records that v1.3 as built does not beat v1.2.0 on symbol@1. Spec:
this document plus the Codex second opinion of 2026-09-21 (kept beside the
evaluation sets, outside every checkout, because it names expected
answers).

## Gate

v1.3.0 does not ship until it beats v1.2.0 on every set: regression (plan
019), `development.json`, and `development-2.json`, on file@1 and symbol@1,
with no regression-set case lower than v1.2.0 without a written ruling.

## Diagnosis (from the 61 development misses on the plan 021 build)

- The answer is usually already a candidate and already holds the query
  words. It loses on score, not on recall.
- `rerank` adds three field coverages (name tier up to 30 for a partial
  match, signature 4, doc 18). A row that repeats two query words across
  three fields beats a row that covers three different words once.
- `word_weights` derives rarity from the candidate sample, so prose words
  that code rarely contains (`into`, `between`, `two`) get the highest
  weights, above the content words.
- Names match by stem equality; signatures and docs match by stem prefix.
  The same word can count in one field and not the other.
- The `admitted` column of the runner measures a different search: the
  recall caps scale with `--limit` (`word_cap = (limit * 4).clamp(40, 160)`,
  `name_cap = (limit * 2).clamp(20, 40)`), and rarity is recomputed. It
  stays as a rough diagnostic; it is not a recall measurement.
- Roughly 15 of the 61 labels have a plausible competing answer (an async
  twin, the documented entry point, the public API in front of a helper).
  Those are reviewed first so the scorer is not tuned against wrong labels.

## Design

### 1. Label review (task 7, eval directory only)

Every one of the 61 misses gets a ruling with evidence: keep, add an
alternative, replace, or drop. Proposed sets are written beside the live
ones; the lead applies them, and both the v1.2.0 binary and the current
build are re-run so every later comparison uses the same labels.

### 2. Distinct-term scoring behind a switch (task 8, `queries.rs`)

- A second scorer in `rerank`, selected by `CODE_KB_RERANK=distinct` (an
  experiment switch, removed when one scorer wins; never an MCP parameter).
- Unit of credit: one query term, credited once, from its strongest
  evidence: name whole-token 3, name stem 2, signature or doc whole token
  or stem 2, name substring 1. Score = sum over terms of credit × term
  weight, plus the existing kind, path, documentation, and test-intent
  priors, plus the whole-name and all-words bonuses that keep exact
  identifier queries where they are.
- Term weights: variant A uniform; variant B the existing rarity weights.
  Both measured.
- The name tier bonuses (`W_NAME_WHOLE`, `W_NAME_ALL_WORDS`) stay, so the
  plan 019 identifier queries do not move.
- Explain reports which field credited each term.

### 3. Decision and promotion

- Measure both variants on `development.json` (89) and `development-2.json`
  (60) with the reviewed labels, and on the regression set, against the
  v1.2.0 binary and the current default scorer, all on identical candidates.
- The variant with the best net symbol@1 across the 149 development cases
  and no regression-set loss becomes the default; the switch is removed;
  the corpus gate (`search_corpus_test.rs`) gains cases for the fixed
  classes; docs and release notes are updated.
- If neither variant wins net, the plan records the per-case table and the
  next lever (Codex's second choice: file and parent context for
  unqualified method names such as `Bm25::Idf`) is built and measured the
  same way. The work continues until the gate is met.

### 4. Final check

`acceptance-2.json` is run once more at the end. No per-case result of it
is read; only the per-repository summary is recorded. The plan 021 run
remains the recorded first run.

## Tasks

7. Label review (eval directory only; no runner, no live-set edits).
8. Distinct-term scorer behind the switch, with unit tests and both
   variants measured. Same file as the plan 021 test-path fix, so it starts
   after that fix's commit.
9. Lead applies labels, reruns baselines, decides, and dispatches the
   promotion (default scorer, switch removed, corpus tests, docs, release
   notes updated with the new numbers).
10. Final gate run and acceptance-2 summary; then the owner decides on the
    release.

## Verification

- Worker: `cargo test -p code-kb-core --lib -- queries::tests`, then
  `cargo test -p code-kb-core --test search_corpus_test`.
- Lead: full branch gate as in plan 021, plus `runner.py` over all three
  sets for v1.2.0 and each scorer variant.
- Security scope: none declared.

## Results (2026-09-21)

### Task 7: label review

61 misses ruled with evidence: keep 41, add 20, replace 0, drop 0. Every
addition is a public entry point, an async twin, a sibling of the same
operation, or a set-internal consistency fix; nothing was removed. Three of
the six "never admitted" cases were label gaps. One regression-set case
(`concept-discover`) gained a second legitimate answer the same day. The
sets before the review are kept beside the live ones. Every number below
uses the reviewed labels, for v1.2.0 as well.

### Tasks 8 and 9: the scorer behind a switch, then the grid

Text credit is the credit a signature or doc hit earns per term (name whole
token 3, name stem 2, name substring 1). Numbers are file@1 / file@3 /
symbol@1 / symbol@3.

| scorer | regression (26) | development (89) | development-2 (60) |
|---|---|---|---|
| v1.2.0 | 25 / 26 / 22 / 26 | 72 / 87 / 68 / 85 | 29 / 43 / 27 / 39 |
| plan 021 default (test-path rule, tie-break) | 25 / 26 / 22 / 25 | 73 / 88 / 68 / 85 | 31 / 43 / 30 / 38 |
| distinct, uniform, credit 2 | 26 / 26 / 18 / 23 | 76 / 85 / 68 / 82 | 35 / 49 / 30 / 41 |
| distinct, sample rarity, credit 2 | 24 / 26 / 20 / 24 | 74 / 85 / 68 / 82 | 28 / 45 / 23 / 37 |
| distinct, IDF, credit 2 | 25 / 26 / 20 / 26 | 76 / 86 / 68 / 83 | 35 / 48 / 29 / 43 |
| distinct, IDF, credit 1.5 | 26 / 26 / 20 / 26 | 77 / 86 / 68 / 84 | 38 / 51 / 32 / 47 |
| **distinct, IDF, credit 1** | **26 / 26 / 22 / 26** | **78 / 85 / 70 / 83** | **40 / 53 / 34 / 48** |
| distinct, IDF, credit 0.75 | 26 / 26 / 22 / 26 | 77 / 85 / 70 / 82 | 38 / 54 / 33 / 49 |
| distinct, IDF, credit 0.5 | 26 / 26 / 22 / 26 | 77 / 85 / 69 / 81 | 38 / 53 / 32 / 48 |
| distinct, uniform, credit 1 | 26 / 26 / 21 / 24 | 74 / 85 / 69 / 81 | 39 / 50 / 35 / 46 |

- A nameless cap of 22 changed nothing at credit 1.5 or 1 and cost one
  development symbol@1 at credit 2, so it is not built.
- IDF weights come from an `fts5vocab` table over `symbols_fts`, created on
  the write path at the next open (no rebuild); an index without it falls
  back to a `MATCH` count per term. The lookups cost about 100 µs per query.
- Credit 1 is the peak on symbol@1; the tension it leaves is a README-only
  answer (`OutputStallTimeout`, whose docstring holds the query) losing to a
  name that holds two common words. Lower credit widens that loss; higher
  credit lets doc-heavy constants outrank the answer.

### Gate against v1.2.0 (reviewed labels)

- Regression: file@1 26 against 25, symbol@1 22 against 22, symbol@3 26
  against 26. No case lower after the `concept-discover` ruling.
- Development: file@1 78 against 72, symbol@1 70 against 68; symbol@3 83
  against 85 and file@3 85 against 87 are the two top-3 columns that moved
  down, both by two cases.
- Development-2: file@1 40 against 29, symbol@1 34 against 27, symbol@3 48
  against 39.

### Task 10: promotion (commit bb2e4fd)

- The promoted scorer equals the winning grid row on every set. The one
  moved rank is `concept-discover` (symbol rank 3 to 1), which the
  regression-set label ruling explains.
- Warm `code-kb search` p50 on the code-kb checkout: 18.8 ms (v1.2.0
  17.9 ms). Live `serve` RSS after 20 searches: 28.2 MiB (v1.2.0 29.7 MiB).
- Branch gate on bb2e4fd: fmt, clippy with warnings denied, the workspace
  tests, the 16 plugin tests, and the AGENTS.md / CLAUDE.md byte check all
  pass.

### Sealed acceptance-2 (per-repository summaries only)

Three summary runs exist; no per-case result was read. Numbers are
file@1 / file@3 / symbol@1 / symbol@3, then the count never admitted at
`--limit 200`.

| build | code-kb | hermes-agent | julie | miller | all |
|---|---|---|---|---|---|
| v1.2.0 (`bin/code-kb-1.2.0`) | 6 / 8 / 5 / 7 | 3 / 7 / 3 / 6 | 7 / 7 / 5 / 6 | 3 / 5 / 2 / 3 | 19 / 27 / 15 / 22, 10 |
| plan 021 build (run 1) | 6 / 8 / 5 / 7 | 4 / 7 / 4 / 6 | 7 / 7 / 5 / 7 | 3 / 6 / 2 / 4 | 20 / 28 / 16 / 24, 8 |
| promoted bb2e4fd (run 2) | 5 / 8 / 4 / 6 | 3 / 7 / 1 / 6 | 7 / 8 / 6 / 8 | 3 / 4 / 1 / 2 | 18 / 27 / 12 / 22, 8 |

The held-out README set does not show the gain the tuning sets show. The
promoted build is one file@1 and three symbol@1 below v1.2.0 on it, and a
quarter of its cases are never admitted on either build. The owner's gate
("better than v1.2.0") is therefore not met on held-out README wording, and
v1.3.0 does not ship on this build. The plan 022 gate (regression,
development, development-2) is met; the next plan records why the two
disagree and what is built next.
