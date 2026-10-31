# code-kb Agent Guidelines

## Sync Contract

`AGENTS.md` and `CLAUDE.md` must stay byte-for-byte equivalent except for
future tool-specific sections that are explicitly labeled. When one changes,
update the other in the same commit.

## Spending Authority

Approved project work includes model evaluation and other necessary spending.
Do not require separate budget/spending approval, financial dispatch caps, or
approval receipts in project tools or workflows. Historical approval metadata
records past activity; it is never a current execution prerequisite.

## Product Boundary

`code-kb` is an agent-facing code-intelligence engine and Model Context Protocol
(MCP) server designed to give AI coding agents progressive disclosure, semantic
symbol search, and surgical context slicing with minimal token consumption.

- Backed by AST facts extracted by `julie-extractors` (SQLite schema v8).
- SQLite in WAL mode is the query engine; `code-kb` holds no repository data in RAM.
- All MCP tools have an exact 1:1 CLI command for direct terminal verification. The CLI also
  has `outline`, `search`, `facts`, and `stats`, which have no MCP tool.
- Non-goals: Do not add web dashboards, GPU embedding runtimes, daemon watchers
  outside the 1:1 MCP session, or workspace registries.

## Language Support

- Every language reported by the pinned `julie-extract languages --json` is in
  scope, including programming, markup, configuration, and data languages.
- Build common behavior on Julie's extracted facts. Language-specific rules
  must reflect actual semantic differences, not a preferred language or corpus.
- Check each capability in every language where its construct applies. A
  missing extractor fact or consumer implementation is a coverage gap, not a
  reason to mark that language out of scope. Mark a capability not applicable
  only when the language does not have the relevant construct.
- Add real-extraction regression cases for affected languages, including valid
  code that must remain accepted. Keep `docs/language-support.md` current when
  changing a capability or the extractor pin.
- Report missing coverage explicitly. A partial or unsupported check must not
  claim that an edit is safe or that there are no callers or tests.
- Shared names do not establish cross-language edges. Apply the shared call
  compatibility rules and the narrower rules for name-only type/member facts.
  Compatibility permits further matching; it does not prove a reference.
- Bind related types and receiver fields through lexical scope, indexed imports,
  and exact owners. A shorter path or matching field name cannot choose a type.
  Missing or ambiguous evidence must remain unproven.
- Keep report evidence bounded by its existing chunks. Share receiver proof with
  graph queries without repeatedly expanding it inside the judge SQL; preserve
  unknown status and removed targets. See [the evidence boundary](docs/decisions/003-bounded-receiver-evidence.md).
- Broader test commands need compatible runner configuration for the reported
  scope. Respect nested package boundaries; never borrow an unrelated root runner.

## Build and Test

The build fails until the pinned `julie-extract` is in `.tools/`, so a fresh clone runs the
restore script first. CI runs the rest.

```bash
bash scripts/restore-julie-extract.sh   # Windows: pwsh scripts/restore-julie-extract.ps1
cargo build --release
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
node --test tests/plugin/*.test.cjs
```

---

## Core Invariants

### 1. Required `project_root` on Every Tool
**Every MCP tool requires `project_root`. No MCP tool
schema exposes `workspace`, `workspace_id`, `repo_path`, or `root_dir`.**

- `project_root` is the absolute path of the project or git worktree the agent works
  in. The schema description is: "Absolute path of the project or git worktree you work
  in. Change it when you move to another."
