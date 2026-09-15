## Review conclusion

Include the query-correctness fixes in v1.0.1. The audit identifies real defects, but overstates several severities and does not establish the proposed memory or timestamp optimizations as safe fixes.

Reviewed local `main` at `84dd5adbf88ea2b1a3b60eac0d15b47a66e5960e` using shell commands only. Source inspection and in-memory SQLite probes confirmed the principal SQL failures. No files changed; Cargo tests and RSS benchmarks were not run in this read-only environment.

The server file is `crates/code-kb-cli/src/mcp/server.rs`, not `src/server.rs`.

## Critical and high findings

### CRIT-01 — VERIFIED

- **Code analysis:** [queries.rs:1918](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:1918), lines 1918–1927, binds the same escaped value to exact equality and `LIKE`. `tests/cli_test.rs` becomes `tests/cli\_test.rs`. Exact equality fails, and the directory alternative cannot match that file.
- **Technical impact:** **High correctness defect**, rather than critical security severity. File-seeded impact analysis can silently omit callers and predicted tests. Literal `%` filenames also fail.
- **Recommended fix:** Allocate separate parameters for unescaped exact paths and escaped directory patterns:

```rust
let exact_path = raw;
let directory_pattern = format!("{}/%", escape_like(&exact_path));
```

```sql
replace(path, '\', '/') = :exact_path COLLATE NOCASE
OR replace(path, '\', '/') LIKE :directory_pattern ESCAPE '\'
```

Update parameter numbering accordingly.

### HIGH-01 — VERIFIED

- **Code analysis:** [queries.rs:2023](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:2023), lines 2023–2038, deduplicates `(symbol_id, depth)`. In `A → B → A`, seed `A` reappears at depth 2. Filtering out depth zero **before** grouping makes `A` an impacted caller.
- **Technical impact:** **Medium.** Incorrect self-impact, misleading depths, and wasted result slots. Depth still bounds traversal.
- **Recommended fix:** Remove `WHERE iw.depth > 0`, preserve the kind filter, and add this after `GROUP BY`:

```sql
HAVING MIN(iw.depth) > 0
```

This excludes every seed, including seeds reached through another seed. The SQLite reproduction returned `A@2, B@1` before this change and only `B@1` afterward.

### HIGH-02 — VERIFIED

- **Code analysis:** [queries.rs:739](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:739) supports exact paths and suffixes, but no directory-prefix alternative. `%/crates/code-kb-core` cannot match `crates/code-kb-core/src/queries.rs`.
- **Technical impact:** **High correctness defect.** Documented directory scoping fails during symbol resolution, particularly qualified lookup and reference queries.
- **Recommended fix:** Preserve exact and suffix matching; add this alternative inside the `:exact = 0` branch:

```sql
OR replace(s.path, '\', '/') LIKE :path_like || '/%' ESCAPE '\'
```

`:path_like` is already escaped at lines 750–751.

### HIGH-03 — VERIFIED

- **Code analysis:** [server.rs:809](/home/murphy/source/code-kb/crates/code-kb-cli/src/mcp/server.rs:809), lines 809–838, discards errors through `let Ok(Some(sym))`, then permits FTS fallback. The same pattern exists in [queries.rs:327](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:327) and [main.rs:497](/home/murphy/source/code-kb/crates/code-kb-cli/src/main.rs:497).
- **Technical impact:** **High correctness defect.** Qualified-name ambiguity becomes an empty result or unrelated search matches. Other query errors are also swallowed.
- **Recommended fix:** Make core qualified lookup propagate errors:

```rust
if query.contains("::") || query.contains('.') {
    if let Some(sym) = get_symbol_by_name(conn, query, path_filter)? {
        return Ok(vec![sym]);
    }
}
```

Have CLI and MCP lookup delegate to that core function and preserve its error. Fallback should follow a successful empty lookup, not an error.

### HIGH-04 — VERIFIED

- **Code analysis:** [queries.rs:1411](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:1411), lines 1411–1416, applies the namespace filename pattern to raw `s_to.path`. Backslash paths fail. Root-level `user.rs` also fails because the pattern requires a preceding slash.
- **Technical impact:** **Medium.** Missing pending callee signatures for affected paths. This does not establish that every current Windows artifact contains backslashes.
- **Recommended fix:** Replace the left operand with:

