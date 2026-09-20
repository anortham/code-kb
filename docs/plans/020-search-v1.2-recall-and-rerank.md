# 020: Search v1.2 — name recall and a deterministic rerank

Date: 2026-09-20. Status: plan, not started. Follows plan 019.

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

## Design

### 1. Trigram FTS table on symbol names (recall)

- New virtual table `symbol_names_tri` with FTS5 `tokenize='trigram'` over
  the `name` column only, as an external-content table on `symbols` like
  `symbols_fts`. Same insert, update, and delete triggers, same local-variable
  exclusion. Plain SQL, so the triggers also work inside julie-extract, which
  writes the symbols table directly.
- Verified on the bundled SQLite 3.51: `MATCH 'sha256'` finds
  `parseSha256Sidecar`. Trigram matching is case-insensitive by default.
- Query side: each query word of three or more characters becomes a trigram
  term. The recall query is the union of rowids from `symbols_fts` (word and
  prefix match) and `symbol_names_tri` (substring match), scored by the word
  table's BM25 where present, otherwise by a name-coverage score from the
  rerank.
- Bump `FTS_RULE` so existing indexes rebuild both tables once.
- Do not put trigrams on `signature` or `doc_comment`. Names are short, so
  the index stays small; doc text would not.
- Alternative kept open: julie-extract emits a `name_words` column (`parse
  sha256 sidecar`) at its next schema bump. Smaller index and stemming
  applies, but it couples two releases. Revisit when julie bumps schema.

### 2. One recall query, then a Rust rerank (precision)

- Replace the AND-then-OR pair with one OR query that fetches up to
  `max(limit * 5, 50)` candidates, capped at 200, ordered by BM25. Rerank
  in Rust, return the top `limit`.
- Features, each a small integer or fraction computed from the row and the
  query, weighted and summed. Ties fall back to BM25:
  - exact name match, case-insensitive; qualified suffix match (`Foo::bar`,
    `Foo.bar`)
  - name coverage: fraction of query content words found in the split name
    (`split_identifier` from plan 019); this gives "all but one word"
    behavior without a second FTS query
  - signature coverage and doc-comment coverage as weaker tiers
  - kind prior: function, method, class, struct, trait, interface, enum, type
    above enum member, field, property, constant, variable; imports last
  - reference count from `relationships` as an importance signal, log-scaled;
    computed for the candidates only, one indexed count per row
  - path role: rows under `scripts`, `examples`, `benchmarks`, `fixtures`,
    `vendor` demoted; test paths demoted unless the query says test
  - dominant-language affinity: the language with the most symbols in the
    index gets a small boost
  - documentation rows keep the existing last-tier ordering
- Explainable: `code-kb search --verbose` prints the feature breakdown per
  row. Weights live in one const table in `queries.rs`.
- Cost: one FTS query plus at most 200 small computations and 200 indexed
  counts. Expected well under 1 ms of rerank time.

### 3. Regression gate inside the repo

- A synthetic multi-language corpus test beside
  `crates/code-kb-core/tests/resolution_test.rs`: Rust, TypeScript, Python,
  C#, and Go files with camelCase, PascalCase, and snake_case names, doc
  comments, an enum with short variants, a scripts directory, and markdown
  sections that contain every query word.
- Queries live as string literals inside test function bodies, which the
  index never sees, so the gate cannot contaminate itself the way an indexed
  query list did in plan 019.
- Asserts top-1 for exact, camel-to-snake, snake-to-camel, substring-in-name,
  and concept queries, and asserts the kind and path demotions.

### 4. Cross-tool bench stays outside the checkout

- Keep the harness from plan 019 in a sibling directory, never in this repo.
  Grow the held-out set across hermes-agent (Python), julie (Rust), and
  miller (C#), 10 queries each, written before any tuning.
- Report both sets per provider in the plan doc. Claim only what the
  held-out set supports.

## Tasks, in order

1. Corpus test first (design 3). It fails on the current build for the
   substring and enum-variant cases. About half a session.
2. Trigram table plus `FTS_RULE` bump (design 1). Measure index size on the
   code-kb checkout and on the largest local repo. About one session.
3. Rerank (design 2). Start with exact name, name coverage, kind prior, and
   path role. Add reference count and language affinity only if the held-out
   set moves. About one session.
4. Re-run the cross-tool bench on both sets and update plan 019's tables.
5. Release as v1.2.0: an `FTS_RULE` bump rebuilds every user's FTS tables
   once at first start.

## Acceptance

- Held-out set: file@1 at least 9 of 10 and symbol@1 at least 8 of 10 on
  code-kb's own checkout. Tuning set does not regress.
- Warm `code-kb search` p50 stays under 25 ms on the code-kb checkout.
- Live `serve` RSS measured before and after: no rise beyond noise.
- Index size growth from the trigram table reported in the release notes.

## Risks

- Trigram index size on very large repositories. Mitigation: names only,
  locals excluded; measure before merging.
- Rerank weights overfit the tuning set. Mitigation: tune on the tuning set,
  accept only on the held-out set, keep the weights in one table.
- The `FTS_RULE` rebuild runs once per existing index at startup. It is
  bounded by symbol count; the first tool call waits for it as today.

## Not in scope

Semantic embeddings, Tantivy, container expansion (Miller surfaces a struct
when its methods match), and synonym lists. Container expansion can be a
follow-up if the held-out set shows a need.
