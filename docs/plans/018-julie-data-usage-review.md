# 018: How well code-kb uses the julie-extract data (2026-09-18)

Review of code-kb main @ f12ef02 (clean) and julie-extractors main @ 9c2fcf1f (clean).
Evidence: schema v7 DDL, every SQL statement in `crates/code-kb-core/src`, live `code-kb`
1.0.3 runs on this repo (facts level, julie 3.0.0, 6,237 symbols), and SQLite queries on the
hermes-agent index (full level, 991k symbols) for scale numbers. No agents were used.

## Verdict

The seam is sound. Plan 017 items are in place: code-kb requests the `facts` level, the
version guard compares against the binary in use, scans pass `--parent-pid`, and syntax
checks go through `julie-extract check`. Every table code-kb needs exists at the `facts`
level. The remaining gaps are in how code-kb reads the data, plus two data-quality issues
that start in julie. None needs a schema change.

## What code-kb reads today

| Table | Read by | Notes |
|---|---|---|
| `symbols` | everything | `semantic_group` is read into the model but is NULL in every row of both indexes |
| `files` | outline, sync | `status = 'unsupported'` rows are never called out |
| `relationships` | refs, context, blast radius, related tests | julie resolves same-file only: 597 of 597 edges here are same-file |
| `pending_relationships` | refs, blast radius | code-kb's shared predicate is the whole cross-file resolver |
| `identifiers` | refs (callers) | `type_usage` and `member_access` only, as designed |
| `type_facts` | context, receiver ranking | 818 of 1,223 rows here sit on local variables |
| `structural_facts`, `literals` | find_structural_facts | `metadata_json` is never read (finding 4) |
| `artifact_metadata` | version guard | |

Never read: `reference_sites`, `symbol_annotations`, `complexity_metrics`,
`parse_diagnostics`, `extraction_revisions`, `revision_file_changes`,
`language_capabilities`, `language_capability_gaps`, `symbols.metadata_json`,
`symbols.content_type`, `symbols.test_lifecycle`, `pending_relationships.metadata_json`,
`pending_relationships.caller_scope_symbol_id`.

## Findings, ranked

### 1. Defect (julie, guarded in code-kb): markdown code blocks are reported as tests

**Where:** julie `crates/julie-extractors/src/markdown/semantic_symbols.rs:58` applies the
rustdoc rule (a fenced block with no language or `rust` is a test) to every standalone
`.md` file. code-kb `find_related_tests` (`queries.rs:561`) then lists them.

**Live:** `code-kb context scan_workspace` shows one related test: `rust code block`
in `docs/plans/014-pre-release-deep-review-findings.md:494`. `context bind_workspace`
shows three of them. This index has 62 markdown "tests". hermes-agent has 2,088, in files
such as `AGENTS.md` and `CONTRIBUTING.es.md`.

**Fix:**
- julie: do not apply the test role in the markdown extractor. Standalone markdown is not
  a rustdoc target. Needs a `## <version>` entry in `docs/contracts/extraction-output-changes.md`
  because `is_test` and `test_role` values change.
- code-kb: exclude `content_type = 'documentation'` in `find_related_tests` and in the
  blast-radius test list, so old indexes stay clean too.

### 2. Defect (code-kb): `find_related_tests` misses every test in another file

**Where:** `queries.rs:561`. Step 1 joins `relationships` only. julie resolves edges
inside one file only, so a test in `tests/` never reaches step 1. Step 2 falls back to a
name `LIKE` match, which fails for most test names.

**Live:** `scan_workspace` has 52 pending call edges from callers, 46 of them test
functions. `context scan_workspace` lists none of them (only the markdown block from
finding 1). `blast-radius scan_workspace` lists 20 real tests because it reads pending
edges with the shared predicate.

**Fix:** In step 1, union a query over `pending_relationships` using
`pending_target_predicate`, the same one `find_references` and `blast_radius` use. One
test: a test function in another file that calls the target appears in the context slice.

### 3. Quality (code-kb): local variables and parameters flood search and lookup

**Where:** `db.rs:97` (FTS content), `queries.rs:307` (search), lookup.

**Numbers:**

| Index | Locals and parameters | All symbols |
|---|---|---|
| code-kb | 3,336 (53%) | 6,237 |
| hermes-agent (python) | 386,434 | 398,639 variables |
| hermes-agent, name `config` | 1,993 variables | 3 functions |