```sql
('/' || replace(s_to.path, '\', '/')) LIKE ...
```

This handles both separators and root-level files.

### HIGH-05 — VERIFIED

- **Code analysis:** [queries.rs:1619](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:1619) compares normalized paths using case-sensitive equality. The directory pattern does not rescue a case-varied exact filename.
- **Technical impact:** **Medium.** A relative Windows filter such as `src/Api.rs` can miss facts indexed under `src/api.rs`.
- **Recommended fix:**

```sql
replace(sf.path, '\', '/') = :path COLLATE NOCASE
```

Apply the same policy to structural-fact literals and category-listing predicates. SQLite `NOCASE` handles ASCII case differences; it is not full Unicode Windows filesystem identity handling.

### HIGH-06 — QUALIFIED

- **Code analysis:** [db.rs:37](/home/murphy/source/code-kb/crates/code-kb-core/src/db.rs:37), lines 37–44, sets a 256 MiB mapping limit on non-Windows systems. Windows already disables mapping. [server.rs:735](/home/murphy/source/code-kb/crates/code-kb-cli/src/mcp/server.rs:735) opens a local query connection for each tool call.
- **Technical impact:** Mapped pages can increase RSS while resident, but **the limit does not allocate 256 MiB of resident memory**. This source alone neither proves the reported retained RSS nor identifies its cause. Query connections closing also matters when distinguishing peak from idle memory.
- **Recommended fix:** First reproduce idle and peak RSS with a defined artifact and workload. Compare the current setting against `PRAGMA mmap_size = 0;` and `8388608`. Ship a lower limit only with measured memory and latency results. Neither lowering mmap nor adding `malloc_trim(0)` guarantees the 15 MB ceiling.

### HIGH-07 — QUALIFIED

- **Code analysis:** [sync.rs:336](/home/murphy/source/code-kb/crates/code-kb-core/src/sync.rs:336) selects size and hash; [sync.rs:405](/home/murphy/source/code-kb/crates/code-kb-core/src/sync.rs:405), lines 405–418, reads and hashes each encountered indexed file whose size matches. There is no mtime check.
- **Technical impact:** The reported behavior is **verified**: unchanged-file reconciliation reads data proportional to total indexed file bytes. Its high severity and claimed speedup are unmeasured here.
- **Recommended fix:** Do **not** implement `disk_mtime <= indexed_at ⇒ unchanged`. Preserved, backdated, or coarse timestamps can hide same-size edits. Retain hash verification for v1.0.1. A later optimization should persist indexing-time filesystem fingerprints, establish when those fingerprints are trustworthy, and retain hashing otherwise.

## Medium findings

### MED-01 — QUALIFIED

- **Code analysis:** [queries.rs:1856](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:1856) accepts unrestricted `usize` depth and casts it at line 1934. However, [ops.rs:326](/home/murphy/source/code-kb/crates/code-kb-core/src/ops.rs:326) already limits CLI/MCP traversal to five.
- **Technical impact:** **Medium library-API hardening issue.** Direct core callers can request excessive cyclic traversal. `UNION` deduplicates symbol/depth pairs, so describing this simply as exponential traversal is inaccurate.
- **Recommended fix:** Add `let max_depth = max_depth.min(5);` at the core boundary before casting. Preserve core depth-zero behavior unless intentionally changing that contract.

### MED-02 — VERIFIED, conditional on invalid stored data

- **Code analysis:** Unguarded `json_each(p.target_namespace_json)` appears at [queries.rs:1399](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:1399) and [queries.rs:1977](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:1977), among other reference branches. A parameterized SQLite probe confirmed that SQL empty text `''` raises `malformed JSON`; SQL `NULL` and JSON `[]` return no rows.
- **Technical impact:** **Medium robustness defect.** Invalid namespace data can break otherwise valid queries. This review did not establish that the pinned extractor normally produces such data.
- **Recommended fix:** Guard the function argument itself everywhere:

```sql
json_each(
  CASE WHEN json_valid(p.target_namespace_json)
       THEN p.target_namespace_json
       ELSE '[]'
  END
)
```

Also validate namespace arrays during ingestion. Avoid treating malformed metadata as evidence for unqualified call matches.

### MED-03 — VERIFIED

