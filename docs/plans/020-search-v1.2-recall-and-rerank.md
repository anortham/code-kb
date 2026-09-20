# 020: Search v1.2 — name recall and a deterministic rerank

Date: 2026-09-20. Status: implemented on branch feat/search-v1.2-recall-rerank,
evaluated 2026-09-20; release pending. Follows plan 019. Revised the same day
after a Codex review (eight findings, all folded in below). Measurements are in
"Results (2026-09-20)" at the end.

## Goal

Close the held-out gap to Miller's lexical search with SQLite and FTS5 only.
No embeddings, no Tantivy, no in-memory index, no new daemon. Query latency
stays in the current band (about 15 ms warm) and retained memory stays at
the measured 25 MB.

## Where the misses come from (plan 019, held-out set)

- Name recall: `parseSha256Sidecar` is one FTS5 token, so a word query never
  reaches it. Only two- and three-word queries are concatenated today.
- Ranking: short enum variants (`Skeleton`, `Outline`) outrank
  `render_symbol_skeleton` because BM25 rewards short rows and nothing ranks
  a function above an enum member. Script variables (`BINARY`,
  `ActualSha256`) outrank crate code for the same reason.

## Evaluation sets, frozen before any code (Codex finding 7)

- The 16 tuning and 10 held-out queries from plan 019 already shaped this
  design. They become the **regression set**: they must not get worse.