**Live:** `search "connection"` returns 20 rows of `conn: &Connection`. `lookup conn`
returns 20 rows of `let conn`. `lookup new` returns two real `new` methods, then six
`new_body` locals.

**Why not use julie's `role`:** julie writes `metadata_json.role = "local"` from the Rust
and C# extractors only. Python sets `role` on 45% of variables. TypeScript never sets it.
The structural signal is uniform across all languages: a `variable` whose parent is a
`function`, `method`, or `constructor` is a local or a parameter.

**Fix (code-kb only):**
- Keep the rows in the DB. Receiver ranking joins `type_facts` on local variables
  (984 pending edges here match through that join).
- Leave them out of the FTS content and of `lookup` and `search` results unless the caller
  asks for `kind = "variable"`. One shared SQL fragment, used by the FTS insert triggers,
  the rebuild, and the two queries.
- Side note: the exclusion lists name a `parameter` kind that julie never emits.
  Parameters arrive as `variable` rows. Harmless, but the list gives false comfort.

### 4. Quality (code-kb): `find_structural_facts` drops the fact's key

**Where:** `queries.rs:1557`, `formatters.rs:453`.

**Live:** `code-kb facts config` prints
`key_value [.github/workflows/ci.yml:3] (pattern: yaml.key_value.v1, in: on)`. The key
name is in `metadata_json`: `{"key":"name","key_path":"$.name","value_kind":"scalar"}`.
TOML rows carry `key_path` such as `mcp_servers.code-kb.command`. Route patterns carry
`normalized_route_template` and `verb` (see julie `docs/contracts/structural-fact-patterns.json`).

**Fix:** Select `json_extract(sf.metadata_json, '$.key_path')`, then `$.key`, then
`$.normalized_route_template`, and print the first one present. About ten lines.

### 5. Quality (code-kb): documentation headings outrank code in search

**Live:** `search "reconcile offline edits"` returns one function and five markdown
headings from `docs/plans`. julie marks these rows `content_type = 'documentation'`
(827 here). code-kb never reads the column.

**Fix:** Add `(s.content_type IS NULL OR s.content_type != 'documentation') DESC` ahead of
the BM25 rank in the FTS query. Documentation still appears, after code.

### 6. Efficiency (julie): the `facts` level still writes data nobody reads

`dbstat` on this repo's 21 MB index:

| Bytes | Object | Read by code-kb |
|---|---|---|
| 4.7 MB | `reference_sites` and its three indexes | never |
| 0.7 MB | `idx_pending_reference_site` | never |
| 0.5 MB | `idx_pending_caller_scope` | never; `caller_scope_symbol_id` equals `from_symbol_id` in 100% of rows |
| small here, 182k rows on hermes | `complexity_metrics` | never |
| small | `symbol_annotations` | never |

About a quarter of the index. On the hermes full-level index `reference_sites` was 880 MB.

**Fix, in order of cost:**
- julie: skip `complexity_metrics` and `symbol_annotations` at the `facts` level. No schema
  change; the tables stay empty. Ledger entry required.
- julie: `reference_sites` is an FK target of `identifiers`, `relationships`, and
  `pending_relationships`, so dropping it is a schema v8 change. Do it only when another
  schema change is already planned.

### 7. Small (code-kb): parse failures and unsupported files are invisible

`parse_diagnostics` has 81 rows on hermes-agent. `files.status = 'unsupported'` covers
1,294 hermes files. code-kb reads neither. A skeleton of a file with parse errors looks
complete. Fix: one line in `file_skeleton` output when the file has diagnostics, and a count
of unsupported files in `codebase_outline`.

### 8. Note (julie): `role` metadata is undeclared and inconsistent

`symbols.metadata_json.role` is not in `docs/contracts/sqlite-schema-v7.md`. Either declare
it best-effort or drop it. Finding 3 does not depend on it.

## Not findings

- `semantic_group` is NULL everywhere; code-kb reads it into `Symbol` for nothing. Trivial.
- `test_lifecycle`, `extraction_revisions`, `language_capability_gaps`: unread, and there
  is no tool that needs them.
- `identifiers` at the `facts` level (type usages and member accesses only) is the right cut.
  Call sites live in `pending_relationships`.

## Suggested order

1. Findings 1 (code-kb guard) and 2: same function, same test file. One session.
2. Finding 3, then 5, then 4: search quality. One session.
3. Finding 1 (julie side) and 6 (annotations and complexity at `facts`): one julie release
   with one ledger entry, then bump the pin in code-kb.
4. Finding 7 when convenient.
