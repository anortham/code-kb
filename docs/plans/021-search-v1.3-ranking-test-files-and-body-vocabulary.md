# 021: Search v1.3 — ranking ties, test-file rows, and body vocabulary

Date: 2026-09-20. Status: draft, awaiting approval. Follows plan 020 (shipped
as v1.2.0). Spec: this document; the evidence is plan 020's "Results" and
"Post-review fixes" sections, plan 019's rerun tables, and the two
measurements below. Reviewed the same day by Codex as a second opinion; its
ordering (evaluation first, extractor test flags next, the cheap rank lever
before any new index) is adopted.

## Goal

Raise symbol@1 on concept and README-phrased queries with SQLite and FTS5
only, without losing the v1.2.0 numbers. No embeddings, no in-memory index,
no new daemon, no change to the MCP tool schema. Warm `code-kb search` stays
under 25 ms on the code-kb checkout (17.5 ms today) and retained memory
stays at about 25 MB.

## Where v1.2.0 stands

- Plan 019 sets: code-kb leads Julie and Miller on every column; held-out
  10 of 10 on file@1 and symbol@1. Saturated: they only detect regressions.
- Development set (89 queries, 4 repositories): 61 of 89 symbol@1, 81 of
  89 symbol@3.
- Sealed acceptance set (40 README-phrased queries, run once on the branch
  build): 19 of 40 file@1, 14 of 40 symbol@1. No repository met both
  targets. It has been run and is not reused for tuning.

## Two measurements that set the order

**1. Development-set misses are ranking misses, not recall misses.** Every
case whose symbol was not first was re-run with `--limit 200` on the final
v1.2.0 build to see whether the expected symbol was admitted at all:

| class | cases |
|---|---|
| not admitted (absent from all candidates) | 1 (`mi-concept-test-path`) |
| admitted, ranked 11 to 200 | 3 |
| admitted, ranked 2 to 10 | 24 |

So on the development set the candidate set already holds the answer 27
times out of 28. New recall branches cannot fix those; the rerank can.
Whether the acceptance misses are the same kind is unknown, because the
sealed set is not probed. Plan task 1 builds the second acceptance set with
an admission column so the next run answers that question directly.

**2. Test-file rows are not what `is_test` filters.** `julie-extract` sets
`is_test` only on callable symbols that carry a test marker (`is_test_symbol`
in `crates/julie-extractors/src/test_detection.rs`) and `test_container` on
their containers. Helpers, fixtures, constants, and imports inside test files
stay unflagged by design. On the hermes-agent index, 68,125 of the 423,537
symbols under obvious test paths (`tests/`, `*.test.ts`, `test_*.py`) are
unflagged functions, classes, or constants. Two of the three hermes-agent
acceptance misses attributed to test files rank exactly such rows first
(`apps/desktop/src/app/chat/composer/directive-scope.test.ts`,
`tests/tools/test_web_tools_config.py`). `candidate_filters` in
`crates/code-kb-core/src/queries.rs` trusts the two flags only; `code-kb`
already owns a path rule, `is_test_path` in the same file, used today only
by `find_related_tests`. This is a code-kb fix, not an extractor fix.

**Tie mechanics.** `name_hits` in `queries.rs` returns one boolean per query
word for token-run equality, substring containment (three or more
characters), or stem equality, so `csr` gives `action_csrf_token` the same
name credit as the function `csr`, and BM25 breaks the tie
(`ju-acronym-csr`, `csr adjacency`).

## Global constraints

- SQLite and FTS5 only; every new structure lives in `artifact.db` and is
  created by `ensure_fts_index` or the reconcile path, never in RAM.
- The `symbols_fts`, `symbol_names_tri`, and `skipped_files` tables and
  their triggers do not change shape.
- The MCP `search_symbols` schema keeps exactly `query`, `path`, `kind`,
  `is_test`, `limit`. Explain stays CLI-only.
