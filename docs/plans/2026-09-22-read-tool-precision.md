# Read-tool precision implementation plan

> For agentic workers: use `razorback:subagent-driven-development` with `terra_worker` implementers. Use `razorback:executing-plans` only when delegation is unavailable or the user selects single-agent execution. The lead owns design decisions, review, and integration verification.

**Status:** Implemented and locally verified on `refactor/read-only-tools` through `74baf19`. No push, merge, or release.

**Goal:** Make impact limits explicit, use consistent test discovery, and let agents select an exact symbol through the existing read tools.

**Base:** `e761398` on `refactor/read-only-tools`, in `/home/murphy/source/code-kb/.worktrees/read-only-tools`. Continue in this worktree. The edit-tool removal is committed here and is not yet on `main`.

**Architecture:** Extend the existing SQLite queries, operation functions, formatters, and CLI/MCP adapters. Reuse extractor symbol IDs and the existing test-path predicate. Keep ten MCP tools, with no new database tables, dependencies, service, or repository cache.

**Tech stack:** Rust, rusqlite/SQLite schema v7, julie-extract 3.3.1, clap, MCP JSON-RPC.

**Spec:** The [behavior contract below](#behavior-contract) implements the three owner-approved findings in [the toolkit assessment](../toolkit-assessment.md). This plan supersedes the remaining identity/completeness proposals in [the September 14 plan](2026-09-14-target-identity-and-result-completeness.md); do not repeat its already-completed path-filter and category work.

**Architecture quality:** Low architectural risk: the changes stay within existing layers and use the existing database identity. The main correctness risk is selecting the wrong definition after a refresh; the ID path must fail rather than fall back to a name. SQLite remains the query engine, and public tool/CLI contracts are the integration-test boundary.

## Evidence and existing interfaces

- `compute_blast_radius_scoped` in `crates/code-kb-core/src/queries.rs` reads up to 200 traversal rows, fetches up to 10 stem-matched files per stem, and then truncates tests and impacts independently to `limit`. The formatter cannot currently see that last truncation.
- Reproduced: `blast_radius(symbol="get_symbol_by_name", limit=1)` says one test found; `limit=20` reports 20 tests and 12 impacts. The smaller answer does not disclose its output limit.
- `blast_radius_op` currently changes explicit `limit=0` to 20. The core query already accepts zero, and the advertised range is 0–200. This plan deliberately makes explicit zero mean zero returned rows in both interfaces.
- `format_blast_radius` separately caps its text display at 20 tests and 50 impacts. Its traversal warning is currently inside the nonempty-impact branch, so a test-only traversal can hide that warning.
- `find_related_tests` checks extractor test flags in its direct, pending, name, and FTS queries. `test_path_predicate` and `is_test_path` already implement the shared path policy; blast radius uses that policy too.
- `Symbol.symbol_id` already exists. `find_references_for_symbol` already filters resolved edges by ID. `get_symbol_body_op` refreshes and reloads its target before slicing, but reloads by name today.
- `get_symbol_by_name_internal` cannot distinguish same-name, same-kind overloads in the same file and parent. Search/lookup JSON already includes IDs through `Symbol`; their text output does not.
- IDs select definitions. Pending edges and identifier fallbacks still use namespace/receiver/name heuristics; choosing an ID cannot supply parameter-type resolution absent from the extractor.

## Global constraints

- No MCP `workspace`, `workspace_id`, `repo_path`, or `root_dir` parameters. Preserve automatic binding and the decision in `docs/decisions/001-zero-workspace-parameters.md`.
- Keep exactly ten tools and a CLI equivalent for every behavior. Do not restore editing tools or add another discovery tool.
- No artifact schema migration, extractor change, FTS-rule change, search-rank change, or new dependency.
- Keep repository data in SQLite. New state is limited to the existing bounded results, one extra probe row per capped query, and scalar metadata. No retained repository graph or symbol registry.
- Retained memory is about 25 MB today. Measure the same live `serve` workload before and after implementation; investigate a persistent increase rather than inventing a new fixed budget.
- Normalize file paths through existing workspace helpers and `dunce::simplified`; output forward slashes. Treat symbol IDs as opaque, case-sensitive strings, never as paths or shortened hashes.
- Keep `AGENTS.md`/`CLAUDE.md`, both distributed code-kb skills, and both routing-block copies synchronized.
- Preserve native-edit freshness, Qt header refresh behavior, external-callee filtering, name-based selection, and current depth defaults/maxima.
- No new general pagination protocol, rename/refactoring feature, test runner, whole-file text index, release, version bump, push, or publication in this plan.
- Preserve unrelated regression fixtures and assertions. A sample symbol name is not an executable API and does not need renaming.

## Behavior contract

### 1. Truthful blast-radius limits

Add three booleans to `BlastRadiusResult`, alongside existing `traversal_ceiling_reached`:

| Field | Meaning |
|---|---|
| `likely_tests_truncated` | The discovered, deduplicated test list contained more rows than the requested output limit. |
| `impacted_symbols_truncated` | The discovered impact list contained more rows than the requested output limit. |
| `test_file_ceiling_reached` | At least one stem-matching query had more than ten candidate files. This does not assert that an additional unique test survives deduplication. |

Set the two output flags from list lengths before truncating. Probe 201 traversal rows, consume only the first 200, and set the traversal flag only when the extra row exists. Probe 11 distinct files per stem, consume only ten, and set the test-file flag when the extra row exists. Give the stem query deterministic path ordering. Do not expand either discovery bound or fetch an unbounded total.

Text headings say `N returned`, not `N found`. Separate notices explain requested-limit truncation, the 200-row traversal ceiling, the ten-files-per-stem ceiling, and the existing 20/50 text-display caps. Emit discovery notices even when one returned list is empty. Increasing `limit` can reveal discovered rows; it cannot raise either discovery ceiling. Offer narrowing the target for discovery ceilings and CLI JSON for rows hidden only by compact formatting.

Omitted `limit` remains 20. Explicit zero returns no list rows, retains the selected seeds and applicable flags, and must not claim that no tests/callers were discovered. Values above 200 remain rejected. All new metadata is present in CLI JSON; MCP text conveys the same meaning. Counts describe returned/discovered rows, never complete workspace coverage.

### 2. Consistent context test discovery

In every candidate source inside `find_related_tests`, admit rows where an extractor test flag is set **or** the row is a test location in a path matched by `test_path_predicate("s")`. For path-only admission, use blast traversal's existing exclusions: `import`, `variable`, `parameter`, `field`, `property`, `module`, and `namespace`. Otherwise name/FTS fallback could present a local variable or import as a related test. Preserve existing flagged rows. Reuse the path predicate and share the existing SQL kind list where needed; do not invent a new ranking or test taxonomy.

Keep current candidate priority, deduplication, documentation exclusion, and five-test context cap. Preserve namespace/receiver checks on pending relationships. A production path merely containing the word `test` must not become a test unless the shared predicate recognizes it. This change does not add new prediction heuristics, reasons, or execution commands.

### 3. Exact symbol selection

Lookup and search text include each result's complete `symbol_id` on its existing metadata line as `id=<symbol_id>`. Their JSON already carries the ID and must not acquire a duplicate ID field. Keep the current ranking and result counts.

| Existing tool | Existing CLI | Added selector |
|---|---|---|
| `get_symbol_body` | `code-kb body` | MCP `symbol_id`; CLI `--symbol-id` |
| `get_symbol_context` | `code-kb context` | MCP `symbol_id`; CLI `--symbol-id` |
| `find_references` | `code-kb refs` | MCP `symbol_id`; CLI `--symbol-id` |
| `blast_radius` | `code-kb blast-radius` / `impact` | MCP `symbol_id`; CLI `--symbol-id` |

- Body, context, and references require exactly one name or ID. Their positional CLI name becomes optional only when `--symbol-id` is supplied. Reject both, neither, and empty selectors with useful argument errors. Enforce this in MCP schemas, handlers, and CLI parsing.
- Blast radius allows one symbol name or ID, optionally with its file constraint; file-only and no-target git discovery stay intact. Reject a name plus ID. An empty ID is an error, not a request for git discovery.
- An optional file with an ID is a consistency check against the selected symbol's file. It must not turn into a second blast-radius seed. Resolve existing paths canonically and reject mismatches. For this new ID mode the check names an actual file; existing name-mode path behavior stays intact.
- Never pass IDs through name sanitization, prefix/fuzzy matching, or a name fallback. Unknown IDs report the bound workspace and direct the agent to run lookup/search again. Reuse central workspace/error-formatting logic rather than duplicating recovery strings in adapters.
- An ID is a selector for the current index, not a promised durable handle across edits or rebuilds. Resolve its owning file, apply the existing freshness check, then re-fetch the **same ID** before using offsets or seeding relationships. Missing IDs after refresh produce a reselection error. File guards must still match after refresh. Do not introduce generation tokens, caches, or a new ID registry.
- Resolved reference edges and blast seeds use the selected ID throughout. Preserve `include_external` on references/context. Pending and identifier-based references retain their existing heuristic behavior; document that exact target selection does not guarantee overload-perfect resolution of unresolved calls.
- Existing name and qualified-name workflows remain useful and supported. Do not add a second collection of ID-only tools or duplicate their implementations.

Proposed core shape: add an indexed `get_symbol_by_id` query using the existing symbol mapper; add a small `SymbolSelector::Name` / `SymbolSelector::Id` enum in `ops.rs`; factor the existing resolve/refresh/reload work into one operation helper returning a `Symbol`. Body and context reuse it. CLI/MCP references reuse it before `find_references_for_symbol`, which must accept/preserve `include_external`. Blast radius receives its explicit database path for freshness and passes already-resolved symbol IDs into the existing CTE implementation. Keep name-to-ID resolution outside that shared traversal body; do not resolve an ID-selected seed back through its name.

## Verification strategy

**Project sources:** `AGENTS.md`, `CLAUDE.md`, `.github/workflows/ci.yml`, README development commands. The base passed 374 Rust tests and 17 plugin tests; these are historical baseline counts, not a requirement to preserve an exact test count.

**Setup:** Use the pinned 3.3.1 extractor. On this machine, `JULIE_EXTRACT_BIN=/home/murphy/source/code-kb/.tools/julie-extract` is available. A shared `CARGO_TARGET_DIR=/home/murphy/source/code-kb/target` avoids recompiling dependencies; serialize cargo builds when sharing it.

**Worker red/green:** Write the behavioral regression first, run its exact test filter to see the expected failure, implement, and rerun that filter. The task commands below define the affected test targets. Workers may repair failures within their task; they report scope/design conflicts to the lead instead of weakening criteria.

**Worker ceiling:** Only the named core/CLI targets for the task and `cargo fmt --all -- --check`. No repeated workspace suites per small edit.

**Lead affected-change:** Review the owned diff, the contract matrix, and the worker's captured red/green output. Run additional affected targets only for uncovered interactions or a changed tree.

**Branch gate, once the combined code is ready:**

```sh
cargo test --workspace --locked --no-fail-fast
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
node --test tests/plugin/*.test.cjs
git diff --check
cmp AGENTS.md CLAUDE.md
cmp skills/code-kb/SKILL.md .claude-plugin/skills/code-kb/SKILL.md
cmp hooks/code-kb-routing-block.md crates/code-kb-cli/src/routing-block.md
```

Capture broad output once; repair and rerun exact failing tests, then repeat the failed gate on the changed tree. Check Linux and Windows path/ID parity through the repository's Windows workflow or the `win-test` guest before release. If Windows execution is unavailable locally, record that limit; do not claim Windows verification or block writing this plan.

**Memory/output evidence:** Compare live `serve` RSS before/after on the same index and a fixed repeated lookup/body/context/refs/blast sequence. Use `scripts/benchmark_quality.py`'s documented workload/options if its existing workload covers the comparison; inspect its CLI before invoking it. Report the added lookup/search output cost of full IDs. A sustained memory increase needs investigation; no new retained repository state is acceptable.

**Security scope:** None declared for separate scanners. Tests must still cover SQL-bound ID strings, invalid/conflicting selectors, path containment, and zero workspace parameters.

**Ledger:** Record scope, command, source commit, result, and timestamp. Separate passing assertions from report-only RSS/output measurements. Do not repeat checks on an unchanged tree to recover lost output.

## Execution and parallel-work contract

Use light, outcome-based task briefs and `serial-worker-commit`. Each task ends with focused verification, lead review, a checkpoint, and a local commit. The owner requested planning only in this turn; execute after approval of this plan. External reviewer choice is `none` unless the owner names one. Push/release authority has not been granted.

The implementation tasks share `queries.rs` and, for tasks 1/3, models, operations, and formatters. Serialize those edits. Independent read-only evidence gathering or review can run alongside implementation; do not run two writers on the shared files.

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| 1. Truthful limits | None | `models.rs`, `queries.rs`, `ops.rs`, `formatters.rs`, `blast_radius_test.rs`; CLI `cli_test.rs`, `mcp_test.rs` | Yes | Establish result metadata first; tasks 2/3 also modify the same query/operation code. |
| 2. Context tests | None | `queries.rs`, `tests/disambiguation_test.rs` | Yes | Shares `queries.rs` with task 1 and task 3. |
| 3. Exact selection | None | Core `queries.rs`, `ops.rs`, `lib.rs`, `formatters.rs`; CLI `main.rs`, `mcp/server.rs`; tests listed below; current guidance/decision files listed below | Yes | Builds on the new result metadata and shares core files; update guidance against the final contract. |

Core paths above are under `crates/code-kb-core/src/` unless marked `tests/`; `blast_radius_test.rs` is `crates/code-kb-core/tests/blast_radius_test.rs`. CLI source paths are under `crates/code-kb-cli/src/`, and CLI test paths under `crates/code-kb-cli/tests/`. Exact paths are repeated in each task.

### Task 1: Report every blast-radius cap accurately

**Files / ownership:** `crates/code-kb-core/src/{models,queries,ops,formatters}.rs`; `crates/code-kb-core/tests/blast_radius_test.rs`; `crates/code-kb-cli/tests/{cli_test,mcp_test}.rs`.

**Interfaces:** Consumes `BlastRadiusResult`, `compute_blast_radius_scoped`, `blast_radius_op`, and `format_blast_radius`. Produces the three new metadata fields and the limit/ceiling semantics defined above.

**Contract inputs:** Existing 200-row traversal, ten-files-per-stem discovery, 20/50 text caps, and public 0–200 output range. **Serialization:** Yes; shared core files. **Commit:** `serial-worker-commit`.

**Approach:** Add regression fixtures with one and two results, exactly 200 and 201 traversal rows, a saturated test-only walk, and eleven matching test files for one stem. Update every `BlastRadiusResult` constructor. Implement the probe rows and pre-truncation flags without changing result ordering or ranking. Fix the zero-limit remapping and text branches that otherwise claim no tests/callers exist. Verify identical semantics in CLI JSON and MCP text.

**Focused checks:** `cargo test -p code-kb-core blast_radius`; `cargo test -p code-kb-cli --test cli_test blast_radius`; `cargo test -p code-kb-cli --test mcp_test blast_radius`. Name new interface regressions with `blast_radius` so these filters include them.

**Acceptance criteria:**
- [x] A two-row result requested with limit one reports output truncation; an exact one-row result does not.
- [x] Omitted limit returns up to 20; explicit zero returns no rows with truthful metadata/text in both interfaces; 201 is rejected as an output limit.
- [x] Exactly 200 discovered traversal rows do not trigger the ceiling flag; a 201st row does, and it is not consumed into either list.
- [x] Test-only saturated walks show the traversal warning; per-stem saturation has its own accurate notice.
- [x] Compact text caps remain distinct from output/discovery caps; JSON retains the full returned lists and metadata.
- [x] Existing default ordering, depth behavior, file/git modes, and focused tests pass; commit follows lead review.

### Task 2: Reuse the shared test-path rule in context

**Files / ownership:** `crates/code-kb-core/src/queries.rs`; `crates/code-kb-core/tests/disambiguation_test.rs`.

**Interfaces:** Consumes `test_path_predicate`, `is_test_path`, `find_related_tests`, and `get_context_slice_op`. Produces the same `ContextSlice` shape with the corrected related-test set.

**Contract inputs:** Existing flag-or-path test policy, blast traversal's low-signal kind exclusions for path-only candidates, documentation exclusion, candidate priority, deduplication, and cap five. **Serialization:** Yes; shared `queries.rs`. **Commit:** `serial-worker-commit`.

**Approach:** Reuse `test_path_predicate("s")` in every direct, pending, name, and FTS candidate query, with the path-only kind guard specified above. Use existing SQLite fixtures to force extractor flags false; this avoids a test accidentally passing because the extractor recognizes the fixture. Include recognized `autotests/tst_*.qml` and `tests/` paths plus a production-path control from the existing path-policy cases. Put a matching function and matching local/import in the same recognized test file and prove only the function is admitted through path recognition. Keep real-extractor context integration coverage too.

**Focused checks:** `cargo test -p code-kb-core related_tests`; `cargo test -p code-kb-core context_slice`; `cargo test -p code-kb-core test_path_rule_and_its_sql_mirror_agree_on_every_path`.

**Acceptance criteria:**
- [x] Each candidate source can return an unflagged test in a recognized test path.
- [x] A matching production-path control, Markdown code example, and path-only local/import remain excluded; existing flag-based admission is unchanged.
- [x] Namespace disambiguation, uniqueness, ordering, and the five-test context cap are preserved.
- [x] Context and blast-radius fixtures agree on flag-or-path classification; focused tests pass and lead reviews before commit.

### Task 3: Carry exact identity from discovery through inspection and impact

**Files / ownership:**
- `crates/code-kb-core/src/{queries,ops,lib,formatters}.rs`.
- `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/src/mcp/server.rs`.
- `crates/code-kb-core/tests/{disambiguation_test,freshness_test,blast_radius_test,qt_cpp_test}.rs` and existing in-module query/formatter tests. Adjust other direct operation call sites only where the selector signature requires it; preserve their assertions.
- `crates/code-kb-cli/tests/{cli_test,mcp_test,adversarial_m2_server_test,adversarial_m3_server_test}.rs`.
- `README.md`, `AGENTS.md`, `CLAUDE.md`, `docs/toolkit-assessment.md`, `docs/site/index.html`, `skills/code-kb/SKILL.md`, `.claude-plugin/skills/code-kb/SKILL.md`, `hooks/code-kb-routing-block.md`, `crates/code-kb-cli/src/routing-block.md`.
- Create `docs/decisions/002-exact-symbol-selection.md` recording the chosen selector, freshness behavior, and unresolved-reference limitation.

**Interfaces:** Consumes `Symbol.symbol_id`, the existing body/context operations, `find_references_for_symbol`, scoped blast-radius queries, workspace normalization, and freshness helpers. Produces the four `symbol_id`/`--symbol-id` contracts, ID-bearing discovery text, and one shared target-resolution path. All newly proposed Rust helpers/types are implementation work, not claims that those interfaces already exist.

**Contract inputs:** The exact-selection rules above, task 1 metadata, task 2 test policy, and the existing name-mode behavior. **Serialization:** Yes; shared core and adapter files. **Commit:** `serial-worker-commit`.

**Approach:** Start with two same-name methods under the same parent/file and different IDs, bodies, and resolved callers/callees. Add indexed ID lookup and the shared resolve/refresh/reload helper; wire existing operations and adapters through it. Extend the existing ID-aware reference helper to preserve external filtering. Factor blast traversal to accept resolved seeds so the selected ID is never converted into an ambiguous name lookup. Add text IDs to lookup/search, then implement argument exclusivity and file consistency checks. Update current guidance and the decision record against the actual final contract.

**Focused checks:** `cargo test -p code-kb-core --test disambiguation_test --test freshness_test --test blast_radius_test --test qt_cpp_test`; `cargo test -p code-kb-core find_references_for_symbol`; `cargo test -p code-kb-cli --test cli_test --test mcp_test --test adversarial_m2_server_test --test adversarial_m3_server_test`. Run exact new regression filters red/green before these target-wide checks.

**Acceptance criteria:**
- [x] Lookup/search return complete reusable IDs without changing rank, count, or JSON identity layout.
- [x] Both same-parent overloads can be selected separately through body, context, references, and blast radius in CLI and MCP.
- [x] Resolved caller/callee edges and impact seeds remain isolated by ID; pending-edge behavior is covered and its heuristic limit is documented.
- [x] Matching file guards work, mismatches fail, and an ID plus file never expands impact to the whole file.
- [x] A deterministic resolver/helper regression checks the refreshed row against the normalized/absolute file guard and refuses a post-refresh mismatch before slicing or seeding. Do not rely on a timed filesystem race or add a production hook only for testing.
- [x] Missing, empty, and conflicting selectors fail consistently; IDs containing quotes are SQL-bound and cannot broaden selection.
- [x] A native edit that changes offsets either reloads the same surviving ID correctly or returns reselection guidance; a removed ID never falls back to a same-named sibling. Cover both a refresh-triggered disappearance and an already-missing ID.
- [x] Include-external, test symbols, qualified names, name/file selection, file-only impact, and zero-argument git discovery retain their behavior.
- [x] Windows/path-separator cases use the existing identity rules; IDs are not case-folded or path-normalized.
- [x] Ten tools remain, schemas expose no workspace parameters, all guidance pairs match, focused tests pass, and lead review precedes commit.

## Completion

The lead checks all acceptance criteria against the final diff, runs the combined branch gate, records the RSS/output comparison, and reports any unrun Windows scope. Checkpoint before committing and include the checkpoint. Reconcile this worktree with the existing unmerged removal commit; do not leave related work on another branch. Report local source state and verification. Publishing, release preparation, and unrelated cleanup require a separate user request.

## Verification record

Source commit: `74baf19`; all checks below ran on its clean task worktree on 2026-09-22 UTC. The final documentation and memory commit changes no executable code.

| Scope | Command or workload | Result |
| --- | --- | --- |
| Branch gate | `cargo test --workspace --locked --no-fail-fast` | 394 Rust tests passed |
| Branch gate | `cargo clippy --workspace --all-targets --locked -- -D warnings` | Passed |
| Branch gate | `cargo fmt --all -- --check`; `node --test tests/plugin/*.test.cjs`; `git diff --check`; three guidance-pair `cmp` checks | Passed; 17 plugin tests |
| Build | `cargo build -p code-kb-cli --bin code-kb --release` | Passed |
| Windows guest | Exact-ID SQLite test; `test_paths_equal`; `cargo check --workspace --locked` | Passed; seven path-identity tests. Build guard used `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1` because the guest lacks the pinned extractor. |
| Report-only memory | Same persistent MCP benchmark workload and final index, old vs new binary | Post-query RSS 16.47 vs 16.50 MB; no material retained increase. |
| Report-only output | Same lookup and search benchmark queries, old vs new binary | Lookup 443 to 512 estimated tokens; search 2,109 to 2,486, reflecting full IDs. |

The benchmark used a temporary copy of `scripts/benchmark_quality.py` with its removed `validate_syntax` target changed to the existing `sanitize_fts5_query` symbol. The repository script was not edited. Extractor-backed Windows integration was not run and remains part of release verification.
