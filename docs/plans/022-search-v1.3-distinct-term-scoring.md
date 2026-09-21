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