- Evaluation sets live in `~/.code-kb/search-eval/` outside every checkout
  (plan 019 explains why). `acceptance.json` (v1) stays sealed and is never
  run again. Tuning uses `development.json` and the new `development-2.json`
  only.
- Regression gate: the plan 019 sets and `development.json` must not rank
  any case worse than `results` recorded for v1.2.0 (plan 020 post-review
  table), except cases a written ruling accepts.

## Design

### 1. Evaluation first: second development and acceptance sets

- `development-2.json`: 15 queries per repository (code-kb, hermes-agent,
  julie, miller), written from README and docs wording, each labelled with
  every legitimate answer symbol (alternatives are listed, not just the
  first found). Tuning for designs 2 to 4 uses this set and the v1.2.0
  development set.
- `acceptance-2.json`: 10 queries per repository, README wording, written
  before any code change, sealed by sha256 in the README. Run once at the
  end. The v1 acceptance result stays recorded as is in plan 020.
- `runner.py` gains two columns per case, computed from one extra run with
  `--limit 200 --json`: `admitted` (rank of the expected symbol among all
  candidates, or null) and `branches` (the `explain.branches` list of the
  expected row when present). The summary prints "not admitted" counts per
  repository. This is the diagnostic that separates recall from ranking for
  every later run.

### 2. Test-file rows excluded by default

- One SQL predicate, `test_path_predicate(alias)` in `queries.rs`, that
  mirrors `is_test_path` rule for rule over `replace(alias.path, '\', '/')`
  (`/test/`, `/tests/`, `/__tests__/`, `_test.`, `.test.`, `.spec.`, the
  `test.rs`, `tests.rs`, `tests.cs`, `test.go` endings, `test_` prefix).
  `is_test_path` gets a unit test that runs both forms over the same path
  list so they cannot drift.
- `candidate_filters` and the `name_search` fallback in
  `fts_search_symbols_explained`, and the lookup query in
  `search_symbols_scoped`, add `AND NOT <predicate>` when tests are
  excluded, beside the existing flag checks. `is_test: true` (MCP) and
  `--include-tests` (CLI) lift both.
- `find_related_tests` and `blast_radius` are untouched: they want test rows.
- Measured on the v1.2.0 development set before and after; the three
  hermes-agent acceptance misses are not re-run, the effect shows in
  `acceptance-2.json` at the end.

### 3. Match strength in the rerank tie-break

- `name_hits` returns a strength per word instead of a boolean: 3 for a
  token run equal to the word, 2 for a stem match, 1 for a substring match,
  0 for none. Tiers and coverage keep treating any non-zero strength as a
  hit, so no score changes.
- The sort in `rerank` gains one key after `score` and before BM25: the sum
  of name strengths, descending. `csr` (3) then beats `action_csrf_token`
  (1) on `csr adjacency`; `parseSha256Sidecar` on `sha256` is unaffected
  (single candidate class).
- Explain prints the strength sum in the per-row line.
- Unit tests: the `csr`/`csrf` tie, an acronym pair, and the existing
  `name_coverage_accepts_token_runs_substrings_and_stems` cases unchanged.

### 4. Body vocabulary from the identifiers table: experiment, then decide

- What exists: `identifiers` (columns `name`, `kind`, `containing_symbol_id`,
  `path`, `language`, positions) written at the `facts` level, kinds
  `type_usage` and `member_access` only. code-kb checkout: 3,747 rows;
  hermes-agent: 366,982 rows against 990,975 symbols. It is the vocabulary a
  symbol's body touches, not its text.
- Step 4a, offline, no code: for every development miss (both sets), an
  SQL query counts whether any query word (lowercased, identifier-split)
  equals an identifier name whose `containing_symbol_id` is the expected
  symbol. Record the count of recoverable misses in this plan.
