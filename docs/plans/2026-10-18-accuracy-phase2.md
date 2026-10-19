# Accuracy Phase 2 Implementation Plan

> **For agentic workers:** Use razorback:subagent-driven-development for independent tasks, or tasks whose separate context has a clear benefit, when delegation is available and permitted. Otherwise use razorback:executing-plans: the current agent runs the plan.

**Goal:** An agent gets every site a change must reach in one `find_references` call (sites, overrides, constructions, variant uses, registrations, and the tests to run with their commands), and a check after the edit lists the sites the diff did not reach. The tool list shrinks to the six tools that serve that job.

**Architecture:** Four slices, in order. Each slice ends with the branch building, the tests green, and the reach gate at PASS.
1. Tool surface: remove four MCP tools, make `lookup_symbol` the one search tool, and drop the word-guess test rows of `blast_radius`.
2. Reach sections: new sections in the callers report, computed in `code-kb-core` from rows the index already holds.
3. Arity: julie-extract writes the argument count of each call; code-kb reads parameter rows for the definition side, shows `N args` per site, and uses a count that no definition of the target accepts as exclusion evidence.
4. Post-edit check: a new `check_edit` tool, `code-kb check`, and Stop and SubagentStop hooks that compare the symbols of a base revision with the working tree and list the sites outside the diff.

**Tech Stack:** Rust 2024 (rusqlite, serde_json), SQLite schema v7, julie-extract 3.8.0 (slice 3), git CLI, Python 3 standard library (gate scripts).

**Spec:** `docs/plans/2026-10-17-accuracy-base-design.md` ("Phase 2"), with the detail from `~/.code-kb/reports/2026-10-17-direction-research/designs.txt` (tools `find_references`, `check_edit`, `lookup_symbol`, `file_skeleton`; lists DROP and EXTRACTOR). Where this plan and the research differ, this plan binds.

**Architecture Quality:**
- New module `crates/code-kb-core/src/reference_report/sections.rs`: one entry point that takes the selected target and the labeled sites and returns the new report sections. No state.
- New module `crates/code-kb-core/src/test_commands.rs`: maps a test symbol to a run command from marker files under the project root. No state.
- New module `crates/code-kb-core/src/edit_check.rs`: base extraction into a temp index, shape compare, and site filter.
- Main risk: the post-edit hook runs on every stop. It must stay silent and fast when no shape changed (bounded by a 20 s budget and at most 10 changed symbols).

**Authority ledger:**
- `local_commit_authority: authorized — owner: "Ok let's do it. You are approved to work in both repos" (2026-10-17); "merge it and keep going" (2026-10-18)`
- `push_authority: missing for code-kb` — the owner's 2026-10-18 merge approval covered the JS/TS branch only. Ask before the phase 2 merge.
- `pr_authority: missing for code-kb` (same ask).
- julie-extractors release (tag, GitHub release, crates.io): authorized — owner 2026-10-18 "do a julie-extractors release if you need to". Slice 3 needs it.
- code-kb release 2.7.0: missing. It waits for the owner after phase 2.
- Plan approval: the owner told the lead to keep going without a second approval round (2026-10-18). This plan records the choices below as lead decisions.
- Reviewer choice: none (Sonnet review agents inside the workflows, lead validation).

**Lead decisions (consequential, recorded):**
- The MCP tools `search_symbols`, `codebase_outline`, `find_structural_facts`, and `telemetry_summary` are removed. Their CLI commands `search`, `outline`, `facts`, and `stats` stay. The design doc lists these removals; the owner approved the direction.
- `find_references` keeps `direction=callees`. The design doc does not list it.
- `blast_radius` stays a tool. It loses only its word-guess rows (stem- and module-matched files, shared words, `possible` rows, handler evidence).
- Overrides come from `extends` and `implements` rows in code-kb. julie writes no method-level edges in this phase.
- The definition side of arity comes from julie's parameter rows (`metadata_json.role = "parameter"`), which exist in every gate language. julie adds only the call-site count.

## Global Constraints

- Core Invariant 1: every remaining MCP tool requires `project_root`. No schema names `workspace`, `workspace_id`, `repo_path`, or `root_dir`.
- CLI 1:1: every MCP tool has a CLI command that prints the same text. New: `check_edit` ↔ `code-kb check`.
- Paths in text and JSON use `/` on every platform.
- No repository data held in RAM across calls.
- `AGENTS.md` and `CLAUDE.md` stay byte-identical. Both routing-block copies (`hooks/code-kb-routing-block.md`, `crates/code-kb-cli/src/routing-block.md`) stay byte-identical, and under 3 KB.
- No new crate dependency.
- Labels and basis strings of phase 1 do not change, except the new arity basis in slice 3.
- Reach gate marks hold after every slice: proven precision >= 0.98 and excluded precision >= 0.98 and accounted share >= 0.98 in every language. `lost_proven.py` prints nothing for the 7 gate projects against the previous slice's build.
- Tune only on dev symbols. Never look at holdout site details. Never run `dump_sites.py`.
- Live `serve` memory and `refs` latency are measured before and after slices 2, 3, and 4 (`~/.cache/code-kb-accuracy/bench/refs_latency_js.py`).
- Heavy output goes under `~/.cache/code-kb-accuracy/phase2/`, never `/tmp`. `TMPDIR=/home/murphy/.cache/julie-tmp`.

---

## Verification Strategy

**Project source of truth:** `CLAUDE.md`, `.github/workflows/ci.yml` (code-kb); `CLAUDE.md`, `docs/release.md`, `.github/workflows/ci.yml` (julie-extractors).

**Worker red/green scope:** `cargo test -p code-kb-core --test <file>`, `cargo test -p code-kb-core --lib <module>`, `cargo test -p code-kb-cli --test <file>`; plugin tests `node --test tests/plugin/*.test.cjs`; julie `cargo test -p julie-extractors <filter>`.

**Worker ceiling:** the test binaries of the crate a task changes, plus `cargo clippy -p <crate> --all-targets -- -D warnings` and `cargo fmt --all -- --check`. Workers never run the reach gate or release scripts.

**Lead affected-change scope:** after each task: `CI=1 cargo nextest run --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`.

