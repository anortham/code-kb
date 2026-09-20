# 020: Search v1.2 — name recall and a deterministic rerank

Date: 2026-09-20. Status: plan, not started. Follows plan 019. Revised the
same day after a Codex review (eight findings, all folded in below).

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