- A new **development set** is written first: about 20 queries each on
  code-kb (Rust plus the JS launcher), hermes-agent (Python), julie (Rust),
  and miller (C#). Features and weights are tuned on this set only.
- A new **acceptance set** is written at the same time and sealed: 10 queries
  per repository, same four repositories, different authors' intent where
  possible (write them from README and docs, not from the code). It is run
  once, after tuning, and reported as is.
- All three sets live outside this checkout (plan 019 explains why).

## Design

### 1. Trigram FTS table on symbol names (recall)

- New virtual table `symbol_names_tri` with FTS5 `tokenize='trigram'` over
  the `name` column only, as an external-content table on `symbols` like
  `symbols_fts`. Same insert, update, and delete triggers with the same
  local-variable exclusion, adapted to that table's single column. Plain
  SQL, so the triggers also run inside julie-extract, which writes the
  symbols table directly (`sync.rs` `update`, `delete`, `scan`).
- Verified on the bundled SQLite 3.51: `MATCH 'sha256'` finds
  `parseSha256Sidecar`; old-value delete and update triggers on a trigram
  external-content table behave (Codex reproduced this in memory).
- Do not put trigrams on `signature` or `doc_comment`. Names are short, so
  the index stays small; doc text would not.
- Alternative kept open: julie-extract emits a `name_words` column (`parse
  sha256 sidecar`) at its next schema bump. Smaller index and stemming
  applies, but it couples two releases. Revisit when julie bumps schema.

### 2. Migration that can fail safely (Codex finding 2)

Today `ensure_fts_index` runs DDL, `delete-all`, population, and the marker
write as separate statements with no transaction, its readiness check looks
only at `symbols_fts_docsize`, and both `scan_workspace` (`sync.rs`) and the
CLI discard its result with `let _ =`. A second table multiplies the partial
states that check cannot see. The plan therefore includes:

- One transaction around DDL, population of both tables, and the marker
  write. The marker (`FTS_RULE` bumped to a new value) is written last, so
  an interrupted migration re-runs on the next start.
- Readiness checks both tables' `_docsize` shadow tables plus the marker.
- Population stays code-kb's filtered `INSERT ... SELECT ... WHERE NOT
  local` for both tables. Never FTS5's `rebuild` command, which would pull
  locals back in.
- Failures surface: `scan_workspace` and the CLI log the error and return a
  retryable state instead of success. The first tool call reports it the
  way a failed scan is reported today.
- Tests: fresh index, upgrade from a `exclude-locals-v1` index, interrupted
  migration (marker absent, tables present) then retry, one table missing,
  extractor `scan`, `update`, and `delete` driving the triggers from the
  real julie-extract process, and the same on Windows through `win-test`.

### 3. Candidate selection that protects every recall source (Codex finding 1)

- Word branch: `symbols_fts MATCH <or query>` ordered by its BM25, with the
  existing import-last, documentation-last, and exact-name rules kept
  **before** its LIMIT, exactly as today. Take up to `max(limit * 4, 40)`
  rows, capped at 160.
- Name branch: `symbol_names_tri MATCH <trigram terms>` for query words of
  three or more characters, ordered by name length ascending (shorter names
  contain the query more densely), up to `max(limit * 2, 20)` rows, capped
  at 40. Reserved capacity: these rows are admitted even when the word
  branch is full.
- Exact-name branch: `name = :query COLLATE NOCASE`, always admitted.
- Deduplicate by rowid in Rust. Word BM25 exists only for word-branch rows;
  it is a tie-breaker inside the rerank, never the primary order across
  branches, because a trigram-only row has no BM25 and a zero would sort
  it behind every negative word score.
- Regression test: more word-branch distractors than the cap plus one
  trigram-only target; the target must survive admission and win.

### 4. Rust rerank (precision)

One score per admitted row from a small const weight table in `queries.rs`.
Ties fall back to word BM25, then name length.

- Coverage is defined on two normalizations, not on `split_identifier`
  alone, because that splitter separates digit runs and turns
  `parseSha256Sidecar` into `parse Sha 256 Sidecar` (Codex finding 5):
  - token coverage: a query word equals one split token or a run of
    adjacent split tokens joined (`Sha`+`256`), case-insensitive;
  - substring coverage: the lowercase collapsed name (`parsesha256sidecar`)
    contains the lowercase query word, for words of three or more
    characters;
  - stem-insensitive: compare Porter stems the way the word index does, so
    `validation` covers `validate`.
  Whole-name match, all-words-covered, and partial coverage are three
  tiers. Unsplit identifiers and stop-word-only queries keep the plan 019
  behavior. `find_related_tests` keeps its strict name query.
- Signature coverage and doc-comment coverage as weaker tiers.
- Kind prior, small: function, method, class, struct, trait, interface,
  enum, type above enum member, field, property, constant, variable;
  imports last. A strong name match always outranks a kind prior, so a
  constant or property that is the exact answer still wins (Codex finding
  6). Positive test: a constant, a property, and a script symbol each win
  their own query.
- Path role, small: `scripts`, `examples`, `benchmarks`, `fixtures`,
  `vendor` demoted. Same rule: never above a strong name match, and a
  query that names the path (`launcher`, `script`) cancels the demotion.
- Test intent: tests are excluded in SQL unless `include_tests`, so a
  test-intent boost only applies when tests are included. The default
  contract does not change. The MCP and CLI descriptions stay accurate.
- Documentation rows keep the existing last tier.
- **Dropped from the first implementation:** reference count. The
  `relationships` table holds resolved same-file edges only (0 of 740 edges
  in this checkout cross a file; plan 018 measured the same), so it would
  reward same-file organization, not repository-wide use, and 200 indexed
  counts are not free (Codex finding 4). **Dropped:** dominant-language
  affinity; it would push down the JavaScript launcher that motivated the
  plan. Either may return later with its own development-set evidence.
- Cost target: the rerank over at most 200 rows is measured with an
  in-process timer around the rerank call, printed by the explain output
  below, not inferred from MCP telemetry, which records whole-call
  milliseconds (Codex finding 8).

### 5. Explain output and result semantics (Codex finding 8)

- `--verbose` today enables debug logging and must stay that. Add
  `--explain` to `code-kb search`: text mode prints the feature breakdown
  per row; JSON mode adds an `explain` object per result. `explain` is
  never emitted without the flag, so existing JSON consumers see no change.
- `score` in results becomes the rerank score; the old BM25 value moves to
  `explain.bm25` when present. Snippets: word-branch rows keep FTS5
  snippets; trigram-only rows get the name with the matched substring
  bracketed; exact-name rows get the name.
- CLI and MCP share one code path and one parity test. README, the CLI
  help, the routing block, and the MCP `search_symbols` description are
  updated together. `AGENTS.md` and `CLAUDE.md` stay identical.

### 6. Regression gate inside the repo

- A synthetic multi-language corpus test beside
  `crates/code-kb-core/tests/resolution_test.rs`, built with that file's
  `scanned_repo` pattern: fixture files are written to a temporary
  directory, julie-extract scans that directory into its own database, and
  the test queries it. Only fixture files enter that database, and the
  query strings live in the test binary, so the self-contamination seen in
  plan 019 cannot recur.
- Fixtures: Rust, TypeScript, Python, C#, and Go files with camelCase,
  PascalCase, and snake_case names, digits inside names, doc comments, an
  enum with short variants, a `scripts` directory, a constant and a
  property that are correct answers, a minority-language file that must
  win, and markdown sections that contain every query word.
- Asserts top-1 for exact, camel-to-snake, snake-to-camel,
  substring-in-name (`sha256`), acronym and digit runs, short tokens,
  Unicode names, stem variants, and concept queries; asserts the kind and
  path rules; asserts the admission test from design 3.

## Tasks, in order

1. Write and seal the evaluation sets (development, acceptance, regression)
   outside the checkout. Half a session.
2. Corpus gate (design 6). It fails on the current build for the substring
   and enum-variant cases. Half a session.
3. Migration (design 2) with its tests, including the extractor-driven
   trigger tests and the Windows run. One session.
4. Trigram table and candidate admission (designs 1 and 3). Measure on the
   code-kb checkout and on hermes-agent: first-start migration time, peak
   RSS during migration, database and WAL size before and after,
   incremental `update` and `delete` latency, and a concurrent reader
   during migration. One session.
5. Rerank and explain output (designs 4 and 5). Tune on the development set
   only. One session.
6. Run the regression set (must not regress) and the sealed acceptance set
   once. Update plan 019's tables. Half a session.
7. Release v1.2.0. The release notes state the migration cost and index
   growth measured in task 4.

## Acceptance

- Regression set: no query ranks worse than in plan 019.
- Acceptance set, run once: file@1 at least 8 of 10 and symbol@1 at least
  7 of 10 on each of the four repositories.
- Warm `code-kb search` p50 stays under 25 ms on the code-kb checkout;
  rerank time from the explain timer stays under 1 ms at 200 candidates.
- Live `serve` RSS measured before and after: no rise beyond noise. Peak
  RSS during migration reported.
- First-start migration on hermes-agent completes, and its duration, disk
  growth, and WAL peak are recorded in the release notes.
- A failed migration is visible to the first tool call and retried on the
  next start; it is never reported as success.

## Risks

- Trigram index size on very large repositories. Mitigation: names only,
  locals excluded; measured in task 4 before the rerank work starts.
- Rerank weights overfit. Mitigation: tune on the development set, accept
  on the sealed set, keep the weights in one table.
- Migration on existing installs runs once per index at startup. Mitigation:
  transactional, retryable, and measured on the largest local repository.

## Not in scope

Semantic embeddings, Tantivy, container expansion (Miller surfaces a struct
when its methods match), synonym lists, reference-count and language
priors. Container expansion can be a follow-up if the acceptance set shows
a need.

## Results (2026-09-20)

Binary: `target/release/code-kb` built from `fe93213` on
`feat/search-v1.2-recall-rerank`. It still reports version 1.1.4; the bump
is part of task 7. The sets, `runner.py`, and every results file live in
`~/.code-kb/search-eval/` outside the checkout. v1.2 numbers come from
`results-v1.2.0-{regression,development,acceptance}.json` and the console
tables in the matching `.txt` files. v1.1.4 numbers come from
`baseline-v1.1.4-{regression,development}.json`, run through the
byte-identical copy `bin/code-kb-1.1.4`. The runner defines the columns:
file@k counts the cases whose expected file is in the top k of 10; sym@k
also needs the symbol name; MRR is the mean of 1/rank with a miss as 0;
p50 is the median wall time of one `code-kb search` process, which includes
the per-command reconcile of changed files.

### Acceptance set (sealed, 10 per repository, run once)

Target: file@1 at least 8 of 10 and sym@1 at least 7 of 10 on each
repository.

| repository | file@1 | file@3 | file MRR | sym@1 | sym@3 | sym MRR | p50 ms | file@1 >= 8 | sym@1 >= 7 |
|---|---|---|---|---|---|---|---|---|---|
| code-kb | 8 | 9 | 0.88 | 6 | 6 | 0.68 | 27 | met | not met |
| hermes-agent | 1 | 1 | 0.12 | 1 | 1 | 0.12 | 803 | not met | not met |
| julie | 5 | 6 | 0.57 | 4 | 6 | 0.50 | 43 | not met | not met |
| miller | 5 | 7 | 0.61 | 3 | 6 | 0.43 | 85 | not met | not met |
| all (40) | 19 | 23 | 0.54 | 14 | 19 | 0.43 | 64 | | |

Only code-kb file@1 meets the target. Every other number misses it. The
lead's reading of the misses:

- Most acceptance queries are README sentences. Their words do not appear
  in the name, signature, or doc comment of the answer (`ha-acc-doctor` and
  `ha-acc-past-conversations` are two examples). Lexical search cannot
  bridge that gap, and this plan excluded semantic search on purpose.
- Three hermes-agent misses rank a symbol from a test file first because
  julie did not flag the file as a test. Two of them show it in the
  recorded top-1 (`ha-acc-past-conversations`, `ha-acc-subagents`).
- The hermes-agent p50 of 803 ms is the per-command reconcile walk over
  12,788 files, not the search.

These are follow-ups. They are not fixed on this branch, because the sealed
set is not run again.

### Development set (89 queries, tuned on this set only)

| repository | n | version | file@1 | file@3 | file MRR | sym@1 | sym@3 | sym MRR | p50 ms |
|---|---|---|---|---|---|---|---|---|---|
| code-kb | 23 | v1.1.4 | 20 | 21 | 0.90 | 18 | 19 | 0.82 | 20 |
| code-kb | 23 | v1.2 | 21 | 23 | 0.95 | 21 | 23 | 0.94 | 22 |
| hermes-agent | 22 | v1.1.4 | 11 | 14 | 0.59 | 6 | 9 | 0.37 | 685 |
| hermes-agent | 22 | v1.2 | 14 | 18 | 0.74 | 12 | 18 | 0.69 | 713 |
| julie | 22 | v1.1.4 | 14 | 19 | 0.76 | 12 | 17 | 0.67 | 44 |
| julie | 22 | v1.2 | 18 | 21 | 0.89 | 17 | 20 | 0.85 | 33 |
| miller | 22 | v1.1.4 | 10 | 11 | 0.54 | 6 | 9 | 0.37 | 71 |
| miller | 22 | v1.2 | 16 | 19 | 0.81 | 11 | 17 | 0.63 | 66 |
| all | 89 | v1.1.4 | 55 | 65 | 0.70 | 42 | 54 | 0.56 | 66 |
| all | 89 | v1.2 | 69 | 81 | 0.85 | 61 | 78 | 0.78 | 46 |

### Regression set (26 plan 019 queries, code-kb repository)

v1.2: file@1 25, file@3 26, file MRR 0.98, sym@1 21, sym@3 25, sym MRR 0.89,
p50 23 ms. v1.1.4: 22, 24, 0.89, 17, 20, 0.73, 21 ms. Eight cases improved.
Two cases rank one place lower than v1.1.4 (`concept-fts` file 1 -> 2,
`concept-telemetry` symbol 1 -> 2); the lead ruled both acceptable. Plan
019's "v1.2 (plan 020) rerun" section has the tuning and held-out split and
the reason for each case.

### Migration (task 4m, build from `8ab5a50`)

- code-kb index (6,998 symbols, 21.7 MiB): migration 25-34 ms. Database
  size unchanged, because the new table fit in the freelist (262 KB
  footprint). WAL peak 766 KB. Peak RSS 7.9 MB in-process.
- hermes-agent index (990,975 symbols, 1.62 GiB): migration 3.9 s on tmpfs,
  5.8 s on NVMe, 6.1 s through the CLI. Database +31,375,360 B (+1.8%).
  WAL peak 73.4 MiB. Peak RSS 9.5 MB in-process.
- Trigger overhead: update and delete medians roughly double (0.02 -> 0.05
  ms); insert +0.1-0.26 ms; all medians under 0.5 ms.
- A second process during a migration waits on the write lock (busy timeout
  60 s on the migration connection) instead of failing.

### Latency and memory

- Rerank timer (explain output, code-kb checkout): 177 candidates in
  0.44 ms, the largest set observed. Worst rate 4.5 µs per candidate, about
  0.9 ms projected at 200.
- Warm `code-kb search` median 24.3 ms (hyperfine, 20 runs) against 21.9 ms
  for v1.1.4 in the same session. The added time is the two extra recall
  branches, not the rerank.
- Live `serve` RSS after 8 `search_symbols` calls plus one lookup, measured
  twice each: v1.1.4 RSS 30.58 MB, PSS 27.9 MB, anonymous 22.5 MB; v1.2 RSS
  30.7-30.8 MB, PSS 28.1 MB, anonymous 22.7 MB. Difference 0.2 MB, within
  noise.

### Post-review fixes on `main` (2026-09-20)

A Codex review of the merged change (v1.1.4..main) found two admission
defects; both are fixed on `main` and measured below.

- **The word branch ran the AND query and fell back to OR only when every
  row was documentation**, not the OR query section 3 specifies. One row
  that matched every word hid every partial match, and neither other branch
  reached them (`ha-concept-strip-ansi` was a miss for this reason). The
  fix admits the AND rows first and then fills the word cap from the OR
  query; a row admitted by both keeps its first BM25, and the two passes
  share one cap (a second Codex pass found the first version let each pass
  spend a full cap).
- **`kind: "variable"` appended locals after the limit.** The trigram branch
  now reaches globals such as `getChecksum`, so at a small limit an exact
  local `checksum` fell off the end. Locals join the candidate set by name
  and go through the rerank like every other row (and get `explain`).
- Found in the lead's own review: `path_role` split paths on `/` only, so
  the `scripts`/`examples`/... demotion did not apply to Windows paths.
- Reconcile (same review series): files the extractor reports as
  unsupported are remembered in a `skipped_files` table so they are not
  re-sent to the extractor on every start; only files whose update
  succeeded are remembered, so a transient extractor failure is retried.
  Warm `code-kb search` on this checkout went from 24.2 ms to 17.6 ms.

Measured with the same runner as above, final build. Regression set: one
rank changed, `ho-outline` symbol 2 -> 1 (sym@1 22 of 26). Development set:

| repository | n | file@1 | file@3 | file MRR | sym@1 | sym@3 | sym MRR | p50 ms |
|---|---|---|---|---|---|---|---|---|
| code-kb | 23 | 21 | 23 | 0.95 | 21 | 23 | 0.94 | 17 |
| hermes-agent | 22 | 15 | 20 | 0.79 | 13 | 20 | 0.75 | 216 |
| julie | 22 | 17 | 21 | 0.87 | 16 | 20 | 0.83 | 36 |
| miller | 22 | 16 | 19 | 0.81 | 11 | 18 | 0.64 | 67 |
| all | 89 | 69 | 83 | 0.85 | 61 | 81 | 0.79 | 47 |

Against the branch numbers (69, 81, 0.85, 61, 78, 0.78): three cases
improved (`ha-concept-strip-ansi` file 8 -> 1 and symbol miss -> 1,
`ha-concept-coerce-args` 4 -> 2, `mi-concept-pack-budget` symbol 6 -> 3)
and one slipped one place (`ju-acronym-csr` 1 -> 2: `action_csrf_token`
ties `csr` on the query `csr adjacency` because both names contain `csr`,
and BM25 breaks the tie). Two variants were rejected on this set: a pure OR
word branch with no AND pass (67 / 82 / 59 / 78, `mi-stem-canonicalizing`
became a miss) and a union where each pass spent its own cap (68 / 81 / 60
/ 80). The hermes-agent p50 fell from about 700 ms to 216 ms because the
reconcile no longer re-sends unindexable files to the extractor.

### Corpus gate

`crates/code-kb-core/tests/search_corpus_test.rs`: 12 of 12 green, none
ignored.

### Acceptance criteria

| criterion | result |
|---|---|
| Regression set: no query ranks worse than in plan 019 | Not met to the letter. 2 of 26 cases rank one place lower, both ruled acceptable; 8 improved; every total rose. |
| Acceptance set: file@1 >= 8 and sym@1 >= 7 per repository | Not met. code-kb file@1 (8) meets it; the other seven numbers miss. |
| Warm search p50 under 25 ms on the code-kb checkout | Met. 24.3 ms (hyperfine); regression-set p50 23 ms. |
| Rerank under 1 ms at 200 candidates | Met. 0.44 ms at 177; 0.9 ms projected at 200. |
| Live `serve` RSS: no rise beyond noise; migration peak RSS reported | Met. +0.2 MB; migration peak RSS 7.9 MB (code-kb) and 9.5 MB (hermes-agent). |
| First-start migration on hermes-agent completes; duration, growth, WAL peak in the release notes | Measured (5.8 s NVMe, +31,375,360 B, WAL peak 73.4 MiB). The release notes are task 7. |
| Failed migration visible to the first tool call and retried; never reported as success | Met. `scan_workspace` and the CLI return the migration error instead of discarding it (`8ab5a50`, `6997b20`); `db.rs` tests cover an interrupted migration, a missing table, and a concurrent writer. |