- Step 4b, only if 4a recovers at least 5 misses across the two sets: a
  fourth recall branch, `body`, with `SELECT DISTINCT containing_symbol_id
  FROM identifiers WHERE lower(name) IN (:words) LIMIT 40`, joined to
  `symbols` with the same filters. Needs `CREATE INDEX IF NOT EXISTS
  identifiers_name_idx ON identifiers(name COLLATE NOCASE)` in
  `ensure_fts_index`'s transaction (the trigger rule marker bumps so the
  index is created once per existing database). Rows reached only by this
  branch score `W_BODY` = 3.0 per covered word fraction, below `W_NAME_ANY`
  (5.0), so a name hit always outranks a body-only hit. Explain lists
  `body` in `branches`.
- Measured like plan 020 task 4: migration time and index size on
  hermes-agent, warm p50, live `serve` RSS, `update` and `delete` cost.
- If 4a recovers fewer than 5 misses, 4b is not built and this section
  records the number as the reason.

## Tasks, in order

1. Evaluation: write `development-2.json` and `acceptance-2.json`, seal the
   acceptance file, add the `admitted` and `branches` columns to
   `runner.py`, record the v1.2.0 baseline on `development-2.json`. Half a
   session. No code-kb code changes.
2. Test-file predicate (design 2) with tests; measure on both development
   sets. Half a session.
3. Match strength tie-break (design 3) with tests; measure. Half a session.
4. Identifier experiment 4a; write the count into this plan. Quarter
   session. Then 4b only if the threshold is met: one session including the
   hermes-agent measurements.
5. Run the plan 019 sets and `development.json` as the regression gate, then
   `acceptance-2.json` once. Update this plan's results section. Half a
   session.
6. Release v1.3.0 with the numbers and, if 4b shipped, the migration cost.

Tasks 2 and 3 both edit `crates/code-kb-core/src/queries.rs`; run them in
series. Task 1 is independent of the code and can run first or in parallel
with nothing else touching the eval directory.

## Verification strategy

- Worker scope: `cargo test -p code-kb-core --lib -- queries::tests` plus
  the corpus gate `cargo test -p code-kb-core --test search_corpus_test`.
- Affected change: `cargo test -p code-kb-core` and `cargo test -p
  code-kb-cli --test cli_test --test mcp_test` (the schema guard).
- Branch gate: `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace --locked`, `node
  --test tests/plugin/*.test.cjs`, `scripts/release-preflight.sh`.
- Expensive: `runner.py` over both development sets and the plan 019 sets;
  hermes-agent migration and RSS measurements for 4b.
- Security scope: none declared.

## Acceptance

- `development.json` and the plan 019 sets: no case ranks worse than the
  v1.2.0 post-review results without a written ruling.
- `development-2.json`: symbol@1 improves by at least 5 cases over its
  v1.2.0 baseline, and "not admitted" is at most 2 of 60.
- `acceptance-2.json`, run once: file@1 at least 8 of 10 and symbol@1 at
  least 7 of 10 on each repository. Reported as is if missed.
- Warm `code-kb search` p50 under 25 ms on the code-kb checkout; live
  `serve` RSS unchanged within noise; if 4b ships, hermes-agent migration
  time and growth in the release notes.
- `search_symbols` schema unchanged (`mcp_test.rs` guard green).

## Risks

- The test-path rule hides production code in a directory called `tests`
  or a file named `test_*.py` that is not a test. Mitigation: the same rule
  already drives `find_related_tests`; `is_test: true` shows everything;
  measured on four repositories before it ships.
- The strength tie-break reorders more than the tie cases. Mitigation: it
  sits after `score`, so only equal-score rows move; the development sets
  measure it.
- The identifiers branch adds noise from common members (`len`, `get`).
  Mitigation: exact-name match on identifier-split query words only, a hard
  40-row cap, and a weight below any name hit; built only after 4a shows a
  measurable ceiling.
- Author bias in the new sets. Mitigation: README wording, every legitimate
  answer labelled, sealed before code, run once.

## Not in scope

Semantic embeddings, Tantivy, synonym or abbreviation lists, container
expansion, changes to julie-extract's `is_test` semantics, and any
`workspace` parameter on any tool.