- **Code analysis:** [queries.rs:361](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:361) embeds an unescaped path inside `%…%`. Related search branches repeat this at lines 457 and 517. Input separators are normalized; stored separators are not.
- **Technical impact:** **Medium.** `_` and `%` broaden matches, and substring scope admits unrelated files such as `score.rs` for `core`.
- **Recommended fix:** Use normalized exact equality plus an escaped directory-prefix parameter, as in CRIT-01. Preserve any supported filename suffix matching as an explicit, escaped, component-boundary alternative. Apply consistently to ordinary and FTS search.

### MED-04 — VERIFIED

- **Code analysis:** [workspace.rs:603](/home/murphy/source/code-kb/crates/code-kb-core/src/workspace.rs:603), lines 603–618, resolves absolute filters through canonicalization. Relative filters only receive lexical cleanup at lines 620–628.
- **Technical impact:** **Low to medium.** An existing in-workspace symlink can succeed as an absolute filter and fail as a relative filter.
- **Recommended fix:** Resolve existing relative candidates under `canonical_root` through `resolve_path`, returning the canonical relative target. Preserve lexical handling for nonexistent prefixes and filenames. Do not turn an outside-workspace resolution failure into an accepted canonical target.

### MED-05 — VERIFIED

- **Code analysis:** [queries.rs:1919](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:1919) lacks `COLLATE NOCASE`. This remains broken after fixing escaping.
- **Technical impact:** **Medium.** Case-varied Windows exact-file seeds can yield empty impact results.
- **Recommended fix:** Include normalized, unescaped exact equality with `COLLATE NOCASE` in the CRIT-01 patch.

### MED-06 — QUALIFIED

- **Code analysis:** [main.rs:465](/home/murphy/source/code-kb/crates/code-kb-cli/src/main.rs:465) opens the artifact directly. Outline, lookup, search, references, blast radius, and facts do not refresh it before querying. However, skeleton explicitly refreshes at lines 483–489, and body/context use refreshing operations. MCP lookup/search likewise query the current index without a per-file freshness barrier.
- **Technical impact:** **Medium.** Untargeted CLI results can remain stale until indexing runs; MCP queries can race background updates. “All CLI invocations” is incorrect.
- **Recommended fix:** Define the freshness contract explicitly. Strict freshness requires completing reconciliation and updates before querying. Eventual freshness requires reporting synchronization state rather than silently implying current disk state. Comparing database mtime with repository mtimes is not a reliable correctness check.

### MED-07 — VERIFIED

- **Code analysis:** [ops.rs:57](/home/murphy/source/code-kb/crates/code-kb-core/src/ops.rs:57), lines 57–79, returns immediately when database lookup finds nothing. The no-file refresh at lines 81–88 is reachable only after finding an indexed symbol.
- **Technical impact:** **Medium.** A newly introduced symbol cannot trigger its own refresh without a file path; it remains undiscoverable until another indexing mechanism catches up.
- **Recommended fix:** Before returning a miss, refresh reliably identified pending changed/new files and retry lookup once. If no reliable candidate set exists, guaranteeing discovery requires reconciliation. Keep the existing explicit-file refresh path; avoid an unrestricted repository scan on every typo.

## v1.0.1 priorities

| Priority | Findings | Recommendation |
|---|---|---|
| Must include | CRIT-01, HIGH-02, HIGH-03 | Fix silent omissions and lost ambiguity diagnostics. |
| Must include | HIGH-01, HIGH-04, HIGH-05, MED-03, MED-05 | Correct traversal output and path matching across query families. |
| Include | MED-01, MED-02 | Small core-boundary and malformed-data hardening changes. |
| May defer | MED-04 | Lower-impact symlink-filter consistency; include if containment tests are ready. |
| Defer implementation, document limitation | MED-06, MED-07 | Coordinate a freshness policy and reliable changed-file discovery. |
| Defer proposed optimization | HIGH-06, HIGH-07 | Measure memory first; preserve hash-based correctness while designing startup optimization. |

Before release, add focused regressions for literal `_`/`%` paths, cycles and multiple seeds, directory scopes, qualified ambiguity in both CLI and MCP, backslash/root-level callees, case-varied paths, malformed namespace JSON, and excessive direct-core depth. Windows-specific claims still need Windows verification.