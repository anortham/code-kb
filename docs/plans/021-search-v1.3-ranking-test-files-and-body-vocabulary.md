# 021: Search v1.3 — ranking ties, test-file rows, and body vocabulary

Date: 2026-09-20. Status: approved 2026-09-20, in progress. Follows plan 020 (shipped
as v1.2.0). Spec: this document; the evidence is plan 020's "Results" and
"Post-review fixes" sections, plan 019's rerun tables, and the two
measurements below. Reviewed the same day by Codex as a second opinion; its
ordering (evaluation first, the test-file path rule next, the cheap rank lever
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
- Second development set (60 queries, 15 per repository), v1.2.0 baseline, as
  file@1 / symbol@1 / symbol@3 / not admitted: code-kb 9 / 8 / 8 / 2,
  hermes-agent 9 / 7 / 11 / 0, julie 7 / 7 / 9 / 1, miller 4 / 3 / 7 / 2.
  Total 29 / 25 / 35 / 5 of 60.
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
by `compute_blast_radius_scoped` (likely tests). This is a code-kb fix, not
an extractor fix.

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
  mirrors `is_test_path` rule for rule. Both turn back slashes into forward
  slashes and then split the path into the directory part and the file name,
  so a directory name can never satisfy a file-name rule. Directory rules
  run over the lowercased directory part framed by slashes, so they match at
  the repository root too (the rule as it stood needed a slash before
  `tests/` and so missed 384,043 of the 384,268 hermes-agent rows under a
  root `tests/` directory): `/test/`, `/tests/`, `/__tests__/`. File-name
  rules run over the lowercased file name: a `test_` prefix on `.py` and
  `.rb` files only (the pytest and minitest convention; Rust modules such as
  `test_quality.rs` are production code); `_test.`, `.test.`, `.spec.`; the
  whole names `test.rs` and `tests.rs`. One rule keeps the raw file name: a
  `Tests.cs` ending, case-sensitive, so `FooTests.cs` is a test file and
  `Contests.cs` is not. Every rule needs the word `test` or `spec` in the
  path, so the SQL form puts a cheap substring test in front of the rules
  and most rows skip the path split; without it the split cost about seven
  times more over the 548,580 hermes-agent rows. The bare `test.rs`,
  `tests.rs`, and `test.go` endings were dropped because they hid
  `likely_tests.rs`, `latest.rs`, and
  `latest.go`. A unit test pins 44 paths to their expected boolean for the
  Rust rule and for the SQL mirror.
- `candidate_filters` and the `name_search` fallback in
  `fts_search_symbols_explained`, and the lookup query in
  `search_symbols_scoped`, add `AND NOT <predicate>` when tests are
  excluded, beside the existing flag checks. `is_test: true` (MCP) and
  `--include-tests` (CLI) lift both.
- `find_related_tests` and `blast_radius` keep their test rows; `blast_radius`
  inherits the corrected `is_test_path` for its likely-tests list.
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
  already drives the blast-radius likely-tests list; `is_test: true` shows
  everything; measured on four repositories before it ships.
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

## Results (2026-09-21, branch `feat/search-v1.3`)

Tasks 1 to 5 ran in order with one opus implementer per code task and the
lead reviewing inline. Numbers are from `~/.code-kb/search-eval` result
files; the v1.2.0 reference for `development.json` and `regression.json` is
the rerun on the released binary (`results-v1.2.0-*-rerun.json`), which
equals plan 020's final post-review table.

### Task 1: sets and the admission column

- `development-2.json` (60) and `acceptance-2.json` (40, sealed, run once)
  exist with every legitimate answer labelled. `runner.py` reports
  `admitted` and `branches` per case and a `notAdm` column.
- v1.2.0 on `development-2.json`: 29 / 41 file@1 / file@3, 25 / 35 symbol@1
  / symbol@3, 5 of 60 not admitted.

### Design 2: test-file rows hidden (commits a0ad988, 3094e68, 6670c85, and the basename fix)

- The first rule shipped rule-for-rule against `is_test_path` and matched
  225 of the 384,268 hermes-agent rows under a root `tests/` directory. An
  external review then found that the file-name rules still read the whole
  path, so a directory name could hide a production file:
  `src/protocol.spec.v1/parser.rs`, `pkg/test_support/runtime.py`, and
  `src/Contests.cs` were all treated as test files. The fix splits the path
  before the rules run and reads the raw file name for the `Tests.cs` ending.
  The design text above records the corrected rule.
- Effect against v1.2.0 (task 2 final): development 69 / 84 / 60 / 80,
  not admitted 1; development-2 31 / 41 / 27 / 34, not admitted 5;
  regression 25 / 26 / 22 / 25. No test-file row outranks a labelled answer
  in any set now.
- The basename fix changed no score: development 70 / 84 / 61 / 80, not
  admitted 1; development-2 31 / 41 / 27 / 34, not admitted 5; regression
  25 / 26 / 22 / 25. No case ranked worse than the design 3 baseline in any
  set. The three corrected paths have no labelled case, so the gain is in
  the rule, not in the numbers.

### Design 3: whole-token tie-break (commit aa8f3d7)

- No score changed. Development 70 / 84 / 61 / 80 (`ju-acronym-csr` back
  to 1, `ju-short-glob` symbol 8 to 6). Development-2 and regression
  unchanged. No case worse than after design 2.

### Design 4a: identifier vocabulary (offline, no code)

- 63 misses examined (both development sets), 6 of them not admitted.
- Query word equals an identifier name inside the expected symbol: 6 of 63,
  none of them a not-admitted case, and every matched word is common
  (`root` in 234 containing symbols, `path` 104, `web` 68, `route` 36,
  `pivot` 11, `file` 2). Design 4b weights only rows no other branch
  reached, so it would move 0 of the 63 misses. The literal count meets the
  threshold of 5 and its purpose does not: **4b is not built.**
- Extra measurement for the "index file content" question: the expected
  symbol's body text holds at least one query word for 60 of 63 misses (4 of
  the 6 not admitted, through words such as `used`) and every query word for
  21 of 63. Full text would admit almost everything; the identifiers table
  (`member_access` and `type_usage` only) holds too little of the vocabulary
  to help. A body-token branch, if ever built, needs call and variable
  identifiers from the extractor first.