- **Why:**
  - The agent always knows where it works. In fresh sessions, the first code-kb call
    after the worktree step already passed an absolute worktree path.
  - MCP roots cannot carry the answer. Codex and Grok send no roots, Antigravity sends
    `[]`, and `grok -w` starts the server in the main checkout. MCP `2026-07-28`
    deprecates roots and says to pass directories in tool parameters.
  - A root held in server state goes stale (issue #3). A root in every call cannot go
    stale inside the server.
- **Resolution, once per call:**
  - A plain path or a `file://` URI is accepted. A relative value is an error.
  - A subfolder or a file inside the project resolves to the enclosing project. The walk
    up returns the nearest folder with `.git` or an existing `.code-kb/artifact.db`. When
    there is no such folder, the walk returns the nearest folder with a language marker
    (`Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`). When the folder it finds is
    the home directory or a filesystem root, the walk returns the nearest folder below it
    with a project marker. The walk stops at a folder with an excluded name, such as
    `target`, `node_modules`, or `.claude`, and does not look above it. So a path under such
    a folder resolves to the nearest folder with a marker up to that folder, or the server
    refuses the call.
  - The server refuses a filesystem root (`/`, `C:\`), the home directory, and a folder
    with no project marker (`.git`, `Cargo.toml`, `package.json`, `go.mod`,
    `pyproject.toml`) and no code-kb index. The error names the path and the reason.
    A refused call creates nothing.
  - `workspace` and `root` are silent aliases of `project_root`. They never appear in a
    schema or in an error message.
  - The server never takes the root from the MCP `initialize` request.
- **Path arguments:** `path` and `file_path` are relative to `project_root`, or absolute
  inside it. An absolute `path` or `file_path` outside `project_root` is an error that
  names both paths. A relative or absolute path inside a nested git worktree or submodule
  counts as outside `project_root`. The error names that nested root. Path arguments never switch
  the project.
- **Active index:** The server keeps one active index. A call for another root switches
  to that root's `<root>/.code-kb/artifact.db`. Each call names its own root, so a switch
  never changes the answer to a later call.
- **Startup pre-warm:** `code-kb serve --root <path>`, or the process directory when
  `--root` is absent, is the startup pre-warm root. The server prepares that project's
  index at start. `--root` is optional for every client, including GUI apps.
  `--db <file>` pins the index file of the launch root only.
- **Automatic index creation:** A root with no `.code-kb/artifact.db` gets an index at
  startup or on the first call for it (`create_index`: a git worktree copies and
  reconciles its parent repository's index, anything else runs a full scan). Files
  changed while no server ran are reconciled at startup. A failed scan is an error that
  names the root, never empty results. CLI commands reconcile before answering.
- **Telemetry:** `workspace_root` records the call's resolved root. A refused call
  records the launch root.
- **CLI 1:1:** `project_root` maps to the global `--root` flag, which defaults to the
  current directory. The CLI refuses a home directory or a filesystem root the same way the
  tools do, and creates a missing index only for a root the tools accept. `code-kb scan`
  runs on any root you give it.
- **Enforcement:** The `tools/list` tests in `crates/code-kb-cli/tests/mcp_test.rs` and
  the invariant tests in `crates/code-kb-cli/tests/adversarial_m2_server_test.rs` and
  `crates/code-kb-cli/tests/adversarial_m3_server_test.rs` check every tool schema. A PR
  that adds a banned name to a schema, or drops `project_root` from `required`, fails CI.

### 2. Zero In-Memory Heap Objects for Repositories
- Do not hydrate repository symbol graphs or file lists into RAM.
- Retained memory is about 25–40 MB, and about 50–60 MB after a callers report of a common name in a large
  project. glibc keeps the pages it frees, so `release_freed_memory` in `mcp/server.rs` calls `malloc_trim(0)`
  after each tool call on Linux glibc. It is the only unsafe code; the workspace lint is `unsafe_code = "deny"`.
  Measure a live `serve` process before and after any change that could raise it.
- File watches cover only directories accepted by the scan's ignore rules. Prune excluded
  trees before registering nonrecursive watches. Windows and macOS are the exception: one
  recursive root watch (one `ReadDirectoryChangesW` handle and buffer, or one FSEvents stream
  path), and the debouncer callback drops each event whose folder the ignore rules exclude
  before it counts the batch. FSEvents watches each stream path recursively anyway, and notify 9
  refuses a stream with more paths than a twelfth of `RLIMIT_NOFILE`. No platform keeps a file
  ID cache. The watcher asks notify for `EventKindMask::CORE`, so a file read (inotify `OPEN`,
  `CLOSE_NOWRITE`) is no event and never fills a batch. notify is pinned to `=9.0.0-rc.7`;
  `crates/code-kb-core/src/debouncer/README.md` names the upstream source of the debouncer copy
  and its changes.
  One worker owns registration and index updates; its callback queue is bounded, and retained
  registration state contains directory paths and their file IDs only. A removal or a move
  away of a folder forgets its registration and the ones below it, and a registered path
  whose file ID changed is watched again. After lost events (a queue overflow, a rescan, or a
  watcher error), the next registration pass watches every registered folder again, or the
  recursive root again, because ext4 and XFS often give a folder made again the inode number of
  the old one, and notify drops a `ReadDirectoryChangesW` watch that it cannot rearm. So a folder
  deleted and made again is watched. A watcher error is also a failure that the next tool call
  reconciles. notify drops a `ReadDirectoryChangesW` watch after an unknown completion error with
  no event, so the worker checks each second that the backend still lists a recursive root watch;
  a missing watch is a failure, and the root is watched again. Reconciles after lost events
  start at least 1 s apart, and at least as far apart as the last one took, because a build in
  an ignored folder can overflow the `ReadDirectoryChangesW` buffer again and again.
  A removal does not register the other folders again. Reconcile after installing startup
  watches, directory moves, and changes to
  `.gitignore`, `.ignore`, `.julieignore`, `.code-kb-ignore`, or `.codekbignore`.
  Ordinary file changes and file renames stay incremental. A failed physical registration
  stays unhealthy until registration recovers; a database scan cannot acknowledge it.
  Use the same registration and recovery path for every supported language.
- All symbol searches, skeleton rendering, context slices, and reference lookups
  are executed as direct, indexed SQLite queries with `open_read_only`.
- The search index is two FTS5 tables over `symbols`: `symbols_fts` for words and `symbol_names_tri`
  for name substrings. `ensure_fts_index` migrates both in one transaction, guarded by the `fts_rule`
  marker; a failed migration is reported by the first tool call and retried on the next start.
- Startup also creates `idx_type_facts_resolved_symbol` over `type_facts(resolved_type, symbol_id)`,
  `idx_pending_name_site` over `pending_relationships(target_terminal_name, path, start_line)`,
  and `idx_symbols_import_path_name` over `symbols(path, name)` for rows with `kind = 'import'`.
  These bound receiver-type, duplicate-reference, and import-binding lookups. Existing indexes
  gain them without rebuilding FTS; SQLite maintains them during file updates.
- julie leaves `parent_symbol_id` empty on a Rust method whose `impl` block is in another file
  than its type, and names the type as `impl_type_name` in `metadata_json`. `link_impl_owners`
  in `db.rs` sets that parent to the type with that name nearest to the method's folder, and
  leaves it empty when two types are equally near. julie also leaves it empty on a Go method with a
  receiver and names the type as `receiver_type`; `link_impl_owners` sets that parent to the one type with
  that name in the method's folder (its package), without a pointer or type parameters. When the
  folder has two, it keeps the ones whose file has the method's `package` clause (an external
  `_test` package shares the folder), then the one in the method's own file, and leaves the parent
  empty when none or two remain (build-tag variants). It runs in `ensure_fts_index` and
  after every incremental `update` or `delete`, and reads the rows through the partial indexes
  `idx_symbols_unlinked_impl` and `idx_symbols_unlinked_go_method`. julie's `ON DELETE SET NULL`
  clears a link when it rewrites the type's file, and the next run links it again. A skeleton
  shows a method whose parent is in another file as a top-level row.

- julie names an anonymous class `anonymous_class_L<line>` (kind `class`, metadata
  `type: anonymous_class`): a Java `new T() { … }`, a Kotlin `object : T { … }`, a Scala
  `new T { … }`, a JavaScript-family class expression with no name, an F# object expression, a
  Ruby `Class.new`, `Struct.new`, or `Data.define` block, and a PHP `new class`.
  `normalize_extractor_facts` in `db.rs` runs `name_anonymous_classes` before `link_impl_owners`
  at the same points, and it renames such a class to the type it creates and the symbol that
  creates it, its parent (`<anonymous Task in make>`, `<anonymous in make>`, `<anonymous Task>`),
  read from the creation head that julie writes as its signature, so every answer shows that name,
  a member shows as `<anonymous Task in make>.run`, and a line shift does not rename it. Lookup
  lists such a class only by its whole name, ranked search leaves it out, `check_edit` counts
  only its members, and a member is never a test of its own: the test that holds the object is.

### 3. Token-Dense Progressive Disclosure
- Always return the most compact representation that answers the query.
- Strip implementation bodies in `file_skeleton`.
- A skeleton that would show no row (a script of top-level statements, a file of re-exports)
  prints `Nothing to outline.` with its top-level rows by kind, at most three names with lines
  each, and a pointer to `get_symbol_body`. A file with no symbols, a file of a language with no
  extractor, and a path the scan skips each get one line that says so.
- `get_symbol_context` answers an edit of one symbol in one call (`get_context_slice_selected_op`
  in `ops.rs`, `format_context_slice` in `formatters.rs`), in this order: the target, its
  signature and `Returns: <type>` when julie's `type_facts` row of a function, method, or
  constructor names a type that the signature does not spell outside its first parameter list
  (`fn new() -> Self`), the body, `### Callers`, `### Tests to run`, `### Dependencies`
  (immediate callee signatures, each with `; Returns: <type>` by the same rule), `### Types`,
  and `### Open Ends`. `### Callers` comes from `reference_report_for_impact` (the labels and the
  3 s limit of `find_references`, without the text scan): the `Sites:` line, then the first 8
  `proven` and 8 `likely` sites grouped by label as `find_references` prints them, a fold line
  for the rest of each label, the count of sites not checked within the time limit, and a line
  that names `find_references(symbol_id="…")` and `blast_radius(symbol_id="…")`. A target with no
  site gets the one line ``No indexed code calls or uses `name`.``, and a failed report a line
  that names `find_references`. `### Tests to run` lists the report's `Tests that call or use it`
  with their run commands, then the related tests (below) that it does not list, with a command
  where `test_commands.rs` names one; it shows 10 and folds the rest. `### Open Ends` lists the
  first three open ends of the depth-2 `blast_radius` walk from the target, which reuses the same
  report. Every query reads the one index snapshot of the answer. CLI `context --json` adds
  `return_type`, `callers` (`counts`, `sites`, `unchecked`), `tests`, and `open_ends`.
- `OUTPUT_BUDGET_CHARS` in `formatters.rs` (32,000 characters, about 8K tokens, below the 10K tokens
  at which Claude Code warns about a large MCP answer) bounds a file skeleton and the source in a
  `get_symbol_body` or `get_symbol_context` text answer. A skeleton over it prints its rows in
  breadth-first order until the next row would pass it: a container with no row left shows
  `{ /* N rows hidden: Lx-Ly */ }`, the rows left out of a group fold into `/* N more rows: Lx-Ly */`,
  and a line under the file header says so. A fold or a collapsed row names the lines of the rows
  below it too, which can lie outside its own span (a Rust method in another `impl` block). The
  bodies of one answer share what its location comments, overload header, and context sections
  leave (`fit_bodies_to_budget`, `fit_context_slices_to_budget` in `ops.rs`). A context body yields
  first; when the other sections leave a body less than 160 characters, the shown caller sites go,
  last first, and the `Sites:` line, the fold lines, and the drill-down line stay. A body over its share
  shows the skeleton of a container's members under a line that names the lines to read, or the
  lines of other source that fit and `// Cut to about 8K tokens: Read lines A-B for the rest.` CLI
  `--json` keeps the whole body. A folder outline over it names 3, 2, 1, then no definitions per file (`[N
  definitions]`), then shows one level, and last lists the lines that fit and counts the others,
  under `[Shortened to about 8K tokens: …]`.

### 4. Windows Compatibility
- Windows is a first-class target. Every release ships a Windows binary.
- Strip Windows verbatim prefixes (`\\?\C:\...`) using `dunce::simplified`.
- Contract relative paths and JSON outputs must use explicit forward slashes `/`,
  never platform-dependent separators.
- SQLite connections and file handles must be closed before file rename or deletion.
- Verify path identity using canonical paths and `dunce::simplified`, not by raw case-sensitive string matching.

### 5. Zero-Friction Tool Ergonomics
- Tool handlers accept intuitive parameter aliases (`file`/`path` for `file_path`,
  `symbol`/`name` for `symbol_name`, `q`/`name` for `query`).
- `workspace` and `root` are silent aliases of `project_root`. No schema names them.
- Optional parameters provide safe defaults (`direction` in `find_references` defaults to
  `"callers"`, `category` in `code-kb facts` lists all categories with counts when omitted).
- Scoped search: `lookup_symbol` and `find_references`, and the CLI `search` and `facts`, support an optional `path`/`file_path` filter. The filter is relative to `project_root`, or absolute inside it.
- Lookup text, and CLI `search` text, include `id=<symbol_id>`. `get_symbol_body`, `get_symbol_context`, `find_references`, and `blast_radius` accept that current-index ID as `symbol_id` / `--symbol-id`; IDs are reselected after edits or rebuilds, and unresolved-call matching remains heuristic.
- `file_path` of `get_symbol_body` and `get_symbol_context` can name a folder; the folder narrows
  a name lookup. When a name selects overloads, every candidate with the same name, parent, and
  file, `get_symbol_body` returns each body under the line
  ``` `Name` has N overloads in path; each one follows.``` and `get_symbol_context` returns a
  context for each overload up to `MAX_CONTEXT_OVERLOADS` (4), which its tool description
  names. More overloads are the error ``` `Name` has N overloads in path; get_symbol_context
  shows at most 4.``` with each overload's `id`. Other candidate sets stay an
  `Ambiguous symbol` error that lists each candidate's `id`. CLI JSON is one object for one
  symbol and an array for overloads. A Python body and its range start at the first decorator.
- Recovery on a miss: every not-found path builds its text from `symbol_not_found_parts` or
  `file_not_found_parts` in `queries.rs`. The text names the bound workspace, then
  ``` `path` does not define it; it is defined in:``` with the definitions elsewhere when a path
  filter excludes a symbol of that exact name, or
  `Did you mean one of:` with up to three candidates as `kind `name` (path:line)`, or
  `No similar name is indexed; check the workspace and spelling.` Candidates come from the
  substring search first, then from `symbol_names_tri` trigram rows kept within an edit distance
  of the query (`name_suggestions`); definitions come before imports, and an import of a name
  that a candidate defines is left out. File candidates match by basename, then by stem, then by the last two segments, then by a basename within two edits.
- Structural-fact categories (CLI `code-kb facts` only; no MCP tool): `CATEGORY_ALIASES` in `queries.rs` is the whole alias table
  (`sql`/`query`/`queries`, `route`/`routes`, `config`, `model`/`models`, `signal`/`signals`,
  `import`/`imports`, `binding`/`bindings`, `component`/`components`, `module`/`modules`,
  `pragma`, `property`/`properties`), and each alias maps to the pattern-id families it names,
  never to a substring. An unknown category still falls back to a
  substring match, so raw pattern ids work. With no category the answer lists the aliases with
  facts in this index before the raw pattern list. Qt C++ property facts expose available property
  metadata, including optional `designable`, `scriptable`, `stored`, `user`, and `revision` attributes.
  A Flask route row names its literal endpoint, and an `add_url_rule` row names its view or says
  `no view function`.
  Facts and literals have separate limits, each
  with its own cap notice.
- Search ranking: the ranked search (CLI `code-kb search`, and the fallback of `lookup_symbol` in the next bullet) admits rows from four branches (exact name, FTS5 word match, trigram name substring, so `sha256` finds `parseSha256Sidecar`, and the members of a class whose whole name is a query word), then a deterministic Rust rerank in `queries.rs` credits each query term once from its strongest field (name whole token 3, name stem 2, the whole name of the member's class 3 when another word matches the member's name, so `Flask init` finds `Flask.__init__`, name substring 1, signature or docstring 1, and for a term that no field of the row holds, the row's module name 0.5, `path` in `--explain`), weighted by the term's rarity across the index (a capped FTS5 match count per term), and adds the whole-name and all-words bonuses (the whole-name bonus is 100 for definition kinds and 60 for every other kind) and the kind, path, documentation, and test priors, subtracts 2 for a function or method nested in a function, method, or constructor, and adds 6 (`field coverage` in `--explain`) to a function or method when one query word is a whole name token, one is in the signature, two different words are in the docstring, and the row matches at least half the query words; the rerank reads the first 400 bytes of a signature and the first 640 bytes of a docstring; `score` is that rerank score. For a query with two or more different words, when at least `min(limit, max(15, ceil(3 * limit / 4)))` rows match two or more words, a row that matches one word is dropped once that many rows that match two or more words rank above it, unless its name holds a rare query word. After this first rerank, search considers at most the first three non-document code types among the top 20 results and reads up to 40 direct members of each in source order with the same filters. It admits or enriches only members whose own name or first 400 bytes of signature or docstring match a query term. The selected owner's name and first 400 bytes of documentation can then credit otherwise unmatched terms at 0.5 (`owner_context` in `--explain`) before a final rerank; this step does not recurse or change the exact-name bonuses. The module names of a row are the names code uses for it (`module_names` in `queries.rs`): the file stem; the folder of a `mod`, `__init__`, `__main__`, or `index` file, a Lua `init.lua`, a Swift `main.swift`, or a Scala `package.scala`; for a Rust `src/lib.rs` or `src/main.rs` the `name` of the `[lib]` (`lib.rs` only) or `[package]` table of the `Cargo.toml` beside `src`, else that package folder, and for a Zig `src/root.zig` or `src/main.zig` the package folder, while another `lib.rs`, `main.rs`, `root.zig`, or `main.zig` takes its folder (`src/bin/tool/main.rs` is `tool`); for a Zig root source file of a `build.zig` module its module name, else of a non-test artifact its artifact name, in place of that convention; for a Swift file in the folder of a SwiftPM target the target name (`main.swift` takes only it); for a Go file its package clause (`package main` gives the folder) and its stem; for a Dart file the last segment of its `library` or `part of` name; for a `qmldir` file its `module` line; and the module that the row is a direct member of, by its last segment (`Post` for `MyApp.Blog.Post`): an Elixir, Ruby, F#, or Erlang module, a Rust `mod` that is not test code, a TypeScript or TSX namespace, or a Scala package object. A C#, C++, PHP, Java, or Kotlin namespace or package credits nothing, because it wraps whole files and code names the type in it. One query word names a module by itself or by its stem (`blog` for `examples/tutorial/flaskr/blog.py`, `view` for `views.py`), and a run of words names it when their letters spell it (`release corpus check` for `release-corpus-check.py`). No other folder above the file credits anything, and a test path or a path through a `fixtures`, `vendor`, `vendored`, or `third_party` folder earns no module credit. `docs/language-support.md` lists the rule and its regression per language. The path prior subtracts 10 from a row under a `scripts`, `examples`, `benchmarks`, `fixtures`, or `vendor` folder unless the query names that folder or the row's module. A module credit is not a matched word for the multi-word rule above or the digit rule below. When the query has a word that is not all digits, its digit words (`1` in `zzq1_thing`) stay out of the FTS5 OR query, the trigram name terms, and the class-name branch, and only the FTS5 AND query keeps them. After the final rerank, and before the multi-word rule above, such a query drops a row that credits no query word other than a digit and that no recall branch admitted (a member that the owner step admits for `= 1`). A row whose word FTS5 matched past the 640-byte docstring head stays, with no credit for that word. A query of digits keeps every row, and no score floor applies, since every document row carries the documentation prior. `lookup_symbol` lists exact names first, then names that start with the query, then names that contain it; inside each of these ranks definition kinds come first, then code rows before rows of data, configuration, and document languages (JSON, YAML, TOML, XML, INI, Markdown, HTML, CSS, plain text, `go.mod`, `go.sum`), and a `self.` attribute row after the other rows of its rank. A documentation link (Markdown and other document languages) is never an import row in lookup or search. In lookup and search text a member of a class or another code type shows as `Owner.name` (`ScriptInfo.__init__`), which the other tools accept as a name; JSON keeps the raw name. When some lookup rows only start with or contain the query, the header says how many are named exactly, and those other rows fold into one line of names, kinds, and counts; JSON keeps every row. A lookup that ends in `.` or `::` (`App.`) lists the members of the classes, modules, and namespaces with that name in source order. A Markdown heading with that name counts only when no code type has it. The `kind` filter of lookup and search accepts the stored kind or the kind the output shows (`attribute`, `section`, `link`, `element`, `table`). An attribute row whose signature starts with `self.`, `this.`, or `cls.` (except an F# member) shows the kind `attribute`, not `property`; JSON keeps the raw kind. An attribute row such as `self.debug = …` whose name a base class defines with a value or a property, or whose own class defines it that way in another row than such a write (`@property def static_folder`), is a write, not a definition: lookup, search, skeletons, and the name lookup of the other tools leave it out, and lookup still fills its limit. An R top-level row that julie writes for a global that a function creates (`counter <<- 0`, `0 ->> counter`, `assign("counter", 0, envir = .GlobalEnv)`) is such a write when a top-level R row of that name without this `assignment` metadata, or an earlier such row by path and line, defines the global; the callers report counts its line as a site. A base class named by a pending `extends` row counts only when it is not the subclass itself (`class EnvironBuilder(werkzeug.test.EnvironBuilder)`) and no other class of that name is as close to the subclass's file. `blast_radius` shows a test method as `Class::test_name`. Rows from test files are hidden by default: a path rule in `queries.rs` (`is_test_path` and its SQL mirror `test_path_predicate`) hides the whole file, not only the symbols `julie-extract` flags, and `is_test: true` / `--include-tests` shows them again. A `lookup_symbol` row whose name equals the query is shown either way. When an exact lookup finds a definition, the import rows with the same name fold into one line with the count and the first three locations, and they do not count toward the limit. Equal scores break by name strength, the sum over query words of 3 for a whole-token name match, 2 for a stem match, 1 for a substring match, and 0 for none, then names that do not start with `_` before names that do; `--explain` reports the name strength as `name_strength`. `code-kb search --explain` prints the breakdown and the rerank timer; `lookup_symbol` takes no `explain` parameter, and `--verbose` stays debug logging.
- Lookup fallback: `lookup_symbols_op` in `ops.rs` serves `lookup_symbol` and CLI `lookup`. When the query is plain (not a qualified name, and not a member listing that ends in `.` or `::`) and no listed row has the query as its name, it also runs the ranked search with the same filters and limit. It appends the ranked rows that the name rows do not show, under `No exact name match; N full-text matches for "q":`. A query that names a symbol exactly prints no ranked section. When the query is plain and one word and no listed row has the query as its name, the `name_suggestions` candidates of a miss print before the ranked section, as `Did you mean one of:` or as the definitions a path filter excludes; close names print there only when no name row is listed, these candidates follow the `kind` filter, and they leave out a name that a name row has. When both lists are empty, also after the rerank drops every ranked row, the answer is the `symbol_not_found_parts` text. The MCP tool and the CLI print the same text, and CLI `lookup --json` prints one object with `names`, `ranked`, `suggestions` (the candidate rows, left out when there are none), and, for a miss, `miss`. CLI `search` runs only the ranked search.
- Reference rules: a pending call whose receiver names an import binding in the caller's file matches
  a target whose path holds that module as a directory or file name (`flask.Flask(...)` matches
  `src/flask/app.py`). A relative import source (`../core/util.js`) names one file, resolved from
  the caller's folder with any script extension or an `index` file, and a receiver bound by such an
  import matches a class or namespace member only in that file. A bare pending call never matches a definition in another file when the
  caller's file defines that name at module level or inside the caller; an import or re-export
  there does not count. A `self`, `this`, or `cls`
  call matches a method of any ancestor class or interface, through same-file and cross-file
  `extends` and `implements` rows (a Java `default` method, a Kotlin interface method with a
  body), and a `self`, `this`, or `cls` call also matches a member of the caller's own class or
  object literal. A base that a pending `extends` or `implements` row names counts unless a type of
  that name is closer to the subclass's file.
  A member access whose receiver names a type-like target groups to one row per
  file with an `occurrences` count. An identifier row is dropped when a relationship row already
  covers the same site. An import alias satisfies a pending receiver unless the import source
  starts with `Qt`. An `extends` row never resolves to its own component. A `type_usage` or `member_access` identifier matches a selected target only in the same language, except among JavaScript, TypeScript, TSX, JSX, and Vue, between Razor and C#, and from QML to C++. A name-only `type_usage` does not match a type when a type with the same name is nearer to the use: nested in an enclosing scope, or at the top level of the same file. The grouped rows of a type-like receiver follow the same language rule and leave out a file that defines its own type with that name. A caller that julie names as a generated lambda shows as `<lambda> in <enclosing symbol>`, an anonymous class shows as `<anonymous Task in make>` (invariant 2), and a site with no caller shows as `(no enclosing symbol)`. A handler row is
  labelled `handler` when the receiver names the owner and `handler (candidate)` otherwise. Test
  paths include `/autotests/` directories and file names that start with `tst_`. A Qt C++
  header's `property` and `event` rows come from the extractor's macro pre-pass and render like
  any other member; a skeleton `event` row whose signature does not spell `signal` or `event`
  carries `// event` before its line range. Rows from one caller to one name at the same path and
  line merge into one row; of rows with one label, a call row wins over a type use. A method call on a local variable matches a method of the class
  that the call assigned to it builds (`runner = app.test_cli_runner()`), or of an ancestor. A
  call builds the class it names, or the class in its callee's return type. A method call on a
  parameter named for a pytest fixture in the test's file or in a `conftest.py` in its
  folder or a folder above it matches the same way, through any call in that fixture. A local
  variable with that name does not borrow the fixture's type. Builders and their returned classes
  respect the importing module, and inherited methods use the nearest definition. A `self`
  call to a class attribute (`should_ignore_error: None = None`) is a callee of the caller.
  The callers of a constructor include the calls that build its class (`Flask()` calls
  `Flask.__init__`), and the `blast_radius` walk follows the same step. Rows it reaches only
  through that step sort after the other rows. A symbol in a test file that julie does not flag
  as a test or fixture counts as a test to run only when it is a member of a test class or its
  name reads like one (`test_run`, `RoutesTest`); an app factory such as `create_app` is an
  impacted symbol.
  A Rust path call (`crate::util::f()`, `my_crate::module::f()`) matches a top-level function when
  a leading `crate`, `self`, or `super` names the caller's own package or a leading crate name names
  the package folder (`-` read as `_`), and every other segment is a file stem or folder below that
  package's `src/`; a call rooted at `std`, `core`, or `alloc` never matches.
  A `super` call matches a method of an ancestor, and a `self`, `this`, `cls`, or `super` call
  matches only the nearest class in the chain that defines the name.
  A receiver chain (`a.b.f()`) starts at one of three roots: a typed value in the caller's scope;
  the caller's class for `self`, `this`, or `cls` when that name has no type fact and the caller is
  a method, constructor, or property of the class; or the type that the caller's scope names
  (`Outer.Inner.build()`, proven as `qualified path`). Each later name is a member with one type
  fact (a field, property, variable, attribute, or constant), or, when no member has the name, one
  nested type, or an enum constant without a body, which steps to its enum where enum constants are
  values of the enum. A chain whose first name binds neither a value nor a type (a package or
  module, `app.Outer.Inner.build()`) keeps the name-path rules, and a chain that stops at a member
  with no type fact stays unproven. A chain does not exclude a target in a companion object, an
  enum-constant body, a partial or reopened part, or an extension of its owner.
  A bare call matches a class
  or function defined inside the caller, and a definition in an enclosing function, namespace,
  or module scope, where the nearest scope that defines or imports the name wins. In Java, C#, Kotlin, Swift, C++, Scala, Dart, Ruby,
  and VB.NET a bare call also matches a method of the caller's class chain, where the nearest
  class that defines the name wins, a method of another part of the caller's class in the same
  file (a C# partial class), and a type nested in an enclosing class. A bare Java call reaches
  nothing else but a static import (`import static C.name;` or `import static C.*;`), so a call
  of a method whose class is none of the caller's classes, the classes enclosing it, their
  ancestors, or a class it imports statically is excluded with the basis
  ``a bare call reaches only the classes of `Caller` and its static imports``. Static-import
  owners are considered together; competing owners keep a target call unresolved rather than
  choosing by import order. An ancestor that
  is not indexed does not change this, since a library class cannot extend a project class. An
  annotation element (`alternate = {…}`) is not a call, and julie records no supertype for an
  anonymous class (it hangs the methods on the field or method that builds it), so the rule
  excludes neither. A member-access identifier never matches a function
  nested in another function, a private module-level name of JavaScript, TypeScript, TSX, JSX,
  or Vue in another file, or any target when its receiver is a capitalized name that no indexed type, module,
  import, field, or property has (`Assert.Contains`); a QML signal handler is exempt. Callee rows
  come in source order. `find_references` for callers ends with one line that counts the files
  that import the target; a relative JavaScript or TypeScript import counts only when it resolves
  to the target's file, so an import of a mirror copy (`deno/lib/`) does not.
  Ordinary member-access identifiers targeting callable class members use the same receiver
  evidence as pending calls. A proven different receiver is omitted, including an imported base
  instance used to query a subclass override. Supported rows retain `member_access`; unresolved
  or conflicting evidence shows `member_access (candidate)` in text and JSON. Supported identifier
  rows precede candidates, with filtering before the limit. Candidate-only identifiers add no
  impact edges. Fields, properties, type usages, and QML handler rules keep their own behavior.
  julie writes no receiver for `super().x` or for Bash, CSS, HTML, Vue, and XML names. Such a
  member access is a candidate for a class method, is dropped for every other target, and is
  dropped from a documentation language. A relative Python import of a package (`from . import
  Flask`) matches any file under that package when a receiver's class is checked, and an import
  that comes before a same-named class in the same file does not hide that class.
- Callers report: `find_references` for callers (CLI `refs`) comes from `reference_report` in
  `reference_report.rs`. It lists every indexed site of the name (pending calls, relationships,
  and identifiers; never an import row or a definition line, and a definition line includes its
  decorators) and gives each site one label and a basis: `proven` (index evidence ties the site to
  the target: same file, qualified path, receiver type, class chain, import, enclosing scope, same
  package, builds the class), `likely` (only the name ties them, and no other definition can reach
  the site), `unresolved` (another definition can reach the site, and the index cannot choose), or
  `excluded` (evidence ties the site to another definition, to an unindexed import, to another
  type, or to another language). A site counts only the other definitions it can reach, and the
  target's own overloads never count. A row of a document language is no definition, except a CSS
  custom function, which every loaded stylesheet can call. A site never reaches a definition in a
  language that the shared call rule keeps apart from its own (a Java call counts a Kotlin method,
  never a TypeScript or C# one). A call never reaches a Go, Java, or Rust field, property,
  variable, or constant, except a Java record component, a Go field or variable with no written
  type, a qualified type, or a func type (written, or a project `type X func`, alias, or generic
  form), and a Rust static, constant, or variable of a function type (written, or a project type
  alias of one). A Java field, variable, or parameter never hides a method from a call, so it
  never excludes the call. A member access never reaches a JavaScript function, variable, or constant that
  is not exported, not in a class, not a local, and not used as a member or placed in an object
  literal (`{ render }`) in its own file. An `unresolved` basis `N definitions` counts the target
  and the definitions that can reach the site. The other definitions of the name
  are checked nearest first, at most 12; the answer counts the ones it did not check. The checks
  of one report stop after 3 s, and the target's own checks stop after half of that, nearest
  sites first. A site left unchecked is `likely` when no other definition can reach it and
  `unresolved` otherwise, with the basis `not checked within the time limit`, and the answer
  counts those sites. A whole-word text scan with a 3 s budget sorts every line of the name into a site, a
  definition, an import, or text only. A definition runs from its first decorator line to the line
  before its body in Python, Ruby, Lua, F#, and VB.NET, where julie starts a body at its first
  statement, and to the first line of its body elsewhere. When no line before the body holds the
  name (`@route` then `def index(): return 1`), the span keeps the first body line. An import row that julie names by its
  full path (Kotlin `import a.b.boxIfPrimitive`) counts for its last segment. Counts are always
  complete, `limit` caps only the listed rows, and CLI `--json` prints the whole report. Callees
  keep the plain list.
  Proof rules: a package import proves a site only when it reaches no other top-level definition
  of the name (`import * as z from "zod"` reaches every `boolean` in the package, so `z.boolean()`
  is unresolved). A module receiver never proves a class member, and a Python receiver chain that
  names a module of the project (`flask.json` after `import flask`, or `json` after `from flask
  import json`) reaches none, so a method of that name is no other definition of the site. A
  C# type or constructor is no other definition of a bare C# use unless its namespace is the
  global one, holds the use, or is opened by a `using` of the use's file or by a `global using`,
  and a bare C# use of a top-level type is excluded on the same terms when no `using Name = …;`
  alias names it: ``namespace `App.Utilities` is not open here``. A bare C# use of a target that
  julie kept from an `#if !X` branch (`preprocessorIf`) is `unresolved` when the use's file always
  opens a namespace that another file opens under `#if X`, since the file builds against the
  namespace that defines the name where `X` holds: ``ConcurrentDictionary` is defined only without
  `HAVE_CONCURRENT`, and this file always opens `System.Collections.Concurrent`, which other files
  open with it``. A file that opens it under `#if X` keeps the use, which a build without `X`
  needs. `check_edit` reads the condition of a removed target from its base scan. A `.csproj`, `.props`, or
  `.targets` file with an MSBuild `<Using …/>` item opens every namespace, since the index does
  not see it. In C#, Java, Kotlin, Scala, Swift, Dart, Rust, Go, C++, C, and VB.NET a place that names a
  type (a base type, `new X()`, a written type, or the type of a C# member) never names a field,
  constant, variable, property, parameter, or enum member, so none of them is its other
  definition or excludes it. A
  Python absolute import proves
  only a file whose top package (the outermost folder with `__init__.py`), or a plain folder
  above it, is the first segment of the import (`import json` does not reach `src/flask/json/`),
  so a receiver bound by an absolute import that reaches no project file (`_json.dumps()` after
  `import json as _json`) is excluded: ``receiver `_json` is imported from `json` ``. An import
  source that names a project file or folder by its path is no outside import, even when its
  first name is `std`, `core`, or `alloc`: Zig `@import("core/math.zig")` and `@import("root")`,
  Lua `require("core.utils")` (`core/utils.lua` or `core/utils/init.lua`), QML `import
  "core" as Core`, Dart `import '…' as s`, and GDScript `preload("res://…")`; a source written
  from the importer's folder (`../lib/widget.zig`) never is. `reference_report/loaded_files.rs`
  resolves the `source` and `form` that julie writes for C, C++, CSS, Bash, PowerShell, R, Ruby,
  F#, Dart, PHP, and GDScript as each language reads the path, and a path built from runtime state
  (`$DIR/env.sh`, a PowerShell module name) proves nothing. A row that only loads a file or opens
  its names (`#include`, Bash `source`, Ruby `require`, PHP `require`, F# `open`, a Java star
  import) binds no receiver. An F# `open Make` still brings the members of module `Make` into the
  file's scope: a bare call there of a function of that module is proven (`import`), and the walk
  follows it. A Dart import prefix reaches only the libraries it names and what
  they export; a GDScript `preload` constant reaches its file's class and that class's bases; an
  R `pkg::f()` reaches only the `R` folder of the project package `pkg`, else it is outside. A bare
  Bash, PowerShell, R, PHP, or Ruby call of a top-level function that a file the caller's file
  loads defines is proven (`import`), and another top-level definition is excluded (``the file
  loads `greet` from `sh/lib/common.sh` ``), only when every load resolves and nothing else loads
  the caller's file. A bare Dart call sees its own library and its parts first, then what its
  imports without a prefix bring in; a conditional import or a Dart method of that name decides
  nothing. In C and C++ an included header's declaration is proven, the definition in
  the source file with that header's stem is likely, and a `static` definition in a file that is
  not included is excluded. A C or C++ site sees only its own file, the headers it includes
  (directly or through other headers), and the files that include its file and what they include
  (`reference_report/include_reach.rs`); a source file that no file includes is a root, and one
  that a file includes (a template `.cpp`, a unity build) is not. A definition in a header
  outside those files is never proven, never the definition a site `resolves to`, and no other
  definition of the site; when every definition of the target is outside them, the site is
  excluded: ``this file includes `binary_reader` from `include/nlohmann/detail/input/binary_reader.hpp`,
  not from `tests/abi/include/nlohmann/json_v3_10_5.hpp` `` or ``this file does not include `x.hpp` ``.
  The rule leaves a site alone when an include does not resolve or matches more than 8 indexed
  files, a header on the way has no indexed includer, the walk passes 16 steps, 2048 files, or
  512 includers, or runs out of its time limit, or a file holds an include that julie did not
  record (`#include CONFIG_H`, `#import`, `#include_next`); and it leaves alone a function or
  variable in a source file (linking), a definition that a bodiless declaration of the same kind
  and name in reach can complete, and a type that a file in reach names by its tag (`struct S;`,
  `class S *p`). The `blast_radius` walk drops a step that the rule rules out, and a C or C++ call
  qualified only by project namespaces (`detail::input_adapter()`) steps to a top-level function
  or type, since julie records no namespace that a macro opens, unless every such call of the
  caller names a namespace that defines the name in a file the caller sees (`util::helper()`).
  `docs/language-support.md` (File-path imports)
  has every rule. A Java
  receiver imported from a package that no project folder holds (`Assert.assertEquals()` after
  `import org.junit.Assert;`) is excluded the same way; julie's `source` names only the package,
  so the package is the segments of the line before the first capitalized one.
  A JavaScript or TypeScript read of the name that only scope ties to the target is likely,
  because julie does not record callback parameters. A bare name never reaches a JavaScript
  method assigned to a property (`req.accepts = function`) or an object-literal method, which
  julie records with no owner, and an import never reaches either one, so `z.array()` does not
  reach `{ array() {…} }` beside `export function array`. A call that julie links to its own
  caller is not proof, because julie links a call inside a method to that method by name alone. A
  call through `super`, `base`, or Kotlin `super<X>` reaches an ancestor of the caller's class,
  never that class. Java and Kotlin link `super.f()` in an override to the override; when the
  call's identifier has such a receiver, the report drops a link into the caller's class or an
  enclosing class and judges the call by its receiver, so the nearest ancestor that defines the
  name proves it (`class chain`). A target whose owners are no ancestor of the caller's class is
  excluded: ``receiver `super` reaches only the ancestors of `Sub` ``. A subclass write to an
  inherited attribute (`self.debug = …`) is a site, not a definition, and does not hide the base
  class's definition in a class chain. An ordinary Python class-body annotation without a value
  is not a runtime binding, so it cannot exclude another provider; without receiver proof, the
  site stays likely or unresolved. Dataclass and TypedDict annotated fields retain their existing
  member semantics. A C# extension method counts the type of its `this`
  parameter as an owner, so a receiver of that type does not exclude a call. A Go receiver type is read
  without its pointer and type parameters, and through a project alias (`type Local = geom.Point`,
  up to four hops) to the type it names. A package name (`geom.Point`) is dropped when it can name
  an owner's package: an import of the site's file with that name whose path ends in the owner's
  folder or package name, or, with no such import, that folder or package name itself. So a
  receiver of the owner, of an alias of it, or of a type that embeds it does not exclude a call,
  and `bytes.Buffer` still excludes a project `Buffer`. A receiver declared
  with a type that can hold a value of any type never excludes a site (the basis ``receiver `k`
  has type `any` `` is not given): TypeScript `any`, `unknown`, and `object` (a check narrows the
  variable before the call), Python `Any` and `object`, and C# `dynamic`. A Python `Any` counts
  when written through its module (`typing.Any`, `typing_extensions.Any`, `t.Any` after `import
  typing as t`) or imported under another name (`from typing import Any as AnyT`); a project
  TypeScript alias of `any`, `unknown`, or `object` (`type Anything = any`, followed through four
  aliases) counts as that type. A C# `object`, a Java `Object`, and a Kotlin `Any` keep excluding.
  A receiver declared with a type parameter never excludes a site either, since the parameter
  stands for any type that fits its bound: a type parameter of the caller, of a function that
  encloses it, or of a class or interface that encloses it (Java `<T extends Greeter>`, C#
  `ViaT<T>(T x) where T : IGreeter`, TypeScript `<T extends …>` on a function, method, class, or
  interface, Kotlin and Rust `<T>`, C++ `template<typename T>`, VB.NET `(Of T)`), and a Python `TypeVar`. The lists are read from the
  signatures, from julie's `typeParameters` of a TypeScript function, and, for a TypeScript class
  or interface, from the first 600 bytes of its source.
  julie names a test call after its label (`test("strictObject", …)`); such a label is not a
  definition, and when the target is a label every site is excluded: ``res` is a test label,
  which code never names``. A function defined inside a Python, Rust, Kotlin, C#, Swift, Dart,
  F#, or Scala function, or in the initializer of a property, field, or variable, a class, struct, enum,
  interface, trait, union, or type alias defined inside a Python, Rust, Kotlin, Swift, Scala,
  Java, C++, C, Go, or Zig function, method, or constructor, and a TypeScript, TSX, or Vue
  interface or type alias defined inside a function, is local to it, so a site outside that
  function is excluded:
  ``read` is local to `test_session_cookie_setting``; such a function or type is no other
  definition of a site outside that function either. A member of an anonymous class is never
  local to the function that creates the object, since a caller reaches it through that object.
  julie names the class around a value as the caller of a call in its initializer; the call
  still binds to a function that the initializer defines (`enclosing scope`). A Java member
  access, written without
  parentheses, never reaches a method, because julie writes a method reference (`parser::next`)
  as a call: ``a member access reads a field, not the method `length` ``. A bare Java
  call is excluded with the basis ``name` is imported from `java.lang.Math`` when its file has
  `import static C.name`, no definition of the name is in the caller's file, and C and the
  caller's class chain own none. When C names the target's class file (`p.util.Checks` for
  `p/util/Checks.java`) and the caller's file defines no method of that name, the import proves
  the call, also for a removed target in `check_edit`. A bare site that a
  nearer definition shadows cites the definition nearest above it in its scope, because julie
  does not record block scopes, or in JavaScript and TypeScript the callback, loop, or catch
  parameter that binds the name when it is nearer: `line N binds the name as a callback
  parameter, loop variable, or catch parameter`. A pending call takes the line of a call
  identifier only inside its caller. A receiver that names an object (`z.coerce.boolean()`) ties the call to a member of
  that object only when the qualifier's import (`z`) reaches the object's file. julie records no
  receiver for a call on a call result (`single().getAsInt()`) or on a string literal, so a call
  whose name follows a `.` after a call result, an index result, or a closing quote, a call on a
  line that starts with `.` (`    .readonly()`), and a call that starts the line below such a `.`
  are never bare calls: scope, class chain, and julie's same-file link neither prove nor exclude
  them. When such a receiver is a call result, an index result, or a string literal (on the line,
  or at the end of the line above a line that starts with `.`), or a `const` of the caller's scope
  or file whose nearest declaration above the call holds a call or a `new` object (`const schema =
  z.object({…})`), and it loads no module (`require(…)`, `import(…)`), the call never reaches a top-level JavaScript or TypeScript function, variable, or
  constant that no object holds: one never written as `object.name` in its file and never placed
  in an object literal there or in a file that imports the name. Such a target excludes the site
  (``a call on a call result never reaches the top-level `refine` ``), and such a definition is no
  other definition of it. A JavaScript call whose receiver names the top-level object that the target is assigned
  on in its file (`res.redirect()` for `res.redirect = function`) is `likely` with the basis
  ``receiver `res` names the object `redirect` is assigned on`` when no other definition is
  assigned on an object of that name. A member of a variable or constant that a function,
  method, or constructor holds, or of a property of its object literal (`var res = { send() {…} }`
  or `var ctx = { res: { send() {…} } }` in a test), has an owner that only that callable names:
  a receiver or qualifier outside it that names the owner never proves the site for that member
  (`qualified path`), so the member excludes no site there. It still counts as another
  definition of a site it can reach by name, since a local object can be returned or passed on.
  JavaScript and TypeScript imports go through the resolver in `reference_report/js_modules.rs`. It
  follows `export … from`, `export * from`, and namespace re-exports to a relative file (any
  script extension or an `index` file; a JavaScript importer tries the JavaScript extensions
  first, so it takes the compiled `.js` twin of a `.ts` file) or to a workspace package by its `package.json`
  `exports`, `module`, `main`, and `types`, scripts before declaration files, and it takes the
  next candidate when a walk reaches nothing. `require()` follows the `require` condition and
  `main`, never `module`; an ES import follows the `import` condition, `module`, and `main`. A
  package with no entry for the importer falls back to its `index` file. `types` comes after the
  scripts and never counts as a second script; a package that still names two scripts proves
  nothing. `exports.name = …` and `module.exports.name = …` count as exports. A destructured
  dynamic import (`const { f } = await import('./m')`), which julie stores as a local variable,
  is a binding, not a definition: the resolver follows its module like an ES import, and its
  line counts as an import. The nearest
  `tsconfig.json` or `jsconfig.json` (`paths`, `baseUrl`, and relative `extends`, one or a list;
  `${configDir}` is the folder of that config) decides before the workspace packages. A `paths`
  key whose targets do not exist on disk falls through, as in TypeScript; a target on disk that
  is not indexed leaves the module unknown, and so does a workspace package when the config
  cannot be read. A declaration file
  and the script with its stem in the same folder are one module. The resolver proves a site when
  every definition it reaches is the target, gives `unresolved` when it reaches the target and
  others, and excludes the site when it reaches only others. An import it cannot follow (an
  alias, an unindexed file) takes the import rules above. A `node:` source, a bare Node builtin
  (`fs/promises`), and a package whose last segment names no project folder or file are outside
  the project: the site is excluded, and no import claim proves a project definition. A test
  label (`describe('mix', …)`) never hides an import. A bare JavaScript or TypeScript type use (`output<T>`) is hidden by a type, interface,
  or enum of that name in an enclosing function, and never by a value of that name (the `const`
  half of an interface and a `const` with one name). A Rust read of a value never names a module
  (`mod git;`): such a module is no shadow and no other definition of the site, and as the target
  it excludes the site with ``git` is a module, which a read of a value never names`. julie records no symbol for a callback
  parameter, a `for … of` or `for … in` variable, or a `catch` parameter, so
  `reference_report/js_bindings.rs` reads them from the source: when one of them between the
  start of the caller and the site has the name, an `import`, `same file`, `enclosing scope`, or
  `qualified path` proof becomes `likely` and names that line. When that binding's body is a
  block that holds a bare site (`catch (e) { … }`, `(e) => { … }`, `function (e) { … }`,
  `for (const e of list) { … }`), the site reads the binding and is excluded with the basis
  `line N binds the name as a callback parameter, loop variable, or catch parameter`; an
  expression body (`(e) => e()`) and a loop body without braces keep the rule above.
  A JavaScript or TypeScript object shorthand (`{ name }`) can read a local parameter or
  variable and write a member at the same token. In supported return, initializer, and
  constructor expressions, a selected field, property, or interface method stays unresolved
  when only the local binding would exclude it. Its owner must be an indexed code type; a
  concrete class method keeps the ordinary rules. Stronger exclusions still apply, the local
  value keeps its own reference, and the candidate adds no impact edge.
  Reach sections: `reach_sections` in `reference_report/sections.rs` adds up to six sections
  after the sites are labeled: `Overrides`, `Overrides of`, `Constructions`, `Variant uses`,
  `Registrations`, and `Tests that call or use it`, where each test has a run command from
  `test_commands.rs` or no command when no runner can run it alone. They print after the site
  groups and before the text-only lines, each only when it has rows. They never change a site's
  label or basis or a count, so the reach gate scores the same labels. `limit` caps the listed
  rows of a section. CLI `--json` adds the arrays `overrides`, `overridden`, `constructions`,
  `variant_uses`, `registrations`, and `tests`, and leaves out an empty one. A section reads
  indexed rows and opens source only for the files it needs. Every rule of each section, its
  known limits, and the run-command forms are in `docs/reach-sections.md`; change that file
  with the code.
  Argument counts: julie 3.8.0 writes `arg_count` on call identifiers (and `arg_spread` for a
  spread or splat). A site shows `; N args` after its basis, and `--json` adds `arg_count`.
  `reference_report/arity.rs` excludes a `likely` or `unresolved` call only when the language
  has a supported strict argument rule and its count fits no definition of the target
  (never JavaScript, never a spread): ``3 args; `render` takes 1-2``. A `proven` site never
  changes. The range comes from the parameter rows checked against the signature, counts a
  method's receiver where a call can pass it, and joins the ranges of the same-named methods of
  every base type, so a call through a base reaches an override. Every rule and every shape with
  no range are in `docs/argument-counts.md`; change that file with the code.
  Overloads: `reference_report/overloads.rs` ties a call of a target with overloads (same name,
  owner, kind, and file) to the overloads its call identifier's facts allow, in Java, Kotlin,
  Scala, C#, C++, Swift, and VB.NET: each overload's own count range (a method's widened by the
  methods of its name in the base types), Kotlin and Scala `arg_names`, Swift `arg_labels`, and
  `arg_types`. A type rules an overload out only by a certain Java, Kotlin, or Scala rule (`int`
  never fills `String`); an overload whose parameter types name the argument types binds alone,
  except that a Scala one that needs a default loses to an overload that applies without one.
  Two written types are compared by the type they name (`type_names.rs`): `App.Box` and `Box`
  can name one type, and in Java, Kotlin, Scala, C#, and VB.NET a simple name is read by the
  scope rules of the language through the imports, package, namespaces, and build project that
  the index records, as a project type, a library type (`library_types.rs`), or unknown. Where
  julie writes a Kotlin, C#, or Swift nullable value without its marker, the call binds the
  exact overload or one that takes that value, unless the argument's source shows a value that
  cannot be null (`plain_values.rs`: a literal, a C# `new` object, a name declared with a plain
  type). A C# `dynamic` argument binds at run time; a VB.NET `Object` argument binds an
  `Object` overload. A Swift argument
  with no label and no type can be a trailing closure, so it can fill a labeled parameter that
  can take a closure. A site of any label whose call binds only other overloads is `excluded`:
  ``arguments `(String)` bind only `run(String)` ``, and that basis wins over another exclusion
  of the same line. A call that fits no overload, or that the facts cannot decide, keeps every
  overload it fits. The rules are in `docs/argument-counts.md` ("Overloads").
- Report recovery: `find_references`, `blast_radius`, and `check_edit` accept `offset` and
  `limit`, with equivalent CLI `--offset` and `--limit` flags. Each displayed section uses its
  own page window; counts describe the complete report subject to stated discovery/check cuts.
  The first callers page lists up to `limit` rows of each label and of the text-only lines in
  report order, so `offset` = `limit` continues every section and no row shows twice.
  Page sizes are at most 200. Restart at offset 0 after editing or rebuilding the index.
  `blast_radius` can show discovered low-signal rows with `include_low_signal` /
  `--include-low-signal`; its graph discovery ceiling remains 200. Incomplete test coverage
  names a broader command only when repository configuration identifies a runner, otherwise
  it names the broader scope to verify. Candidate-only references still add no impact edge.
- Related tests (`get_symbol_context`, after the report's tests in `### Tests to run`): callers from relationships, pending calls, and references
  that are tests, test classes, or functions and methods in test files that are members of a test
  class or have a name that reads like a test (`names_a_test`), so a module-level app factory such
  as `create_app` is not a related test. A helper function defined inside such a test counts as
  that test (`enclosing_test`) unless that test is setup; never the target itself, never setup, teardown, or
  fixture members, and none for a target in a document language. A constructor gets the related
  tests of its class. The name and full-text stages run only when the target's name has one
  definition in the index and is not a dunder name. `member_access (candidate)` rows do not enter
  through the reference stage; independent name and full-text test suggestions still apply.
- Language-agnostic callee filtering: `find_references(direction="callees")` and `get_symbol_context`
  filter unresolved AST tokens against workspace symbols, eliminating external stdlib/runtime noise
  across supported languages by default (`include_external: true` / `--include-external` restores them).
  A callee row names the definition the call reaches (``→ `AppContext.push` (src/flask/ctx.py:416)``),
  or up to three candidates, or says that more than three definitions have that name.
- Blast radius & test prediction: `blast_radius` (alias: `impact`, CLI: `code-kb blast-radius` / `impact`)
  computes multi-hop reverse reachability and predicts targeted tests to run. The walk goes level by level: one
  SQLite query per level finds the callers of the symbols that the level before added, so a symbol that
  several paths reach is expanded once, and the depth-5 count and the open-end check below reuse the same walk.
  A JavaScript call that julie stores only as an identifier (`res.send()`) is a caller step when its receiver
  names the object the target is assigned on (`res.send = function`), or is `this` in a function assigned on
  that object, and no other definition is assigned on an object of that name.
  Auto-discovers uncommitted git changes when no target is passed and walks from the innermost symbol
  that holds each changed line, or from every symbol of the file when a hunk touches no function, method,
  or type (a changed import or module-level constant); outside a git repository that
  call is the `check_edit` error `not a git repository: <root>` (MCP `isError`, CLI exit 1). A caller at depth 1 is a
  `direct caller`, a deeper one an `indirect caller [depth N]`. A caller defined inside a test
  (a view function in a pytest test), also as a method of a class defined inside it or of an
  anonymous class that a local variable holds, is replaced by that test, ``encloses `index` (direct caller)``, unless the test is setup or a fixture. A member of an anonymous class steps to the symbol that creates the object, or to the function around a local variable that holds it, with the reason ``creates `<anonymous Task in make>` ``, and the walk goes on to that symbol's callers. A caller that julie flags as a
  pytest fixture is replaced by the tests that take it as a parameter, by the name in its
  `@pytest.fixture(name=...)` when it has one: in its file, or under the directory of the
  `conftest.py` that defines it. A fixture that takes that fixture is replaced by the tests that
  take it in turn. A setup member in another test framework, such as an xUnit test-class
  constructor or a Python `setUp` or `tearDown` method, is replaced by the tests of its class. A
  fixture or setup member with no such tests stays, labelled `fixture` or `setup`. A test, fixture, or setup member among the seeds counts the same way: the target (`the target is a test`), every one in a target file (`in the target file`), and, with no target, the ones whose lines the uncommitted diff touched (`changed in the diff`; a hunk that only removes lines touches the line before them). A seed is a test only when julie flags it as a test, a test container, a fixture, or setup, or when it is a function or method whose name reads like a test in a test path; another member of a test class, such as a helper method or a data class, is walked like other code. A test in a seed file that the walk reaches from another seed is listed like any reached test. The diff comes from `git diff HEAD` and the untracked files, with paths relative to the root. Every other test row is a test that the call graph reaches: a caller, a fixture user, a test of a setup member's class, a test that encloses a caller, or a test that reaches the target through a constructor. When the walk finds no test for one symbol, the rows are the `Tests that call or use it` of its callers report, with the reason `calls it (find_references)`, or `calls it by name only (find_references)` when only `likely` sites with the basis `name only` reach the test; a test container the walk does not start from (a Rust `mod tests`) is then its own row, `the target is a test`. When the walk lists tests but no impacted symbol, the report's tests that the walk did not list follow the walk's tests with the same reasons. For a diff, the tests in the callers report of each function, method, or constructor outside tests that holds a changed line follow the walk's tests the same way, when at most 5 hold one, and such a function whose report has a `proven` or `likely` site is no open end; the callers of its report that hold no changed line join the impacted symbols as for one symbol. For a file or folder target, the callers report (without the text scan) of each function, method, constructor, and type of it outside tests and test files, and of each variable or constant at module level, runs in line order until 3 s from the start of the call pass; the symbols stream from one query one at a time, and each report checks its sites only for the time that is left. So a test that reads a decorated command (`run_command.make_context()`), passes a module-level object (`runner.invoke(cli)`), or names a class as a value (`cls=FlaskGroup`) is listed with the reason ``calls `run_command` (find_references)``, after the walk's other tests and never in a constructor fold; a test that only `name only` sites tie is left out, a symbol whose report has a `proven` or `likely` site is no open end, and the answer counts the symbols left unchecked, which the query counts without loading them (`find_references did not check N symbols of the target within the time limit`, `report_symbols_unchecked` in JSON). The callers of those reports' `proven` and `likely` sites outside the file or folder join the impacted symbols as for one symbol below (a method read as a value in another file, `return [app.handle_error]`), and the walk goes on from them; a call of an overload keeps its caller, since every overload of a group is in one file and so is a seed. A file or diff target also counts the symbols whose report failed (`find_references failed for N symbols of the target`, `report_symbols_failed`), and a file, diff, or symbol target counts the sites that a report did not check within its time limit (`find_references did not check N sites within the time limit`, `report_sites_unchecked`); each of these counts adds the broader test scope. The walk follows calls, never a read of a member (`cls.handle_user_exception` without a call). So for one symbol its callers report (its labels without the text scan) runs when the walk finds no test or no impacted symbol, or when a member read of the target's name lies outside the symbols the walk listed: each caller of a `proven` or `likely` site that is not a test and that the walk did not list is a depth-1 row after the walk's depth-1 rows (a caller that the walk listed deeper moves to depth 1, and the walk goes on from it again; when the walk found impacted symbols, a `likely` member access of a function, method, or constructor with the basis `receiver not typed` or `name only` is a candidate and adds no row, and for a target with overloads a call adds no row, since the walk drops a step from a caller whose every call of the name binds only other overloads (`OverloadSteps` in `overloads.rs`) and keeps the others for each overload they fit) with the reason `direct caller (find_references)`, or `direct caller (find_references, name only)` when only `name only` sites tie it, and the answer says how many `unresolved` sites the index could not tie to the target. The walk then goes on from those callers with one level less depth, and its tests, impacted symbols, open ends, and deeper-test count join the answer one level deeper (`direct caller` becomes `indirect caller [depth 2]`); a symbol that both walks list keeps the smaller depth, and the rows stay in depth order. `No downstream callers found within depth.` stays only when the report has no site, and for a file or git-discovered seed when neither the walk nor its reports find a caller; JSON adds `reason` to such rows, and `callers_report` with the site counts when the walk found no impacted symbol. The index guesses no test from a shared word in a name, a stem in a file name, or an error handler, and it adds no `possible` row for a call that the runtime makes. When a caller lies past the depth (the walk looks one level further, up to the largest depth, 5), a second walk to depth 5 counts the tests it adds (`tests_past_depth` in JSON). A list then ends with ``The walk stopped at depth N. A `depth` of 5 lists M more tests.`` when M is above 0, and a walk that finds no test says ``No test reaches it within depth N. A `depth` of 5 lists M tests.``, or `No test reaches it within depth 5. Run the full test suite.` when M is 0; the count is `at least M`, and JSON adds `tests_past_depth_cut`, when that walk reached the 200-row ceiling. A walk with no caller past the depth that finds no test says `No test reaches it through the index. Run the full test suite.`; a list that `limit` or the 200-row discovery ceiling cut says neither. An open end is a function, method, or constructor that the walk reached and that is not a test (the target, a symbol on a line the diff touched, a symbol of a target file, or an impacted symbol) when no caller step of the walk leaves it except a call to itself, such as `App.__call__`, which the runtime calls. When the depth-5 walk runs, the open ends come from it, so an open end past the depth warns even when that walk lists no more tests. The answer lists up to three under `### Open Ends` with their kind and location, says that the list does not show the tests that reach the target through them, and ends with `N more; pass offset 3 to list them.` when there are more; a page at another `offset` lists the open ends of its window like the other sections. JSON adds `open_ends` with every one. The target is no open end when its callers report has a `proven` or `likely` site. `scripts/bug_replay.py` measures these warnings against the tests that planted bugs break (`evals/bug-replay/README.md`). For a file target, the tests in one test file that reach the target only through a constructor fold into one row for that file, `N tests reach it only through a constructor`; a file with one such test lists it. When `limit` cuts a list,
  the answer says how many rows it shows of how many it found.
- Edit check: `check_edit` (CLI `code-kb check [--base REV]`, `edit_check_op` in `ops.rs`) compares the
  working tree with a base revision (the given `base`, else the session base, else `HEAD`) and lists each
  symbol whose shape changed: removed or renamed, its parameters or signature, the fields of a class,
  struct, or record, or the variants of an enum. Each change lists the sites the diff reached, the
  `proven` and `likely` sites it did not reach, a count of the `unresolved` ones, and the tests to run:
  the tests that call a changed symbol, then the tests that the `blast_radius` walk reaches from the
  callers of its `proven` and `likely` sites, up to depth 5. The check keeps 50 commands. A test that no
  runner the project configures runs alone keeps its place on its own line (`Tests with no run command`),
  and the check keeps 50 of those. A line after them gives the count and each reason the list can be
  incomplete (tests with no run command, the walk's ceiling, depth, or open ends) with the broader test
  command. The Stop hooks skip that walk, since their reason lists no tests.
  `code-kb check` exits 0 with or without such sites and 1 on an error; with `--fail-on-proven` it
  exits 3 after the report when a change has a `proven` site outside the diff. MCP `check_edit` has
  no such option.
  A Java, Kotlin, Scala, C#, C++, Swift, or VB.NET call outside the diff that binds another
  overload of the same owner by an exact match (julie's `arg_types` all known and equal to the
  parameter types they fill, Kotlin and Scala `arg_names` by name, Swift `arg_labels`, omitted
  parameters with defaults) is no site of a
  parameter or signature change. Custom type namespaces stay distinct; only explicit framework
  aliases such as `System.String` normalize to keywords. With typed overload evidence, an
  accepted argument count alone never drops an unbound call.
  Required named parameters are checked before accepting an argument count where the language
  reader can establish binding. Missing names keep the site outside the diff; ambiguous source
  or a spread that could supply them stays unresolved. Removed indexed members include reads
  tied to their owner. A callback value is an advisory dependency when the callable contract
  changes, since its invocation is unknown; a removed callable value can be a proven dangling
  reference. Competing overloads and receivers retain their uncertainty.
  The base symbols come from a `julie-extract scan` of the base content of the changed files into
  `<root>/.code-kb/check/<pid>/`, removed after the call; the main index changes only by the refresh
  of the changed files. A check covers at most 200 files and 30 symbols and starts no step after 20 s, and the
  answer states each cut. `code-kb hook SessionStart` reads the hook JSON on stdin and writes
  `<root>/.code-kb/sessions/<session_id>.json` with `{"base": "<HEAD sha>"}` when the root is a git
  repository with a commit, with or without an index; a file that has a base keeps it, and files
  older than 7 days are removed. `code-kb hook Stop` and `code-kb hook SubagentStop` run the check
  against that base and print `{"decision":"block","reason":…}` when a `proven` site is outside the
  diff, never for a `likely` site, a `signature changed` row, or a JavaScript parameter change,
  never when `stop_hook_active` is true, and nothing on any
  error or when another `julie-extract` wrote the index (the server or the CLI rebuilds it; a hook
  never does). The session file keeps the last checked and the last blocked diff hash
  (`checked_diff`, `blocked_diff`): a Stop on either runs no scan and prints nothing, and a
  `<session_id>.lock` file makes parallel hooks of one session wait up to 2 s, so they block once.
  `hooks/claude-codex-hooks.json` wires them. The text, every rule, and the known limits are in
  `docs/edit-check.md`; change that file with the code. `scripts/edit_replay.py` replays the check
  and the Stop hook offline on real commits and session events (`evals/replay/README.md`); the
  scores are in `docs/replay-baseline.md`, and the session cases stay outside the repository.
- Continuous testing and editing stay in native agent terminal commands. `code-kb` predicts the
  targeted test targets to run before or after a change; filesystem changes automatically refresh indexed files.
- Self-cleaning workspaces: Every repository and git worktree maintains its own isolated database at
  `<root>/.code-kb/artifact.db`, and `code-kb` writes a `.gitignore` with `*` inside that directory, so no project `.gitignore` edit is needed. Deleting a repository directory or running `git worktree remove`
  automatically cleans up the AST index database with no orphaned external state. Durable tool
  telemetry and token efficiency history are preserved centrally at `~/.code-kb/telemetry.db`.
  `tool_telemetry` records the MCP client (`clientInfo.name` of `initialize`), and `hook_telemetry`
  records each Stop check: the event, the session ID, the diff hash, the sites listed, whether it blocked, and
  `followed` (the next Stop of that session after a block changed a line of a listed site).
- Cross-platform agent hooks: `code-kb hook [SessionStart|SubagentStart|PreInvocation]` outputs agent routing instructions
  directly from the native binary without external runtime dependencies (Node.js, bash, python), formatting JSON
  natively for Claude Code/Cursor (`SessionStart`), Copilot, and Antigravity (`PreInvocation` injectSteps).
  `code-kb hook SessionStart` also records the session base, and `code-kb hook [Stop|SubagentStop]` runs the
  edit check from the hook JSON on stdin (Edit check bullet above).

### 6. Index Freshness
- A debounced watcher refreshes filesystem changes, startup reconciles changes made while no server ran,
  and body and skeleton reads refresh their target file before answering.
- A known reconciliation or watcher failure is an error for indexed reads, never a successful stale
  answer. Reconciliation preserves completed updates and unreadable files' existing rows, but reports
  bounded failure details with the project root and a recovery action. A detail is cut at a space,
  so a path in it is whole or left out. A subdirectory that the walk may not read
  (`PermissionDenied`) is no failure: reconciliation and watch registration skip it with a warning,
  keep the index rows of its files, and watch it after its permission returns. A denied project
  root stays an error. MCP retries failed preparation
  or reconciliation on subsequent calls; a watcher failure clears only after successful reconciliation
  with no newer failure. A failed watcher start is also reported. The unreadable-index rebuild guard
  retains its no-automatic-retry rule below.
- Only the active root keeps a watcher; a root the server switched away from loses its watcher by
  the next call. When a call switches roots, the watcher and the offline
  reconcile restart for the new root. Dropping a watcher does not wait for its worker: the worker
  stops at its next check. A new watcher of the same root, an index rebuild, and `create_index`
  wait for it, so a root never has two workers and a rebuild never replaces an index that a
  stopped worker still scans or holds open.
- A full scan passes `HARD_EXCLUDED_DIRS` and `HARD_EXCLUDED_SUFFIXES` (`workspace.rs`) to julie
  as a temporary `--ignore-file`, so a reconcile never deletes what a scan wrote. julie's own list
  lacks `.miller`, `.agents`, and `.razorback`.
- A call waits up to 5 s for its root's index. If the index is not ready, the answer is
  `Indexing <root> started; call again in a few seconds.` The next call for that root checks again.
- Until upstream header updates preserve C++ language detection, a batch of changed `.h` files triggers a
  content-aware workspace scan that only re-extracts changed files.

### 7. Pinned Extractor & Bundled Distribution
- `code-kb` pins the exact extractor version in `scripts/julie-pins.json` (currently `3.10.0`).
- Build guard: `crates/code-kb-cli/build.rs` verifies that `julie-extract` is restored and matches the pinned version. A missing or mismatched extractor fails the build immediately (bypassable for offline packaging via `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1`).
- Single-download distribution: Release archives ship `code-kb` and matching `julie-extract` pre-packaged side-by-side. Users download one archive and receive both binaries ready to execute.
- Runtime discovery: `code-kb` checks `JULIE_EXTRACT_BIN`, next to its own executable (`current_exe().parent()`), `.tools/julie-extract` in that directory or any directory above it, the same two places for the file a symlinked executable points to (`~/.code-kb/bin/code-kb` → `target/release/code-kb`), and `PATH`. The current directory is never searched, so another repository's vendored extractor is never run. The first candidate whose version matches the pin wins; otherwise the first candidate found is used with a warning.
- Index version guard: the index is rebuilt once when it was written by a different `julie-extract` than the one in use. The guard compares against the binary in use, never against the pin alone, so a non-pinned extractor never causes repeated rebuilds. An index that a newer `julie-extract` wrote (artifact metadata or any file row) is kept rather than rebuilt by an older binary. A rebuild scans into `artifact.db.rebuild` and moves it over the index only after the scan succeeds, so readers and the project-root check never find the index missing. Servers that start together on one stale index take turns on `artifact.db.rebuild.lock`, so one rebuilds and the others find the index current. When `julie-extract` refuses an incremental `scan`, `update`, or `delete` with `fingerprint_mismatch` (another extractor build wrote the index), `code-kb` runs `scan --force` on the index in place, so callers with an open connection read the new rows. When a scan meets an index of another schema (`schema_incompatible`, or `schema_migration_required` from a julie with a newer schema, which refuses `--force` too), `code-kb` scans a new one into `artifact.db.rescan-<pid>` and moves it over the index, so the index file never goes missing while the scan runs; the version guard already rebuilds an index whose metadata names another `julie-extract`. The guard also rebuilds, the same way and under the same lock, an index that SQLite cannot read as a database (a non-empty file without the SQLite header, or `file is not a database` or `database disk image is malformed`), and removes its stale `-wal` and `-shm` files with it; the CLI and the server (startup or first call) do this before any read. When that rebuild fails, the error names the remedy, ``Run `<code-kb> scan --root <root>` to rebuild the index.``, where `<code-kb>` is the path of the running binary, because a plugin install does not put `code-kb` on `PATH`. On Unix a path with a space or another shell character is in single quotes (`'\''` for a `'`). On Windows the remedy gives two commands, because PowerShell runs a quoted path only after the call operator `&`, which `sh` rejects: ``Run `& '<code-kb>' scan --root '<root>'` in PowerShell or `'<code-kb>' scan --root '<root>'` in Git Bash to rebuild the index.``, where PowerShell doubles each single quote (also `‘`, `’`, `‚`, and `‛`) and nothing in single quotes expands in either shell. The server does not retry it: each later call for that root returns the same error until the index is readable again, such as after that scan. This also holds for a rebuild that failed while the server answered calls for another root: the server reads the result of each finished prepare of an inactive root before it drops it.
- New artifacts are built at the extractor's `references` level: the `facts` rows plus the call and variable-reference identifiers with their call metadata, without the annotations, complexity metrics, source regions, and type-argument rows of `full`, which code-kb never reads. At that level julie 3.9.0 (schema v8) writes no `reference_sites` row for an identifier that no relationship shares, so `identifiers.reference_site_id` can be null; code-kb never reads `reference_sites`. The callers report reads the call and read identifiers. On this repository the index is 175 MB against 276 MB at `full` and 145 MB at `facts`, and a scan takes 4.3 s against 5.4 s and 4.2 s; hermes-agent is 2.45 GB and aspnetcore 2.14 GB, within 1.5 times their 2.6.0 `facts` size. The version guard also rebuilds an index recorded at another level. `execute_julie_extract` sets `JULIE_BULK_CACHE_KIB` to 1 GiB unless the environment sets it: julie sizes its page cache at an eighth of physical memory, so a scan's memory grew with the index (hermes-agent: peak 5.4 GB, now 2.1 GB, for 1.4% more scan time).
- Scans pass `--parent-pid` (Unix) so an extractor scan aborts when the `code-kb` process that started it dies.
- Plugin distribution: every harness plugin (`.claude-plugin/plugin.json`, `.codex-plugin/plugin.json` with `.mcp.json`, root `plugin.json` with `mcp.json`, `mcp_config.json`, and `hooks.json` for Antigravity) starts the server and hooks through `bin/code-kb-launcher.cjs`. The launcher is a dependency-free Node script: on first run it downloads the release archive for the plugin's version from GitHub Releases into `~/.code-kb/dist/<version>/<target>/`, verifies the `.sha256` sidecar, unpacks it, and then executes `code-kb` with the given arguments. When that download fails, it runs the newest other cached version with a warning, so a version bump that reaches `main` before its release does not stop the server. `CODE_KB_BIN` names a binary to run instead; a binary or symlink at `~/.code-kb/bin/code-kb` overrides the download in every harness (Codex clones plugins into a cache and drops the environment of MCP servers, so only a file can reach it); and a plugin installed from a source checkout runs that checkout's `target/release/code-kb` when it exists. With one of these, `cargo build --release` plus a session restart is the whole dev loop. No binaries are committed to git. `tests/plugin/*.test.cjs` (run with `node --test`) cover the launcher and keep every manifest on the launcher and on one version.
- Release workflow: Documented step-by-step in `docs/RELEASING.md`; automated pre-flight check via `scripts/release-preflight.sh`, which also requires a fresh PASS from `scripts/release-corpus-check.py` (the previous release against the candidate on the real projects in `scripts/release-corpus.txt`) a dogfood report from `scripts/release-dogfood.sh` with `## Result: PASS` and no `[open]` defect, and `target/reach-gate/report.md` from `scripts/reach_gate.py run` with `## Result: PASS` and a `Commit:` line that names an ancestor of HEAD. The reach gate scores the callers report against compiler (SCIP) truth for 30 symbols in each repository of `evals/reach/corpus.json`: proven precision, excluded precision (excluded sites that cover no truth line), and accounted share must be at least 0.98 in every language, and fixes are tuned on the dev half only. The corpus check finds only changed output; the dogfood round finds wrong answers. Each release runs one dogfood round: a wrong answer is fixed before the tag with a regression test and a rerun of the failed call, not a new round; a gap (a true but incomplete answer, a heuristic miss, a feature request) does not block the release; only the user defers a wrong answer. A new julie-extract version is tagged after its code-kb dogfood round has no `[open]` defect. The launcher fetches by the version in `.claude-plugin/plugin.json`, so the version bump reaches `main` only in `git push --atomic origin main vX.Y.Z` with its tag, after CI passed on the commit before it; the release workflow waits for CI on the tagged commit.

### 8. Dynamic MCP Discovery & Zero Ghost Compatibility
- **An MCP server is an ephemeral, agent-facing discovery surface, not a frozen REST API.**
- Agents discover tools dynamically via `tools/list` on session start and carry zero state across sessions. There is no concept of "backward compatibility" for tool names across sessions.
- `tools/list` sets `_meta["anthropic/alwaysLoad"]: true` on every tool, and lists seven: `file_skeleton`, `lookup_symbol`, `get_symbol_body`, `get_symbol_context`, `find_references`, `blast_radius`, and `check_edit` (CLI `check`). Accuracy phase 2 removed `codebase_outline` (`file_skeleton` on a folder replaces it), `search_symbols` (`lookup_symbol` adds the ranked matches), `find_structural_facts`, and `telemetry_summary`; the CLI keeps `outline`, `search`, `facts`, and `stats`.
  Claude Code otherwise defers MCP tools behind `ToolSearch`, and in a 2026-10 test no Claude agent
  called `ToolSearch` for them: 0 of 24 runs used code-kb, against 22 of 24 with the tools loaded.
  A routing-block line that told agents to call `ToolSearch` changed nothing.
- Do not freeze suboptimal tool names, preserve dead aliases, or compromise ergonomics for "backward compatibility" when improving tool schemas.
- Optimize ruthlessly for agent cognitive clarity and minimal tool-selection ambiguity. When a tool name or boundary causes model friction, rename or sharpen it cleanly.
