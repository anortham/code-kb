# 019: Search Quality Comparison (code-kb vs julie vs miller)

Date: 2026-09-20. Status: query-side fixes 1-3 implemented and reviewed by Codex the same day. Fix 4 (index-side name splitting) is open.

## Method

16 hand-labeled queries against the code-kb checkout (Rust, 6911 symbols).
Each provider ran as a CLI with limit 10. A hit counts at file level when the
path matches, and at symbol level when the symbol name also matches. The
runner script must live outside the checkout: when it was copied into
`scripts/`, its own query list was indexed and became the top hit for every
concept query. Keep it in a scratch directory and paste the table here.

Providers: `code-kb search --json`, `julie-server search --semantics off`
(8.0.0, Tantivy), `miller search --arm lexical` (SQLite, camel/snake token split).

Semantic arms did not run. Julie's daemon reported `Embeddings: None` after
indexing and `SEMANTICS_NOT_READY` on every query. Miller's vector sidecar
never received its completeness stamp because that needs a resident leader
that drains the vector target. Both are problems in those repos, not here.

## Results

| provider | file@1 | file@3 | file@5 | file MRR | sym@1 | sym@3 | sym MRR | p50 ms |
|---|---|---|---|---|---|---|---|---|
| code-kb | 9 | 9 | 10 | 0.58 | 7 | 9 | 0.52 | 64 |
| julie-lexical | 10 | 11 | 11 | 0.66 | 3 | 4 | 0.26 | 442 |
| miller-lexical | 11 | 15 | 15 | 0.81 | 9 | 13 | 0.70 | 262 |

code-kb is 4-7x faster and beats Julie on symbol-level answers, because Julie
returns file lines for most concept queries. Miller wins overall.

## Why code-kb misses (6 of 16 queries)

1. **All-terms-required query hides code behind docs.** `sanitize_fts5_query`
   builds an AND query and falls back to OR only when AND returns nothing.
   For "syntax validation before edit" only markdown sections contain all four
   words, so the result is five doc sections and no code. The same OR query
   with docs demoted returns `validate_syntax` at rank 1. Same cause for
   "copy parent repository index for git worktree" (`copy_parent_index` is
   rank 1 under OR), "discover workspace root from a path", "blast radius
   multi hop reachability", and the julie-extract locator query.
2. **No camelCase or snake_case splitting.** "ValidateSyntax" finds nothing.
   The FTS `name` column uses `porter unicode61`, which splits on `_` but not
   on case changes, and the query side does not split either. Miller finds
   `validate_syntax` at rank 1.
3. **Exact name is not boosted.** "search_symbols_scoped" returns
   `fts_search_symbols_scoped` first because BM25 sees the same tokens.

## After the fix

The 16 cases above found the defects and then measured the fix, so they are a
tuning set. A second set of 10 held-out queries was written after the fix and
run once, without tuning. It includes cross-language cases against the
JavaScript launcher. Both sets, all providers re-run on the same day:

| set | provider | file@1 | file@3 | file MRR | sym@1 | sym@3 | sym MRR | p50 ms |
|---|---|---|---|---|---|---|---|---|
| tuning (16) | code-kb after | 15 | 15 | 0.95 | 11 | 13 | 0.76 | 58 |
| tuning (16) | julie lexical | 9 | 11 | 0.62 | 3 | 4 | 0.26 | 423 |
| tuning (16) | miller lexical | 10 | 15 | 0.77 | 8 | 13 | 0.66 | 264 |
| held-out (10) | code-kb after | 7 | 9 | 0.80 | 6 | 7 | 0.68 | 57 |
| held-out (10) | julie lexical | 7 | 8 | 0.75 | 2 | 3 | 0.25 | 507 |
| held-out (10) | miller lexical | 10 | 10 | 1.00 | 9 | 10 | 0.95 | 522 |

Reading: code-kb now beats Julie's lexical search on both sets and is the
fastest by 4-9x. Miller wins the held-out set. Its remaining edge is that it
splits camelCase names at index time: "parse the sha256 sidecar file" cannot
reach `parseSha256Sidecar`, which FTS5 stores as one token, because a
four-word query is never concatenated. The other two held-out misses are
rank 2 behind the short CLI enum variants `Skeleton` and `Outline`.

code-kb latency is 14 ms warm and about 60 ms when the CLI first reconciles
files changed since the last run. The earlier "latency unchanged" wording was
wrong; the fix did not change query cost, but the two runs were measured in
different states.

## Codex review (same day)

Codex found two code defects, both fixed before this table was measured:

- A stop-word prefix dropped the unsplit identifier, so `isReady` searched
  only `Ready` and missed the camelCase symbol; a symbol named `before`
  produced an empty query. Now every split identifier keeps its unsplit form,
  and a query made only of stop words is searched as typed.
- The unsplit alternative was ORed against the whole conjunction, so
  "fooBar quux" matched a row with only `fooBar` and blocked the OR
  fallthrough. Now each identifier's alternatives are grouped inside its own
  required term: `(("foo"* "Bar"*) OR "fooBar"*) AND "quux"*`.
- `find_related_tests` no longer shares the sanitizer; it uses the symbol
  name as typed, so `isReady` does not pull in `test_ready`.

Codex's third point, that the tuning set cannot support a general claim, is
why the held-out set exists.

## Fix plan (small, in `crates/code-kb-core`)

1. `sanitize_fts5_query`: split each token on case changes and digits before
   quoting (`ValidateSyntax` -> `validate syntax`); drop a short stop-word
   list (a, an, the, for, to, of, in, on, and, or, with, from, by, before,
   after); no prefix wildcard for tokens under 3 chars. Keep AND then OR.
2. `fts_search_symbols_scoped`: run the OR query too when the AND result
   contains only documentation rows, and merge code rows first. Alternative:
   always OR and rely on BM25 plus the existing docs-last ordering; measure
   both on the 16 cases.
3. Add `(s.name = :raw_query COLLATE NOCASE) DESC` to the ORDER BY.
4. Open. The FTS table is an external-content table over `symbols`, so a
   computed column is not possible, and a split function in the triggers
   would have to exist inside julie-extract's process, which writes the
   symbols. The query side carries each identifier unsplit and concatenates
   two- and three-word queries, which covers `validate syntax` ->
   `validateSyntax` but not a longer phrase. Closing the held-out gap needs a
   code-kb-maintained words table refreshed per changed file after each sync,
   or a split-name column written by the extractor. Decide before v1.2.

Result on the tuning set: file@3 from 9 to 15, sym@1 from 7 to 11.

## Not planned

Semantic embeddings. Plan 011 records the Miller calibration where the
semantic arm lost to lexical at a fixed token budget, and this run could not
produce a semantic result from either tool without extra daemons.