### Regression gate (final tree aa8f3d7 against v1.2.0)

| set | n | v1.2.0 file@1 / @3 / sym@1 / @3 / notAdm | v1.3 |
|---|---|---|---|
| regression (plan 019) | 26 | 25 / 26 / 22 / 26 / 0 | 25 / 26 / 22 / 25 / 0 |
| development | 89 | 69 / 83 / 61 / 81 / 1 | 70 / 84 / 61 / 80 / 1 |
| development-2 | 60 | 29 / 41 / 25 / 35 / 5 | 31 / 41 / 27 / 34 / 5 |

Eleven cases rank one to three places lower; every one stays admitted.
Ruling for all eleven: hiding test-file rows frees slots under the recall
caps and removes rows from the sample that sets the word rarity weights, so
a different production row wins a close partial-name contest (for example
`_strip_code_fences` above `strip_ansi` once `codes` became rarer than
`ansi` in the sample). None is a lost answer and none is a defect in the
new rules. The cases: `ha-concept-docker` 1 to 2, `ha-concept-strip-ansi`
1 to 2, `mi-concept-sensitive-root` 2 to 3, `mi-concept-pack-budget` 3 to 4,
`concept-julie-find` 3 to 4, `kb-d2-persist-retry` 7 to miss (admitted 7),
`ha-d2-dialectic` 4 to 5, `ju-d2-impact-budget` 2 to 3, `mi-d2-content-import`
file 3 to 4, `mi-d2-route-bridge` 3 to 4, `mi-d2-family-store` file 6 to miss
(admitted 41).

### Acceptance (sealed `acceptance-2.json`, run once on aa8f3d7)

| repo | file@1 | file@3 | sym@1 | sym@3 | not admitted |
|---|---|---|---|---|---|
| code-kb | 6 | 8 | 5 | 7 | 0 |
| hermes-agent | 4 | 7 | 4 | 6 | 2 |
| julie | 7 | 7 | 5 | 7 | 2 |
| miller | 3 | 6 | 2 | 4 | 4 |
| all (40) | 20 | 28 | 16 | 24 | 8 |

The v1 acceptance set (plan 020) was 19 / 40 file@1 and 14 / 40 symbol@1.

### Acceptance criteria

- Regression gate: met with the eleven rulings above.
- `development-2.json` symbol@1 +5: **not met** (+2, 25 to 27). Not
  admitted at most 2 of 60: **not met** (5; only a new recall branch could
  move those, and 4a showed the identifiers table is not it).
- `acceptance-2.json` 8 of 10 file@1 and 7 of 10 symbol@1 per repository:
  **not met** on every repository. The misses are README wording that never
  appears in a name, signature, or docstring; 8 of 40 are not admitted at
  all.
- Warm `code-kb search` p50 on the code-kb checkout: 19.6 ms (v1.2.0 binary
  on the same machine and run: 17.9 ms; the difference is the eleven-clause
  path predicate). Under 25 ms: met.
- Live `serve` RSS after 20 searches: 29.8 MiB against 29.5 MiB for v1.2.0.
  Unchanged within noise: met.
- `search_symbols` schema unchanged; `mcp_test.rs` green: met.

What ships is correct and small: test-file helpers no longer outrank
production code, ties prefer whole-token names, and the blast-radius test
list uses a rule that matches a root `tests/` directory. What the plan
hoped for, a symbol@1 gain on README-phrased queries, did not happen; the
lexical levers left are exhausted on these sets.