**Branch gate (end of each slice):** `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `CI=1 cargo nextest run --workspace`, `node --test tests/plugin/*.test.cjs`, `cmp AGENTS.md CLAUDE.md`, `python3 scripts/reach_gate.py run` PASS, `lost_proven.py` empty for 7 repos. julie (slice 3): `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features --no-deps -- -D warnings`, `scripts/check-agent-doc-sync.sh`, `node scripts/language-data-quality-report.mjs --strict`, `cargo xtask test default`, `cargo xtask test contract`, `cargo xtask compat-check`.

**Security scope:** julie: `cargo deny check --all-features` (CI). code-kb: none declared.

## Parallel Execution Contract

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| 1.1 Remove four MCP tools | S1 | `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/tests/*.rs` | Yes | 1.2 edits the same handler. |
| 1.2 One search tool | S1 | `server.rs`, `main.rs`, `formatters.rs` (lookup part), `queries.rs` (lookup part), tests | Yes | After 1.1. |
| 1.3 Drop word-guess tests | S1 | `queries.rs` (blast radius), `formatters.rs` (blast radius), `models.rs`, tests | Yes | Shares `queries.rs` and `formatters.rs` with 1.2. |
| 1.4 Docs and guidance | S1 | `CLAUDE.md`, `AGENTS.md`, `README.md`, routing blocks, `skills/`, `docs/site/`, `evals/`, `scripts/` | Yes | Describes 1.1-1.3. |
| 2.1 Overrides | S2 | `reference_report/sections.rs` (new), `reference_report.rs`, `models.rs`, `formatters.rs`, tests | Yes | Creates the sections module. |
| 2.2 Constructions and variant uses | S2 | same | Yes | After 2.1. |
| 2.3 Registrations | S2 | same | Yes | After 2.2. |
| 2.4 Tests with commands | S2 | `test_commands.rs` (new), `sections.rs`, tests | Yes | After 2.3. |
| 2.5 Docs | S2 | `CLAUDE.md`, `AGENTS.md`, `README.md`, `skills/code-kb/SKILL.md` | Yes | Describes 2.1-2.4. |
| 3.1 julie call arg counts | S3 | julie-extractors `crates/julie-extractors/src/base/*`, tests, contract docs | No | Independent of code-kb tasks. |
| 3.2 julie 3.8.0 release | S3 | julie version files, release notes | Yes | After 3.1 and its dogfood round. |
| 3.3 code-kb arity | S3 | `reference_report.rs`, `models.rs`, `formatters.rs`, pins, tests | Yes | Needs 3.2. |
| 4.1 Edit check core | S4 | `edit_check.rs` (new), `ops.rs`, tests | Yes | Needs slices 2 and 3. |
| 4.2 Tool, CLI, and hooks | S4 | `server.rs`, `main.rs`, `hooks/*.json`, `tests/plugin/*`, tests | Yes | After 4.1. |
| 4.3 Docs and release prep | S4 | docs | Yes | Last. |

Commit mode: `serial-worker-commit` is off. Workers hand the verified diff to the lead; the lead reviews, runs the lead scope, saves a goldfish checkpoint, and commits (`parallel-lead-commit`).

---

## Slice 1: Tool surface

### Task 1.1: Remove four MCP tools

**Files:** `crates/code-kb-cli/src/mcp/server.rs`; tests in `crates/code-kb-cli/tests/mcp_test.rs`, `adversarial_m2_server_test.rs`, `adversarial_m3_server_test.rs`, `cli_test.rs`.
**Interfaces:** `tools/list` returns exactly `file_skeleton`, `lookup_symbol`, `get_symbol_body`, `get_symbol_context`, `find_references`, `blast_radius`. A call to a removed name returns the existing `Unknown tool` error.
**Contract inputs:** the tool-surface map (`~/.cache/code-kb-accuracy/phase2/maps/tool-surface.md`, sections 1, 2, 5).
**File ownership:** as in the table.
**Serialization required:** Yes. **Dependency reason:** 1.2 edits the same handler.

**What to build:** Delete the four tool definitions and handlers, the `telemetry_summary` exceptions (no `project_root`, no `alwaysLoad`, dispatch before the index wait), and the imports that become unused. `file_skeleton` on a folder keeps calling `codebase_outline_op`. The `initialize` instructions text loses the two names: it becomes, exactly: `Pass project_root, the absolute path of the project or git worktree you work in, on every call. Call file_skeleton before you read a source file, then get_symbol_body or get_symbol_context for the symbols you need. Use lookup_symbol for a name or a concept and find_references instead of grep on a name, and blast_radius to pick the tests to run.`

- [x] `tools/list` has 6 tools, each with `project_root` required and `alwaysLoad`.
- [x] Tests that used a removed tool as a probe use a remaining tool; tests that read telemetry through `telemetry_summary` read it with `code-kb stats --json` or SQLite.
- [x] `test_cli_search_order_matches_mcp_search_symbols` is replaced by a test that `code-kb search` and the MCP `lookup_symbol` fallback list the same ranked rows for a concept query.
- [x] Worker scope green; diff handed to the lead.

### Task 1.2: One search tool

**Files:** `server.rs` (lookup handler), `crates/code-kb-cli/src/main.rs` (lookup arm), `crates/code-kb-core/src/formatters.rs` (`format_find_symbol_results`), `crates/code-kb-core/src/queries.rs` (only if a helper is needed), tests.
**Interfaces:** `lookup_symbol` description becomes, exactly: `Find symbols by name, qualified name ('Type::method'), prefix, or concept; returns kind, path, line, signature, and id. A query that names no symbol exactly also gets the ranked matches by name words, signatures, and docs. Use instead of grep on a name.` The `query` property description becomes, exactly: `Identifier, qualified name, prefix, or a few words that describe the code.`
**Contract inputs:** tool-surface map section 4.
**File ownership:** as in the table.
**Serialization required:** Yes. **Dependency reason:** after 1.1.

**What to build:** When the query is a plain query (not a qualified name and not a member listing ending in `.` or `::`) and no listed row has exactly the query's name, the answer runs the ranked search (`fts_search_symbols_scoped`, same filters and limit) and appends the ranked rows that the name rows do not already show, under the existing header `No exact name match; N full-text matches for "q":`. When both lists are empty, the answer is the `symbol_not_found_parts` text (bound workspace, `Did you mean one of:` or `No similar name is indexed; check the workspace and spelling.`). The CLI `lookup` prints the same text. `code-kb search` stays as it is.

- [x] A one-word concept that some name contains (`retry` in a fixture with `retry_count` and a function documented "retry with backoff") lists the name row and the ranked row.
- [x] An exact-name query prints no ranked section.
- [x] A total miss prints the suggestions text.
- [x] MCP and CLI text are equal for the three cases.
- [x] Worker scope green; diff handed to the lead.

### Task 1.3: Drop the word-guess test rows

**Files:** `queries.rs` (`compute_blast_radius_scoped_with_ids` and helpers), `formatters.rs` (`format_blast_radius`), `models.rs` (`test_file_ceiling_reached`), tests in `crates/code-kb-core/tests/blast_radius_test.rs`, `references_test.rs`, `crates/code-kb-cli/tests/cli_test.rs`, unit tests in `queries.rs` and `formatters.rs`.
**Interfaces:** `blast_radius` lists only tests that the call graph reaches: direct and indirect callers, fixture users, setup members' class tests, and the constructor step. The JSON field `test_file_ceiling_reached` is removed. When no test is found the text says, exactly: `No test reaches it through the index. Run the full test suite.`
**Contract inputs:** the blast-radius map (`maps/blast-radius.md`), rules A-D stay, rules E-K (word guesses) go.
**File ownership:** as in the table.
**Serialization required:** Yes. **Dependency reason:** shares files with 1.2.

**What to build:** Delete the stem-matched and module-matched file rows, shared seed words, `possible` rows, handler evidence, and the whole-file row merge, with every helper only they use. Keep `fixture_users`, `tests_in_class`, `names_a_test`, `is_test_path`, and `find_related_tests` unchanged. Tests that asserted the removed rows are deleted or rewritten to assert their absence; tests of rules A-D keep their meaning.

- [x] A Flask-shaped fixture where a test reaches the target only through `__call__` lists no `possible` row.
- [x] A file seed whose test file only shares the stem lists no `stem-matched test file` row.
- [x] Fixture, setup, and constructor rows still appear in their existing tests.
- [x] Worker scope green; diff handed to the lead.

### Task 1.4: Docs and guidance

**Files:** `CLAUDE.md`, `AGENTS.md`, `README.md`, `hooks/code-kb-routing-block.md`, `crates/code-kb-cli/src/routing-block.md`, `skills/code-kb/SKILL.md`, `skills/code-kb-telemetry/SKILL.md`, `docs/site/index.html`, `docs/toolkit-assessment.md`, `docs/decisions/001-zero-workspace-parameters.md`, `scripts/release-dogfood-prompt.md`, `scripts/benchmark_quality.py`, `scripts/retrieval_score_tasks.json`, `scripts/retrieval_score.py`, `evals/**` that name a removed tool, `evals/mocks/code-kb/*`.
**Interfaces:** the routing block line for concepts becomes, exactly: `- **To find code by concept**, call \`lookup_symbol\` with a few words; it adds ranked matches when no name matches.` and the first line drops `except \`telemetry_summary\``.
**Contract inputs:** tool-surface map section 3 (every place that names a removed tool). History under `docs/reviews`, `docs/plans`, `docs/release-notes`, and `docs/post-v1.0-audit-report.md` stays unchanged.
**File ownership:** as in the table.
**Serialization required:** Yes. **Dependency reason:** describes 1.1-1.3.

- [x] `rg -n 'search_symbols|codebase_outline|find_structural_facts|telemetry_summary'` outside history files names only CLI commands or the removal note.
- [x] Invariant 1 says every MCP tool requires `project_root`; invariant 5 and the blast-radius paragraph describe the new behavior; the alwaysLoad paragraph has no exception.
- [x] The telemetry skill and its four evals use `code-kb stats`.
- [x] `cmp AGENTS.md CLAUDE.md` and the routing-block sync test pass; the routing block stays under 3 KB.
- [x] Worker scope green; diff handed to the lead.

**Slice 1 gate:** branch gate; the reach gate report is identical to the previous one except the Commit and Generated lines.

---

## Slice 2: Reach sections

All sections print after the site groups and before the text-only lines, only when they have rows, and are counted in full (`limit` caps listed rows per section, with one fold line). CLI `--json` adds the arrays `overrides`, `overridden`, `constructions`, `variant_uses`, `registrations`, `tests` (each omitted when empty). The MCP and CLI text stay equal.

### Task 2.1: Overrides

**Files:** `crates/code-kb-core/src/reference_report/sections.rs` (new), `reference_report.rs` (call it), `models.rs`, `formatters.rs` (`format_reference_report`), tests in `crates/code-kb-core/tests/reference_report_test.rs`.
**Interfaces:** for a callable target whose parent is a class, struct, record, interface, or trait:
- `Overrides ({n}): calls through \`{Owner}.{name}\` can reach these` with rows `- \`{Sub}.{name}\` ({path}:{line})`: the same-named callable children of every type that extends or implements the owner, transitively (depth 8, at most 50 types), through `relationships` and through pending `extends`/`implements` rows resolved by name with the nearest-class rule that `class_chain_sql` uses.
- `Overrides of ({n}): calls to these can reach \`{Owner}.{name}\`` with rows `- \`{Base}.{name}\` ({path}:{line})`: the same-named callable children of the owner's ancestors, nearest first, at most 5.
- The `cannot_see` line about dynamic dispatch is kept only for Go targets (implicit interfaces) and is reworded to, exactly: `calls through a Go interface that the index does not record reach methods this list does not show`.
**Contract inputs:** reach-sections map section 2 (class chain helpers, `idx_symbols_parent`, Rust impl methods linked by `link_impl_owners`).
**File ownership:** as in the table.
**Serialization required:** Yes. **Dependency reason:** creates the module.

- [x] Python, TypeScript, Java, C#, and Rust (trait impl) fixtures list the overrides and the base method.
- [x] A subclass in another file that is shadowed by a nearer same-named class is not counted.
- [x] A target with no owner prints neither section.
- [x] Worker scope green; diff handed to the lead.

### Task 2.2: Constructions and variant uses

**Files:** `sections.rs`, `models.rs`, `formatters.rs`, tests.
**Interfaces:**
- For a class, struct, or record target: `Constructions ({n}): {path}: {line}, {line} · {path}: {line}` (lines folded per file, files in site order). A construction is a site of the target's name whose row is a call (`calls`, `call`, `instantiates`), a site the constructor step labels `builds the class`, or, in Rust and Go, a `type_usage` site whose source text after the name (after optional generic arguments `<…>`, `::<…>`, or `[…]`) starts with `{`, unless the token before the name is `->`, `impl`, `for`, `struct`, `enum`, `union`, `trait`, `type`, or `dyn`. Excluded sites never count.
- For an enum target: `Variant uses ({n} functions): a match in them may need a new arm` with rows ``- `{caller}` [{path}:{line}] names {k} of {m} variants``, sorted by `k` descending, then path. A variant use is an identifier whose name is a child `enum_member` of the target and that sits on the same line as a non-excluded site of the enum's name, or a `member_access` whose receiver is the enum's name.
**Contract inputs:** reach-sections map sections 2-3 (row kinds julie writes; Rust `Enum::Variant` writes a `variable_ref` for the enum and a `type_usage` for the variant).
**File ownership:** as in the table.
**Serialization required:** Yes. **Dependency reason:** after 2.1.

- [x] Rust fixtures: `Foo { a: 1 }`, `&Foo { .. }`, `Foo::<T> { .. }` count; `impl Foo {`, `fn f() -> Foo {`, `impl T for Foo {` do not.
- [x] Python `Foo()`, Java `new Foo()`, TypeScript `new Foo()`, Go `Foo{}` count.
- [x] A Rust enum with a `match` on 3 of 3 variants and a function naming 1 variant prints both rows in that order.
- [x] Worker scope green; diff handed to the lead.

### Task 2.3: Registrations

**Files:** `sections.rs`, `models.rs`, `formatters.rs`, tests.
**Interfaces:** `Registrations ({n}):` with rows `- {what} [{path}:{line}]{by_name}` where `{what}` is `route {VERB} {template}` for a route fact (verb from `verb`, template from `effective_route_template`, else `route_template`, else the pattern id) and the pattern id otherwise, and `{by_name}` is ` (by name)` for a name match. A registration is a structural fact in a route family (`ROUTE_FAMILIES`) whose `containing_symbol_id` is the target and whose `api_style` is `decorator_routing`, or whose metadata key `view_target`, `handler`, `handler_name`, `controller_action`, `view_name`, or `function_name` equals the target's name, or whose `handler_names` array holds it.
**Contract inputs:** reach-sections map section 2 ("Structural facts").
**File ownership:** as in the table.
**Serialization required:** Yes. **Dependency reason:** after 2.2.

- [x] A Flask fixture with `@app.route("/x")` on the target and `app.add_url_rule("/y", view_func=target)` lists both rows, the second `(by name)`.
- [x] A function with the same name in another file gets the `(by name)` row only.
- [x] Worker scope green; diff handed to the lead.

### Task 2.4: Tests with commands

**Files:** `crates/code-kb-core/src/test_commands.rs` (new), `crates/code-kb-core/src/lib.rs`, `sections.rs`, `models.rs`, `formatters.rs`, tests.
**Interfaces:** `Tests that call it ({n}):` with rows ``- `{test}` [{path}:{line}] run: {command}`` (or without ` run: …` when no command is known). A test is the caller of a `proven` or `likely` site that is a test (`is_test`, or `names_a_test` in a test path), or the test that encloses a helper caller (`enclosing_test`); setup, teardown, and fixture members are not tests. At most `limit` rows, one fold line. Commands, exactly:
- Python: `pytest {path}::{Class}::{name}` (`::{Class}` for every class around the test); `cd '{dir}' && pytest {file}::{Class}::{name}` when the folder holds `[` or `::`; no command when pytest would not collect the test or the file name holds `[` or `::`.
- Rust: `cargo test -p {package} --test {stem} {name}` for a file directly under a package's `tests/`, else `cargo test -p {package} {name}`; `{package}` is `[package] name` of the nearest `Cargo.toml` above the file; without one, `cargo test {name}`. A test with `#[ignore]` ends with ` -- --ignored`.
- Go: `go test ./{dir} -run '^{name}$'`, `{dir}` relative to the nearest `go.mod` folder, run from that folder (`.` for the folder itself), for `Test`, `Fuzz`, and `Example` functions; `go test ./{dir} -bench '^{name}$' -run '^$'` for `Benchmark` functions; no command for a method, `TestMain`, a helper, or an `Example` without an output comment.
- JavaScript and TypeScript, by the nearest `package.json` (`devDependencies`, `dependencies`, `scripts.test`): vitest `npx vitest run {path} -t "{name}"`; jest `npx jest --runTestsByPath {path} -t "{name}"`; mocha `npx mocha {path} --fgrep "{name}"`; `node --test` in `scripts.test` → `node --test-name-pattern "^{name}$" {path}`; otherwise no command. `{path}` is relative to that `package.json` folder.
- Java: `mvn test -Dtest={Class}#{name}` when the nearest build file is `pom.xml`; `./gradlew test --tests '{Class}.{name}'` (or `gradle` without a `gradlew`) for `build.gradle` or `build.gradle.kts`. Both start with `cd {dir} && ` when the build file is below the root; the wrapper path is relative to that folder.
- C#: `dotnet test --filter "FullyQualifiedName~{Class}.{name}"`.
- Other languages: no command.
Double quotes inside `{name}` are escaped with `\`.
**Contract inputs:** reach-sections map section 4 (helpers `qualify_test_methods`, `enclosing_test`, `names_a_test`, `is_test_path`, symbol flags `is_test`, `test_lifecycle`).
**File ownership:** as in the table.
**Serialization required:** Yes. **Dependency reason:** after 2.3.

- [x] One fixture per language rule above, with its exact command.
- [x] A helper defined inside a test maps to that test; a fixture caller is not listed.
- [x] Worker scope green; diff handed to the lead.

### Task 2.5: Docs

**Files:** `CLAUDE.md`, `AGENTS.md` (the "Callers report" paragraph), `README.md` (the find_references part), `skills/code-kb/SKILL.md`.
- [x] The sections, their rules, and the JSON arrays are described in the same style as the phase 1 text.
- [x] `cmp AGENTS.md CLAUDE.md` passes.

**Deviations from the plan text:**
- Variant uses strings pluralize: `Variant uses (1 function)` and `names 1 of 1 variant`.
- Shell quoting of a test name escapes `\`, `$`, and a backtick besides `"`. A JavaScript or TypeScript name is first decoded from its source literal (`\'` becomes `'`, `\\` becomes `\`, `\n` and `\t` become control characters), because julie gives the literal text. For vitest and jest the decoded name is then escaped as a regular expression (`\ ^ $ . * + ? ( ) [ ] { } |`), because `-t` takes a regular expression, and each `test.each` placeholder (`%s %d %i %f %j %o %O %p %#`, `$name`) becomes `.*`; mocha `--fgrep` matches a plain substring, so it is not escaped, and keeps its placeholders.
- Rust patterns nested in a group (`Some(Foo { a, .. }) = o`, `Outer { inner: Foo { a, .. } } = o`) are not constructions; the scan climbs out of the groups and reads what follows the outermost one.
- A variant name that is not a member access counts only when `Enum::` or `Enum.` stands before it on the line of a site of the enum. The exception is a bare enumerator of an unscoped C or C++ enum (not `enum class` or `enum struct`), which counts as a variable read on that line. julie stores the enumerators of a C enum as `constant` rows, so a C enum takes its `constant` children as variants.
- A Rust member counts as an override only in the `impl Trait for Type` block of the owner trait; inherent methods, the impl methods of other traits, and subtrait declarations do not.
- A pending `extends` or `implements` row counts only when its type is in the same language family as the owner, and an import that reaches other definitions and not the owner drops it.
- Registrations by name match the last dotted segment of a handler value (`views.cart_view` names `cart_view`), for every handler key and for each `handler_names` entry, and apply to a target of any kind. A by-name row is always kept when a `proven` or `likely` site of the target is on the lines of the fact. Otherwise it is dropped when every site of the report on those lines is `excluded`, or when the target is in another file and the file of the fact defines a function, class, or variable of that name at module level or nested in a function; a member of a class or another type and an import never drop a row.
- Registrations by id accept `decorator_routing`, `attribute_routing`, `annotation_routing`, and `attribute` (actix) facts whose containing symbol is the target. The facts read include Spring `.request_mapping.` and Django `.url_pattern.`, which `ROUTE_FAMILIES` does not name.
- Every path in a section is read with `\` replaced by `/`.
- The full rules of the sections live in `docs/reach-sections.md`; `CLAUDE.md` and `AGENTS.md` keep a short summary, because every session in this repository loads them.
- A Go function inside another function (a `t.Run` closure) is a helper and folds into the test around it, as a Python function inside a function does. A Python class defined inside a function folds into that function.
- Go commands: `Benchmark` functions run with `-bench '^name$' -run '^$'`. A method, a non-`_test.go` file, `TestMain`, a name with a lowercase letter after the prefix, and an `Example` with no `// Output:` or `// Unordered output:` comment get no command, because `go test` runs none of them and exits 0.
- Maven and Gradle commands run from the folder of the nearest `pom.xml`, `build.gradle`, or `build.gradle.kts`, with `cd dir && ` and a relative wrapper path.
- A Python test in a folder with `[` or `::` runs as `cd dir && pytest file::name`, because pytest rejects such a path in a node id and `-k` does not help. A file name with either has no command.
- A Python test pytest does not collect under its defaults (`python_classes = Test*`; a config is not read) has no command: every class around it must be named `Test…` or derive from `TestCase`, and no class around another may be a `TestCase`.
- A Rust test with `#[ignore]` ends with ` -- --ignored`, because cargo runs 0 tests and exits 0 otherwise. `cfg_attr(…, ignore)` is not read.
- A Rust override check treats a `use` row whose first segment is a dependency of the nearest `Cargo.toml` like a std path (`use serde::Serialize;` names no project trait), unless a project module, folder, file, package, or crate has that name.
- `node --test-name-pattern` takes a regular expression, so the node command escapes the name like vitest and jest and anchors it with `^` and `$`. A test row in text output writes control characters of a test name as escapes; JSON and site rows are unchanged.

**Slice 2 gate:** branch gate; gate report identical except Commit and Generated; latency and memory measured against slice 1 (no p50 regression over 15% on the bench symbols without a stated cause).
- Result (2026-10-19): fmt, clippy, 1073 tests, plugin tests, and `cmp` pass; the reach gate report equals slice 1 except Commit, Generated, and Binary; `lost_proven` prints nothing for the 7 repos. Live servers, slice 1 against slice 2 (`refs_latency_js.py`, 5 rounds): p50 within +6% on all 10 bench symbols, RSS after calls within 3 MB. A unary plus on `i.name` in the judge SQL (fix round 3) cut the CLI peak RSS of gson `TypeAdapter.write` by 4 MB but cost zod `parse` 27% p50; the live server showed no memory gain, so slice 2 keeps the name index.

---

## Slice 3: Arity

### Task 3.1: julie call argument counts

**Repo:** `/home/murphy/source/julie-extractors/.worktrees/accuracy-base` (branch `feat/accuracy-base`, version `3.7.3` unreleased, becomes `3.8.0`).
**Files:** a new post-pass next to `crates/julie-extractors/src/base/receiver_metadata.rs` (same calling point in `registry.rs`), its tests, `docs/contracts/extraction-output-changes.md` (`## 3.8.0`, `classification: compatible`), the contract marker in `crates/julie-extractors/src/lib.rs` and `tests/api_surface.rs`, goldens that change.
**Interfaces:** every `call` identifier in Python, JavaScript, TypeScript, TSX, Java, C#, Go, Rust, Kotlin, C, and C++ gets `metadata_json.arg_count` (integer: the named, non-comment children of the call's argument list) and `metadata_json.arg_spread: true` when one argument is a spread or splat (`*x`, `**x`, `...x`, Go `x...`). A macro call (`foo!(…)`) and a call with no argument list node get no key. No schema change.
**Contract inputs:** julie-arity map sections 1-4.
- [x] Tests per language: 0, 1, and 3 arguments, a keyword argument (Python, Kotlin, C# named), a spread, a nested call (each call counts its own list), a constructor call (`new Foo(a)`).
- [x] `cargo xtask test default`, `contract`, and `compat-check` pass; clippy as in julie CI.

### Task 3.2: julie 3.8.0 release

Follow `docs/release.md` in julie-extractors: version 3.8.0 in the three crates, release notes `docs/release-notes/v3.8.0.md` (Rust type-fact fix from 3.7.3, call argument counts), ledger, preflight, the code-kb dogfood round with the 3.8.0 binary (no `[open]` defect), merge to julie `main` through CI, tag `v3.8.0`, Release Binaries workflow, `scripts/check-release-state.sh`, crates.io publish as the release doc says.
- [x] Release state check passes; the six assets exist with `.sha256` files.

### Task 3.3: code-kb arity

**Files:** `scripts/julie-pins.json`, `crates/code-kb-core/src/sync.rs` (`PINNED_JULIE_VERSION`), `reference_report.rs`, `models.rs`, `formatters.rs`, `CLAUDE.md`/`AGENTS.md` (invariant 7 version and the rule), tests.
**Interfaces:**
- `LabeledSite` gains `arg_count: Option<u32>` (JSON `arg_count`, omitted when unknown); the text row adds `; {n} args` (`1 arg`) after the basis.
- The target's accepted range comes from its parameter rows (`metadata_json.role = "parameter"`, children of the target): `min` counts rows with no default (`=`) and no variadic marker; `max` is the row count, or none when a row is variadic (`*`, `**`, `...`, `params `, a Java `...` type); a first `self` or `cls` row in Python and Rust `self` rows do not count. Overloads union their ranges.
- New exclusion evidence, only for Python, Java, C#, Go, Rust, Kotlin, and TypeScript sites that are `likely` or `unresolved` and have no spread: an argument count outside the target's range is `excluded` with basis, exactly, `{n} args; \`{name}\` takes {range}` (`{range}` is `{min}`, `{min}-{max}`, or `{min} or more`). A `proven` site never changes label. JavaScript sites never use the rule.
- [x] Fixtures: a Python call with 3 args to a 1-2 parameter function is excluded; a `*args` call is not; a Java overload pair accepts both counts.
- [x] Reach gate PASS; excluded precision >= 0.98 in every language; `lost_proven.py` empty. If the rule lowers excluded precision on dev, the rule is narrowed on dev symbols only, or removed.

**Deviations from the plan text:**
- A spread site carries no `arg_count` (julie gives no real count for it), so it is never excluded and its text row shows no count.
- A call row gets its count by pairing: the identifier alone on its line. A pending or relationship row with no identifier of its own (`new Foo(a)`, `instantiates`) takes the count of the one call identifier on the same path and line.
- The range comes from the signature text cross-checked against the parameter rows, because the C# and Kotlin rows hide defaults and variadic markers. A count or name mismatch gives no range. Parameter rows after the body start (lambda parameters) are ignored.
- A method with a receiver accepts one more argument (`Class.method(obj, a)` in Python, `Type::method(x, a)` in Rust, a C# extension method, a Go method expression), and `takes` shows the bound range. A Python module function whose first parameter is `self` accepts both forms. A Python decorator outside `staticmethod`, `classmethod`, `abstractmethod`, `override`, `final`, and `overload` gives no range.
- A Go site with one argument is never excluded for a target that needs more, because `f(g())` can pass several values.
- Kotlin keeps no trailing-lambda tolerance, because julie already counts the trailing lambda as one argument.
- Constructors: a Python and TypeScript constructor has a range. A C# constructor has one only for a `class` owner with no primary constructor; a struct, a record, and a class with a primary constructor have none. Every Java and Kotlin constructor has none, because julie records no identifier for a Java `new` and the Kotlin primary constructor is no symbol.
- A TypeScript property whose signature comes from an arrow initializer has no range; the source header must read `name(` or `name<`.
- The site language must equal the target language (a TSX site counts as TypeScript). Classes, fields, properties, and variables get no range.
- The basis says `1 arg` for one argument. The text row does not repeat the count when the basis already starts with it.
- `reach_gate.py` has no `diff` subcommand, so the dev comparison used a one-off script that compares two binaries on dev symbols only.
- A C# extension method is found by its first parameter after attribute groups and `ref`, `in`, `scoped`, and `readonly` modifiers, before and after `this` (`[NotNull] this Lib l`, `ref this Big b`, `this ref int x`). The owner rule in `extended_type` uses the same reader, so a receiver of the extended type no longer excludes such a method. TypeScript needs no change: a decorator or modifier before `this` is error TS1433.
- The range of a Python, Java, C#, Kotlin, or TypeScript method is the union of its own range and the ranges of the methods of the same name in every base type of its owner (the `Overrides of` walk, run before the sites are labeled). A cut walk (50 types, depth 8, or the 1 s limit) or a base method without a range gives no range. Rust and Go skip the walk, because a trait or interface method has the same count. The depth cut of the walk is now reported, which it was not before; the `Overrides` note still names the 50-type limit only.
- A base method keeps its own range. A call that passes a count the base method does not accept either fails to compile or fails when it runs on a base object, so it runs only an override.
- A TypeScript abstract or interface method has no range, because julie starts its body at the parameter list and the rows after the body start are dropped. An override of one has no range either, by the rule above.
- A TypeScript parameter typed `void`, or a union that has a top-level `void` member, is optional. A type parameter that a caller sets to `void` (`f<void>()`) is not read.
- A Python function that a class-level variable, property, or constant of the same name binds, with a value that names the function (`render = render`, `wrapped = staticmethod(wrapped)`), has no range. An assignment through `self.` and an assignment outside the class body (`View.render = render`, julie records none) are not read.
- Parameter rows are sorted by start byte, then by the first position of the name in the parameter list, so Go parameters that share one type keep their signature order.
- Three wrong exclusions older than slice 3 are fixed in the callers report. A call through `super`, `base`, or Kotlin `super<X>` never resolves to the caller's class: Java and Kotlin link `super.f()` in an override to the override, so `pair_identifiers` drops a relationship row that links into the caller's class or an enclosing class (`links_enclosing_class`) when its one call identifier on the line has such a receiver, and judges the identifier (`class chain`). An identifier with such a receiver stays a row on a definition line (Kotlin `override fun g(a) = super.g(a)`).
- The dropped-link call is excluded as ``receiver `super` reaches only the ancestors of `Sub` `` when no owner of the target is an ancestor of the caller's class (an unindexed base class keeps the old exclusion). Pending `super` rows of TypeScript and Python are not touched: a mixin or a Python MRO can reach a class outside the declared ancestors.
- A receiver declared `any`, `unknown`, or `object` (TypeScript), `Any` or `object` (Python), or `dynamic` (C#) gives no receiver type, so it never excludes a site. TypeScript `unknown` and `object` reach a member only after a check narrows the variable, and the compiler rejects the direct call (TS18046, TS2339). A C# `object`, a Java `Object`, and a Kotlin `Any` keep excluding. A Kotlin smart cast (`if (x is Cat) x.purr()`) could make a call on an `Any` variable legal; no Kotlin compiler is installed here, so that case is not covered.
- Gaps left: a C# `base.F()` whose base class is in another file stays `unresolved` (the resolver knows `super`, not `base`); a Python `super().f()` whose base class is in the same file stays `unresolved` (the call result before the `.` marks a hidden receiver).
- The C# `this ref int x` parse was fixed in the first 3.3 pass; the slice 2 binary still excluded `a.ExtRef(2)`. The test `a_receiver_of_the_extended_type_does_not_exclude_an_extension_method_with_modifiers_after_this` covers `this ref`, `this in`, `this scoped ref`, and `this ref readonly`.
- A Python `Any` written through its module (`typing.Any`, `typing_extensions.Any`, `t.Any` after `import typing as t`) or imported under another name (`from typing import Any as AnyT`) gives no receiver type, and so does a project TypeScript type alias of `any`, `unknown`, or `object` (`type Anything = any`; up to four aliases deep, read from the source text of the alias, because julie gives the alias no signature). `reference_report/receiver_types.rs` holds the rule. A TypeScript union that contains `any` (`any | null`) already gave no receiver type; `Optional[Any]` and a Python alias of `Any` are not read.
- A receiver declared with a type parameter gives no receiver type: a type parameter of the caller, of a function that encloses it, or of a class or interface that encloses it. The lists come from the `<…>` or `[…]` list after the name or before the return type in the signature (Java, C#, Kotlin, Rust), from julie's `typeParameters` of a TypeScript function or method, and, for a TypeScript class or interface (julie writes neither signature nor metadata for its parameters), from the first 600 bytes of its source. A Python `TypeVar` variable counts as well. The bound is not read: a call through `T extends Greeter` is `unresolved`, never excluded by `Greeter`.
- The ancestor walk (arity range and `Overrides of`) reads an import alias that the file of the subtype binds: `import { Base as B }`, `from m import Base as B`, Kotlin `import a.Base as B`, and C# `using B = N.Base1;`. The `Overrides` walk (subtypes) reads no alias, because it would scan every import row of the index for each type. The JVM tie check in `qualified_import` ignores an aliased import line, which writes no dotted name for the base. A base that a TypeScript default import or a Kotlin `typealias` names stays unresolved; the override keeps only its own range.

**Slice 3 task 3.3 result (2026-10-19):** fmt, clippy, 1111 tests, plugin tests, and `cmp` pass; the reach gate is PASS. Against the slice 2 report, excluded sites grew in csharp (317 to 419), java (2407 to 2432), rust (2627 to 2631), and typescript (1852 to 1966); go, javascript, and python are unchanged. Excluded precision, recall, accounted share, and proven precision are unchanged in every language. `lost_proven` prints nothing for the 7 repos. Every changed dev site went from unresolved to excluded and none covers a truth line. Latency and memory are not yet measured.

**Slice 3 task 3.3 fix 1 (2026-10-19):** super calls, `any`-typed receivers, and the C# `this ref` test; the reach gate is PASS. Against the first 3.3 gate, one typescript holdout site went from excluded to unresolved (it covers no truth line); every other cell is unchanged. `lost_proven` against the slice 2 binary prints nothing for the 7 repos.

**Slice 3 task 3.3 fix 2 (2026-10-19):** `typing.Any` and its aliases, type parameters, TypeScript aliases of `any`, and base types named through an import alias; fmt, clippy, 1161 tests, and `cmp` pass; the reach gate is PASS. Against the fix 1 gate, excluded sites fell in java (2432 to 2429) and typescript (1965 to 1964) and became unresolved; the unresolved sites that cover truth lines stay 29 and 200, and the excluded sites that cover truth lines stay 2 in java and 2 in typescript. Excluded precision, proven precision, recall, and accounted share are unchanged in every language. `lost_proven` against the slice 2 binary prints nothing for the 7 repos.

**Slice 3 gate:** branch gate; latency and memory measured.
- Result (2026-10-19): fmt, clippy, 1161 tests, plugin tests, and `cmp` pass. The reach gate passes; against the slice 2 code with the 3.8.0 pin (`gate-s2-j38`), excluded sites rise in C# (317 to 419), Java (2407 to 2429), Rust (2627 to 2631), and TypeScript (1852 to 1964), the excluded sites that cover a truth line stay at 2 in Java and 2 in TypeScript, and every other cell is equal. `lost_proven` prints nothing for the 7 repos. Live servers (`refs_latency_js.py`, 5 rounds): p50 within +3% on all 10 bench symbols, RSS after calls within 1 MB. julie 3.8.0 alone leaves the gate report identical to slice 1. The detailed arity rules are in `docs/argument-counts.md`; `CLAUDE.md` keeps a summary.

---

## Slice 4: Post-edit check

### Task 4.1: Edit check core

**Files:** `crates/code-kb-core/src/edit_check.rs` (new), `crates/code-kb-core/src/lib.rs`, `ops.rs` (an `edit_check_op`), `crates/code-kb-core/tests/edit_check_test.rs` (new).
**Interfaces:** `edit_check_op(workspace, base: Option<&str>) -> EditCheck`:
1. Base: the given revision, else the session base (task 4.2), else `HEAD`. Changed files: `git diff --name-only {base}` plus untracked files (`git ls-files --others --exclude-standard`), only files the index covers, at most 200.
2. New-side hunks: `git diff -U0 {base} -- {files}` line ranges; an untracked file is one whole-file hunk.
3. Base symbols: each changed file that exists at the base is written from `git show {base}:{path}` into `<root>/.code-kb/check/<pid>/tree/{path}`, then `julie-extract scan --root <tree> --db <tree>/../base.db --level full` runs once; the temp folder is removed after the check.
4. Current symbols: the main index after `ensure_fresh_file` on each changed file.
5. Shape changes, keyed by (path, parent names, kind, name, overload ordinal): `removed` (`renamed to {new}` when a symbol with the same parent, kind, and body hash has a new name), `parameters {a} → {b}` (callable; parameter rows as in task 3.3), `signature changed` (callable; whitespace-normalized signature differs, counts equal), `fields +{a}, -{b}` (field and property children of a class, struct, or record), `variants +{a}, -{b}` (enum members). Local variables and parameters themselves are not symbols for this step.
6. Sites per change, from the callers report of the current symbol (or, for a removed symbol, of every remaining definition with the old name and the name's unresolved sites): calls for parameter and signature changes (a site whose `arg_count` already fits the new range is dropped), constructions for field changes, variant uses for variant changes, all sites for removals. A site inside a new-side hunk is "in your diff"; the rest, with label `proven` or `likely`, are "not in your diff"; `unresolved` sites outside the diff are counted.
7. Budget: 20 s and 10 changed symbols (most sites first); a cut is stated.
**Contract inputs:** post-edit map sections 2-4.
- [x] Temp-repo fixtures (git init in a tempdir): a Rust function gains a parameter and one caller is updated (the other caller is listed); a struct gains a field (an unchanged construction is listed); an enum gains a variant (the function with the `match` is listed); a removed Python function (its caller is listed); a renamed function (`renamed to`); a body-only edit lists nothing.
- [x] The main index file is unchanged by a check (its mtime and row counts).
- [x] Worker scope green; diff handed to the lead.

**Deviations from the plan text (task 4.1):**
- `edit_check_op(workspace, db_path, conn, base)` takes the index path and connection like the other ops and returns `Result`. `edit_check_with(..., BaseRequest, EditLimits)` serves the session base and the tests. `EditCheck` gains `diff_hash`, which the once-per-diff rule of 4.2 reads.
- The revision shows as a 10-character short sha, or `no commits` for a repository with none; the source labels are `given`, `session base`, and `HEAD`.
- The count text is `fields +1 (z), -0`. A parameter change shows the `Arity` range text (`1-2`) when both sides have a range, and the parameter row counts otherwise. Singular forms are used (`1 shape change`, `1 file changed`). A cut prints as `Cut: …`. The in-diff list shows 12 sites and the tests line 8; the check keeps 50 commands.
- The callers report used here (`reference_report_for_edit`) runs no text scan and no argument-count exclusion, because the exclusion would hide a caller that still passes the old count. A call whose count fits the new range is dropped.
- A variant counts as "in your diff" when a changed line inside the caller names an added variant. A struct literal counts as "in your diff" when any line of its bracket extent changed.
- A rename pairs one-to-one by julie's `body_hash`, within the same parents and kind. Git rename detection is the default `-M` (50% similarity), and base files are read with `git cat-file blob`.
- A file that is not indexed and has no indexed extension is ignored. Check folders older than 1 hour are removed.
- A Python class counts fields only when it is a `@dataclass`, and only annotated class-body variables.
- Callers of a Rust function imported with `use` come out `likely`, not `proven`; that is the report engine's rule. The tests accept either.
- No unit test covers `format_edit_check` itself (the integration tests cover its output), no run covers a very large repository, and the time budget is tested only with a zero budget.

### Task 4.2: Tool, CLI, and hooks

**Files:** `server.rs`, `main.rs`, `hooks/claude-codex-hooks.json`, `tests/plugin/plugin-manifests.test.cjs`, CLI and MCP tests.
**Interfaces:**
- MCP tool `check_edit`: description, exactly, `After you edit: the symbols whose shape changed against the session base (or base, or HEAD) and every site of each that your diff did not reach. Run it before you say a change is done.` Properties: `project_root` (required), `base` (string, optional, "Git revision to compare with (default: the session base, else HEAD).").
- CLI `code-kb check [--base REV] [--json]` prints the same text.
- Text, exactly as this shape:
  ```
  check_edit: working tree against {rev} ({source}), {n} files changed
  {k} shape changes:
    {kind} `{display}` ({path}): {change}
      In your diff ({a}): {path}:{line}, …
      Not in your diff, proven ({p}):
        {path}:{line}  {caller}  {arg note}
      Not in your diff, likely ({l}): …
      Unresolved outside the diff: {u}
  Tests to run: {command}; …
  Not checked: calls inside macro arguments, functions passed as values.
  ```
  No shape change: `check_edit: working tree against {rev} ({source}): no symbol changed shape.`
- `code-kb hook SessionStart` reads the hook JSON on stdin when present and writes `<root>/.code-kb/sessions/{session_id}.json` with `{"base": "<HEAD sha>"}` (no file when the root has no git repo or no index); its stdout does not change.
- `code-kb hook Stop` and `code-kb hook SubagentStop` read the hook JSON on stdin (`session_id`, `cwd`, `stop_hook_active`), run the check with the session base, and print `{"decision":"block","reason":…}` only when some change has sites not in the diff, at most once per diff hash (the hash is stored in the session file), and never when `stop_hook_active` is true. The reason, exactly: `Before you finish: {n} sites still use the old shape of {names}.` then at most 15 site lines, then `Edit them or say why they need no change.` Otherwise they print nothing and exit 0. Any error prints nothing and exits 0.
- `hooks/claude-codex-hooks.json` adds `Stop` and `SubagentStop` entries with `node "${CLAUDE_PLUGIN_ROOT}/bin/code-kb-launcher.cjs" hook Stop` (and `SubagentStop`), timeout 60.
- [x] MCP and CLI text equal on a fixture; the tool list has 7 tools.
- [x] Hook tests over stdin: block once, silent the second time with the same diff, silent with `stop_hook_active`, silent with no change, silent outside a repo.
- [x] Plugin manifest test covers the new entries.
- [x] Worker scope green; diff handed to the lead.

**Deviations from the plan text (task 4.2):**
- Hook payloads were checked against the Claude Code hooks page (`code.claude.com/docs/en/hooks`) and the Codex hooks page (`developers.openai.com/codex/hooks`). Both run `Stop`, `SubagentStop`, and `SessionStart`, and both read `{"decision":"block","reason":…}` on stdout with exit 0. Plain text on Stop stdout is invalid in Codex, so the hook prints nothing otherwise. The pages do not say that stdin is always provided, and they do not confirm that Codex loads a Claude hooks file unchanged; `.codex-plugin/plugin.json` already points `hooks` at the shared file.
- `SessionStart` writes the session file only when it does not exist, so a resume or a compaction does not move the base. The file also holds `"blocked_diff": "<hash>"`. A Stop that blocks and finds no session file checks against `HEAD` and creates a file that holds only that hash.
- With no session id, `check_edit` and `code-kb check` use the newest session file written in the past 24 h. `SessionStart` removes session files older than 7 days. Risk: two sessions on one root can see each other's base.
- `edit_check_session_op` falls back to `HEAD` when git no longer has the recorded base. An empty `base` string means no base.
- Stop blocks only on `proven` and `likely` sites. `unresolved` sites are counted and never block.
- Site lines read `{path}:{line}  {display}: {change}`, `proven` first. The header keeps the plan's literal "{n} sites", so one site reads "1 sites".
- A stdin that stays open is cut off after 2 s. A bad `session_id`, a missing index, or any error gives silence.
- The four new `mcp_test.rs` tests were not run before the code; they were checked afterwards to fail with the tool or handler removed.
- Not done: the `instructions` string of the MCP `initialize` reply does not name `check_edit` (task 1.1 fixes its exact text); no run in a live Claude Code or Codex; no run of the hooks on Windows or macOS.

### Task 4.3: Docs and release prep

**Files:** `CLAUDE.md`, `AGENTS.md`, `README.md`, routing blocks (one line: `- **After you edit**, call \`check_edit\` before you say the change is done.`), `skills/code-kb/SKILL.md`, `docs/site/index.html`, `docs/release-notes/v2.7.0.md` (draft, not released).
- [x] Docs describe the tool, the hooks, and the session file.
- [ ] Branch gate green; the owner is asked about the merge and the 2.7.0 release.

**Deviations from the plan text (task 4.3):**
- The rules, the text, and the known limits live in `docs/edit-check.md`; `CLAUDE.md` and `AGENTS.md` keep one summary bullet and the changed hooks bullet, because every session in this repository loads them.
- `docs/release-notes/v2.7.0.md` starts with an HTML comment that marks it as a draft and lists what must happen before the tag. No version file changed.
- The Codex install text now says "the four code-kb hooks".
- `docs/site/index.html` gets a feature entry, the hooks tab text, and the two new hook entries, besides the tool row that 4.2 added.

**Fix round (2026-10-19):** The end-to-end check of slice 4 (about 120 probe repositories in Rust, Python, TypeScript, Java, and Go) found the defects below. Each fix has a test in `edit_check_test.rs` or `edit_hook_test.rs`.
- Fixed, sites: a removed or renamed method lists the callers that reach it through a typed receiver or a class qualifier (Rust, TypeScript, Java, Go), because the report now judges the old name, kind, file, and owner. A removed or renamed top-level symbol lists the import lines of its old name. A changed method lists the overriding members of its subtypes. A Go call with one argument is listed when a function gains a second parameter. A call that still passes the old count, in an untracked file or on an edited line, is never "in your diff" and has the note `passes N args; takes R`.
- Fixed, false sites: a variant is a site only in a `match`, `switch`, or `when` that names it and has no fallback arm (`Self::`, `use Enum::*`, and Java `case NAME:` are read); a function that only builds the variant is neither listed nor counted, and one with a fallback arm is counted as unresolved. An added field lists only the constructions that must name it: Rust and Go literals, a Python dataclass call when the field has no default, and a Java record call. A parameter name, a default value, or an annotation is no `signature changed`; `parameter renamed` lists the calls that name the old parameter in Python, Kotlin, C#, and Swift. A change with equal range texts prints the row counts, or `parameters changed`, and not `1 → 1`. An unknown base prints its error once.
- Fixed, hooks: `SessionStart` records the base in any git repository with a commit, also with no index yet; it creates `.code-kb/`, its `.gitignore`, and the session file, and no index. This replaces the task 4.2 text "no file when the root has no index". `Stop` and `SubagentStop` print nothing and start no scan when another `julie-extract` version or level wrote the index (`index_written_by_other_extractor`, the rule of the version guard in `sync.rs`); before, the first `Stop` after an upgrade rescanned the index in place (9.26 s on zod).
- Fixed, docs: the release notes now say that the SessionStart routing text gains a line, list the reach gate rerun in the draft comment, give the timings recorded in the end-to-end check, and describe what blocks a stop.
- Kept as known limits (`docs/edit-check.md`): fields of a TypeScript `interface` or `type` alias are not detected; the reads of a removed field are not listed; a renamed parameter is listed only in Python, Kotlin, C#, and Swift; a plain `mv` without `git add` reports the functions as removed; a `SubagentStop` block uses the once-per-diff hash, so a later `Stop` on the same diff is silent; two sessions on one root can see each other's base; the hooks check nothing after a `julie-extract` upgrade until the server or a `code-kb` command rebuilds the index.
- Not run in the fix round: the reach gate and `lost_proven`, although `reference_report.rs` changed.

**Fix round 2 (2026-10-19):** A recheck of the first fix round (about 165 probe scenarios in Rust, Python, TypeScript, Java, Go, and C#, plus hook, payload, and timing probes) found the defects below. Each fix has a test in `edit_check_test.rs`, `edit_hook_test.rs`, or next to the code. `code-kb --json refs` was compared before and after the report changes on probe repositories, and the output did not change.
- Fixed, variants: a Rust `match` whose arms hold a sub-pattern wildcard (`Some(_)`, `(_, _)`), a C# arm that binds a name (`var other`), and a Python `case (_, _):` are no longer sites; they count as unresolved. A function that holds no `match` over the enum (a dictionary or `Record` keyed by variants, an `if` chain, a value that builds or compares a variant) is counted as unresolved and never listed, so the total stays honest. A removed variant runs the `Variant uses` finder again with its old name: the functions that still name it are sites at the line of their first use, and a function that names only other variants is not.
- Fixed, signatures: an `async`, `suspend`, or `static` that came or went is `signature changed: async added` and lists every call. A Go result list with names, a Java or Kotlin parameter annotation, and a Rust `_` parameter that gets a name are no shape change.
- Fixed, removed overloads: when overloads of a removed or renamed method remain, a call is listed only when the removed overload took it and no remaining overload does; a call with no known count is unresolved, and no import line is listed.
- Fixed, fields: a Rust `Self { … }` literal in a method of the struct and a destructuring pattern without `..` are judged like a literal; a Go slice, array, or map literal of a package-qualified type is judged by its elements; a Python `ClassVar` is no field, and a `field(…)` call has a default only with `default=` or `default_factory=`; a Java record with an extra constructor lists only the calls with the old component count, and a `this(` line is no site.
- Fixed, calls: a call that spreads its arguments is unresolved when the parameters change, and a named argument of another call on the same line no longer lists the call.
- Fixed, hooks: `Stop` and `SubagentStop` compute the diff hash first (`PendingCheck` in `edit_check.rs`: the base, the changed files, and their content, with no scan of the base) and end with no output when it equals `checked_diff` or `blocked_diff` in the session file. Before, every turn end ran the whole check: 9.3 s warm on a 75-file shape change in Newtonsoft.Json. Now a repeat costs 0.07 to 0.08 s (zod copy, 17 changed files; the first check took 5.35 s). `checked_diff` holds the last diff a hook checked, blocked or not, and the rule "block at most once per diff hash" is kept. A `<session_id>.lock` file, created exclusively, makes the hooks of one session take turns: a hook that cannot get it within 2 s prints nothing, and a lock older than 60 s is taken over. Before, four parallel hooks of one session all blocked.
- Fixed, docs: `docs/edit-check.md` and the release notes give the fold text (`… N more likely`, `… N more proven`), no longer claim that a TypeScript bound change is detected, state what blocks a stop, and list the known limits below. The release notes carry the recorded timings: a warm check on a small two-file change takes 0.15 to 0.17 s (0.19 to 0.25 s the first time); zod shape edits of 1, 5, and 30 files took 0.23, 0.82, and 4.89 s; Newtonsoft.Json shape edits of 30 and 75 files took 13.4 and 12.5 s; the worst first check was 17.5 s, a 39-file change of this repository, 14.6 s of it the CLI reconcile of the index before the check (a hook in a running server does not pay it).
- Kept as known limits (`docs/edit-check.md`): fields of a TypeScript `interface` or `type` alias; the reads of a removed field; a call through an alias of a removed symbol (`import x as y`, Rust `use … as`) and the Java `import` and `import static` lines; a Rust tuple struct whose element count changes; a field added to an enum variant's payload; a new method on a trait or interface (the implementors are not listed); TypeScript type parameters and bounds (julie's TypeScript signature holds neither); a renamed parameter outside Python, Kotlin, C#, and Swift; a plain `mv` without `git add`; the once-per-diff hash shared by `Stop` and `SubagentStop`; two sessions on one root that see each other's base; the hooks that check nothing after a `julie-extract` upgrade.
- Found and not fixed: removing a function that nothing references prints `no symbol changed shape` and `Cut: The sites of … could not be listed: Database query error: Query returned no rows.` The same text came from the binaries before both fix rounds. It never blocks a stop.
- Not run in fix round 2: the reach gate and `lost_proven`, although `reference_report.rs` changed; a hook run inside a live Claude Code or Codex.

**Fix round 3 (2026-10-19):** A recheck of the second fix round found the defects below. Each fix has a test in `edit_check_test.rs`, `edit_hook_test.rs`, or next to the code. The change to `reference_report/arity.rs` was checked with `code-kb --json refs` before and after on 248 names in the probe repositories, and the output did not change. The other fixes touch no report code.
- Fixed, sites: a removed function, method, or class that nothing uses is a change with no site and no cut; before, it printed `Cut: The sites of … could not be listed: Database query error: Query returned no rows.` (`a_removed_function_that_nothing_calls_has_no_sites_and_no_cut`). When overloads of a removed method remain, a call that both the removed overload and a remaining one take is counted as unresolved, never dropped and never listed (`a_call_that_the_removed_and_a_remaining_overload_both_take_is_counted_as_unresolved`); a site that the report labels `unresolved` is counted and not listed; and no import line is listed while an overload of the name remains (`an_import_of_a_name_is_not_listed_when_an_overload_of_it_remains`). A Java or Kotlin constructor has a range when the index lists every constructor of its class; a Java record (canonical constructor) and a Kotlin class with a primary constructor give no range, so their sites are unresolved (five tests with `constructor` in their names). The body cut of a Kotlin secondary constructor no longer drops its parameter rows.
- Fixed, calls: a named argument lists a call only when the argument stands in the arguments of the call itself, or in a nested call that has the name of the changed function (`f(f(old=1))`); `f(g(old=1))` and `f(1, b=[g(old=1)])` no longer list `f` (`a_named_argument_counts_only_in_the_calls_own_arguments_or_in_a_nested_call_of_the_same_name` and a unit test in `signatures.rs`). A Java record `this(…)` line is a site when its count equals the old component count and no site with another count, and the argument count splits at commas outside round, square, and curly brackets (`a_java_record_with_an_extra_constructor_lists_the_calls_and_this_calls_with_the_old_component_count`, `a_this_call_with_the_old_component_count_is_a_site_of_a_record_that_loses_a_component`, and a unit test in `fields.rs`).
- Fixed, signatures: a C# attribute (`[Obsolete]`, `[HttpGet("/a")]`, a parameter attribute `[Range(0, 1)] int a`) and a PHP `#[Pure]` are dropped from the head of the signature and from each parameter before the compare, with balanced brackets (a string inside the group is skipped). The same strip runs before the `async` and `static` words are read, so an attribute that holds the method's name (`[Authorize] public async Task Authorize()`) no longer hides the word. Before, every attribute change read as `signature changed` and listed every call. A C++ `static` counts as a call-shape word only for a member of a class or struct; at file scope or in a namespace it gives internal linkage, and the toggle `static int helper(int)` ↔ `int helper(int)` is no shape change. The row in `docs/edit-check.md` that claimed C++ toggles is fixed. Tests: `a_csharp_attribute_added_removed_or_on_a_parameter_is_no_shape_change`, `a_csharp_attribute_that_holds_the_name_of_the_method_does_not_hide_an_async`, `a_php_attribute_added_or_on_a_parameter_is_no_shape_change`, `a_cpp_static_is_a_call_shape_word_only_for_a_member_of_a_class_or_struct`, and a unit test in `signatures.rs`.
- Fixed, variants: a C# pattern that starts with `not` (`not Kind.Circle =>`, `not (A or B)`, `case not Kind.Circle:`) matches every other value, so it names no variant and its construct makes the function unresolved. Before, it read as an arm that names a variant, and the function was listed as a site (`a_csharp_pattern_that_starts_with_not_names_no_variant` in `arms.rs`).
- Fixed, hooks: a session file that a `Stop` created before `SessionStart` has no `base`; the next `SessionStart` now adds it under the session lock and keeps `checked_diff` and `blocked_diff`. A file that has a base keeps it. A `SessionStart` that cannot get the lock within 2 s writes nothing. Before, `record_session_base` returned `Ok` on `AlreadyExists`, so that session compared with `HEAD` for good (`a_record_adds_the_base_to_a_file_that_has_none_and_keeps_its_other_fields`, `session_start_adds_the_base_to_a_session_file_that_a_stop_created`, `session_start_that_cannot_take_the_session_lock_within_two_seconds_adds_no_base`).
- Fixed, docs: "the same diff never blocks twice" was false. A hook skips only the last checked diff and the last blocked diff, so a block on one diff, a block on a second, and a return to the first blocks again. No hash set was added. "The whole check stops after 20 s" was false too: the clock is read between steps (the file list, each file refresh, the shape comparison, each changed symbol), so a scan of the base, a refresh, or the sites of one symbol that already runs can pass 20 s. Both claims are fixed in `docs/edit-check.md`, the release notes, `CLAUDE.md`, `AGENTS.md`, the README, and the site.
- Kept as known limits (`docs/edit-check.md`): a Rust `Self { … }` literal in an associated `const`, a `for Point { x, y } in v` pattern, and a Go struct-keyed map literal (`map[Point]int{{1, 2}: 3}`) are neither listed nor counted; a removed unit variant that a function names only in a `match` arm as `Kind::Circle { .. }` is not found; a Python `@staticmethod` or `@classmethod` toggle, `KW_ONLY`, and `InitVar` are not read (listed since round 2).
- Fixed after the round 3 recheck (lead): the Java argument count read from the source skipped no type argument list, so `new Pair<>("a", 1)` and `new Box<String>("a", 1)` got a wrong count and a real site was dropped from a record change and from a removed constructor overload. `call_counts` now skips a `<…>` group after the name and reads a call only when `(` follows (`a_generic_java_record_lists_the_constructions_with_type_arguments` and the unit test in `fields.rs`). The recheck's 99 probe scripts were rerun on the fixed build: every difference is this fix or noise (hashes, paths, times).
- Not run in fix round 3: a hook run inside a live Claude Code or Codex.

**Slice 4 task 4.3 result (2026-10-19):** fmt, clippy, 1471 tests (after fix round 3), plugin tests (20), and `cmp` of `AGENTS.md`/`CLAUDE.md` and of the two routing blocks (1070 bytes each) pass. The reach gate passes with a report equal to slice 3 (`phase2/s4/gate/gate/report.md`), `lost_proven` finds no lost proven site in the 7 repositories against the slice 3 binary, and a live `serve` gives the same `refs` latency (zod `parse` 726 ms against 734 ms, hermes-agent `get` 3.32 s against 3.18 s within noise) and memory (47 MB peak against 43 MB). A warm `code-kb check` takes 0.15 to 0.17 s on a two-file change in a small repository. In the recheck, a shape change in 1, 5, and 30 files of zod (205 MB index) took 0.23, 0.82, and 4.89 s cold; a 400-file change on Newtonsoft.Json, cut to 200 files, took 6.10 s; the worst case was 17.5 s (a 39-file change of this repository, 14.6 s of it the CLI reconcile), under the 20 s budget. The release dogfood round passed on `59ca3dc` (one wrong answer, the lambda rename, fixed in `7531245`; the gaps are in the 2.7.0 notes). julie 3.8.0 is published (task 3.2), and `scripts/julie-pins.json` holds its `sha256` values. Open before the owner can release: the owner's answer on the merge and on 2.7.0, and the corpus check on the version-bump commit.
