# code-kb Agent Guidelines

## Sync Contract

`AGENTS.md` and `CLAUDE.md` must stay byte-for-byte equivalent except for
future tool-specific sections that are explicitly labeled. When one changes,
update the other in the same commit.

## Product Boundary

`code-kb` is an agent-facing code-intelligence engine and Model Context Protocol
(MCP) server designed to give AI coding agents progressive disclosure, semantic
symbol search, and surgical context slicing with minimal token consumption.

- Backed by AST facts extracted by `julie-extractors` (SQLite schema v7).
- SQLite in WAL mode is the query engine; `code-kb` holds no repository data in RAM.
- All MCP tools have an exact 1:1 CLI command for direct terminal verification. The CLI also
  has `outline`, `search`, `facts`, and `stats`, which have no MCP tool.
- Non-goals: Do not add web dashboards, GPU embedding runtimes, daemon watchers
  outside the 1:1 MCP session, or workspace registries.

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
  leaves it empty when two types are equally near. It runs in `ensure_fts_index` and after every
  incremental `update` or `delete`, and reads the rows through the partial index
  `idx_symbols_unlinked_impl`. julie's `ON DELETE SET NULL` clears a link when it rewrites the
  type's file, and the next run links it again. A skeleton shows a method whose parent is in
  another file as a top-level row.

### 3. Token-Dense Progressive Disclosure
- Always return the most compact representation that answers the query.
- Strip implementation bodies in `file_skeleton`.
- Include only immediate callee signatures, related types, and test
  locations in `get_symbol_context`.

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
  context for each overload up to `MAX_CONTEXT_OVERLOADS` (4). Other candidate sets stay an
  `Ambiguous symbol` error that lists each candidate's `id`. CLI JSON is one object for one
  symbol and an array for overloads.
- Recovery on a miss: every not-found path builds its text from `symbol_not_found_parts` or
  `file_not_found_parts` in `queries.rs`. The text names the bound workspace, then
  ``` `path` does not define it; it is defined in:``` with the definitions elsewhere when a path
  filter excludes a symbol of that exact name, or
  `Did you mean one of:` with up to three candidates as `kind `name` (path:line)`, or
  `No similar name is indexed; check the workspace and spelling.` Candidates come from the
  substring search first, then from `symbol_names_tri` trigram rows kept within an edit distance
  of the query. File candidates match by basename, then by stem, then by the last two segments, then by a basename within two edits.
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
- Search ranking: the ranked search (CLI `code-kb search`, and the fallback of `lookup_symbol` in the next bullet) admits rows from four branches (exact name, FTS5 word match, trigram name substring, so `sha256` finds `parseSha256Sidecar`, and the members of a class whose whole name is a query word), then a deterministic Rust rerank in `queries.rs` credits each query term once from its strongest field (name whole token 3, name stem 2, the whole name of the member's class 3 when another word matches the member's name, so `Flask init` finds `Flask.__init__`, name substring 1, signature or docstring 1), weighted by the term's rarity across the index (a capped FTS5 match count per term), and adds the whole-name and all-words bonuses (the whole-name bonus is 100 for definition kinds and 60 for every other kind) and the kind, path, documentation, and test priors, subtracts 2 for a function or method nested in a function, method, or constructor, and adds 6 (`field coverage` in `--explain`) to a function or method when one query word is a whole name token, one is in the signature, two different words are in the docstring, and the row matches at least half the query words; the rerank reads the first 400 bytes of a signature and the first 640 bytes of a docstring; `score` is that rerank score. For a query with two or more different words, when at least `min(limit, max(15, ceil(3 * limit / 4)))` rows match two or more words, a row that matches one word is dropped once that many rows that match two or more words rank above it, unless its name holds a rare query word. After this first rerank, search considers at most the first three non-document code types among the top 20 results and reads up to 40 direct members of each in source order with the same filters. It admits or enriches only members whose own name or first 400 bytes of signature or docstring match a query term. The selected owner's name and first 400 bytes of documentation can then credit otherwise unmatched terms at 0.5 (`owner_context` in `--explain`) before a final rerank; this step does not recurse or change the exact-name bonuses. `lookup_symbol` lists exact names first, then names that start with the query, then names that contain it, and a `self.` attribute row after the other rows of its rank. A documentation link (Markdown and other document languages) is never an import row in lookup or search. In lookup and search text a member of a class or another code type shows as `Owner.name` (`ScriptInfo.__init__`), which the other tools accept as a name; JSON keeps the raw name. When some lookup rows only start with or contain the query, the header says how many are named exactly, and those other rows fold into one line of names, kinds, and counts; JSON keeps every row. A lookup that ends in `.` or `::` (`App.`) lists the members of the classes, modules, and namespaces with that name in source order. A Markdown heading with that name counts only when no code type has it. The `kind` filter of lookup and search accepts the stored kind or the kind the output shows (`attribute`, `section`, `link`, `element`, `table`). An attribute row whose signature starts with `self.`, `this.`, or `cls.` (except an F# member) shows the kind `attribute`, not `property`; JSON keeps the raw kind. An attribute row such as `self.debug = …` whose name a base class defines with a value or a property is a write, not a definition: lookup, search, and skeletons leave it out, and lookup still fills its limit. A base class named by a pending `extends` row counts only when no other class of that name is as close to the subclass's file. `blast_radius` shows a test method as `Class::test_name`. Rows from test files are hidden by default: a path rule in `queries.rs` (`is_test_path` and its SQL mirror `test_path_predicate`) hides the whole file, not only the symbols `julie-extract` flags, and `is_test: true` / `--include-tests` shows them again. A `lookup_symbol` row whose name equals the query is shown either way. When an exact lookup finds a definition, the import rows with the same name fold into one line with the count and the first three locations, and they do not count toward the limit. Equal scores break by name strength, the sum over query words of 3 for a whole-token name match, 2 for a stem match, 1 for a substring match, and 0 for none, then names that do not start with `_` before names that do; `--explain` reports the name strength as `name_strength`. `code-kb search --explain` prints the breakdown and the rerank timer; `lookup_symbol` takes no `explain` parameter, and `--verbose` stays debug logging.
- Lookup fallback: `lookup_symbols_op` in `ops.rs` serves `lookup_symbol` and CLI `lookup`. When the query is plain (not a qualified name, and not a member listing that ends in `.` or `::`) and no listed row has the query as its name, it also runs the ranked search with the same filters and limit. It appends the ranked rows that the name rows do not show, under `No exact name match; N full-text matches for "q":`. A query that names a symbol exactly prints no ranked section. When both lists are empty, the answer is the `symbol_not_found_parts` text. The MCP tool and the CLI print the same text, and CLI `lookup --json` prints one object with `names`, `ranked`, and, for a miss, `miss`. CLI `search` runs only the ranked search.
- Reference rules: a pending call whose receiver names an import binding in the caller's file matches
  a target whose path holds that module as a directory or file name (`flask.Flask(...)` matches
  `src/flask/app.py`). A relative import source (`../core/util.js`) names one file, resolved from
  the caller's folder with any script extension or an `index` file, and a receiver bound by such an
  import matches a class or namespace member only in that file. A bare pending call never matches a definition in another file when the
  caller's file defines that name at module level or inside the caller; an import or re-export
  there does not count. A `self`, `this`, or `cls`
  call matches a method of any ancestor class, through same-file and cross-file `extends` rows,
  and a `self`, `this`, or `cls` call also matches a member of the caller's own class or object
  literal. A base class that a pending `extends` row names counts unless a class of that name is
  closer to the subclass's file.
  A member access whose receiver names a type-like target groups to one row per
  file with an `occurrences` count. An identifier row is dropped when a relationship row already
  covers the same site. An import alias satisfies a pending receiver unless the import source
  starts with `Qt`. An `extends` row never resolves to its own component. A `type_usage` or `member_access` identifier matches a selected target only in the same language, except among JavaScript, TypeScript, TSX, JSX, and Vue, between Razor and C#, and from QML to C++. A name-only `type_usage` does not match a type when a type with the same name is nearer to the use: nested in an enclosing scope, or at the top level of the same file. The grouped rows of a type-like receiver follow the same language rule and leave out a file that defines its own type with that name. A caller that julie names as a generated lambda shows as `<lambda> in <enclosing symbol>`, and a site with no caller shows as `(no enclosing symbol)`. A handler row is
  labelled `handler` when the receiver names the owner and `handler (candidate)` otherwise. Test
  paths include `/autotests/` directories and file names that start with `tst_`. A Qt C++
  header's `property` and `event` rows come from the extractor's macro pre-pass and render like
  any other member; a skeleton `event` row whose signature does not spell `signal` or `event`
  carries `// event` before its line range. Rows from one caller to one name at the same path and
  line merge into one row. A method call on a local variable matches a method of the class
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
  matches only the nearest class in the chain that defines the name. A bare call matches a class
  or function defined inside the caller, and a definition in an enclosing function, namespace,
  or module scope, where the nearest scope that defines or imports the name wins. In Java, C#, Kotlin, Swift, C++, Scala, Dart, Ruby,
  and VB.NET a bare call also matches a method of the caller's class chain, where the nearest
  class that defines the name wins, a method of another part of the caller's class in the same
  file (a C# partial class), and a type nested in an enclosing class. A member-access identifier never matches a function
  nested in another function, a private JavaScript or TypeScript module-level name in another
  file, or any target when its receiver is a capitalized name that no indexed type, module,
  import, field, or property has (`Assert.Contains`); a QML signal handler is exempt. Callee rows
  come in source order. `find_references` for callers ends with one line that counts the files
  that import the target.
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
  target's own overloads never count. A call never reaches a Go, Java, or Rust field, property,
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
  is unresolved). A module receiver never proves a class member. A Python absolute import proves
  only a file whose top package (the outermost folder with `__init__.py`), or a plain folder
  above it, is the first segment of the import (`import json` does not reach `src/flask/json/`).
  A JavaScript or TypeScript read of the name that only scope ties to the target is likely,
  because julie does not record callback parameters. A bare name never reaches a JavaScript
  method assigned to a property (`req.accepts = function`). A call that julie links to its own
  caller is not proof, because julie links a call inside a method to that method by name alone. A
  call through `super`, `base`, or Kotlin `super<X>` reaches an ancestor of the caller's class,
  never that class. Java and Kotlin link `super.f()` in an override to the override; when the
  call's identifier has such a receiver, the report drops a link into the caller's class or an
  enclosing class and judges the call by its receiver, so the nearest ancestor that defines the
  name proves it (`class chain`). A target whose owners are no ancestor of the caller's class is
  excluded: ``receiver `super` reaches only the ancestors of `Sub` ``. A subclass write to an
  inherited attribute (`self.debug = …`) is a site, not a definition, and does not hide the base
  class's definition in a class chain. A C# extension method counts the type of its `this`
  parameter as an owner, so a receiver of that type does not exclude a call. A receiver declared
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
  interface, Kotlin and Rust `<T>`, C++ `template<typename T>`), and a Python `TypeVar`. The lists are read from the
  signatures, from julie's `typeParameters` of a TypeScript function, and, for a TypeScript class
  or interface, from the first 600 bytes of its source.
  julie names a test call after its label (`test("strictObject", …)`); such a label is not a
  definition, and when the target is a label every site is excluded: ``res` is a test label,
  which code never names``. A function defined inside a Python, Rust, Kotlin, C#, Swift, Scala,
  or Dart function is local to it, so a site outside that function is excluded:
  ``read` is local to `test_session_cookie_setting``. A Java member access, written without
  parentheses, never reaches a method, because julie writes a method reference (`parser::next`)
  as a call: ``a member access reads a field, not the method `length` ``. A bare Java
  call is excluded with the basis ``name` is imported from `java.lang.Math`` when its file has
  `import static C.name`, no definition of the name is in the caller's file, and C and the
  caller's class chain own none. A bare site that a
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
  them. A JavaScript call whose receiver names the top-level object that the target is assigned
  on in its file (`res.redirect()` for `res.redirect = function`) is `likely` with the basis
  ``receiver `res` names the object `redirect` is assigned on`` when no other definition is
  assigned on an object of that name.
  JavaScript and TypeScript imports go through the resolver in `reference_report/js_modules.rs`. It
  follows `export … from`, `export * from`, and namespace re-exports to a relative file (any
  script extension or an `index` file) or to a workspace package by its `package.json`
  `exports`, `module`, `main`, and `types`, scripts before declaration files, and it takes the
  next candidate when a walk reaches nothing. `require()` follows the `require` condition and
  `main`, never `module`; an ES import follows the `import` condition, `module`, and `main`. A
  package with no entry for the importer falls back to its `index` file. `types` comes after the
  scripts and never counts as a second script; a package that still names two scripts proves
  nothing. `exports.name = …` and `module.exports.name = …` count as exports. The nearest
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
  `qualified path` proof becomes `likely` and names that line.
  Reach sections: `reach_sections` in `reference_report/sections.rs` adds up to six sections
  after the sites are labeled: `Overrides`, `Overrides of`, `Constructions`, `Variant uses`,
  `Registrations`, and `Tests that call it`, where each test has a run command from
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
  `reference_report/arity.rs` excludes a `likely` or `unresolved` Python, Java, C#, Go, Rust,
  Kotlin, or TypeScript call in the target's language (never JavaScript, never a spread) when its
  count fits no definition of the target: ``3 args; `render` takes 1-2``. A `proven` site never
  changes. The range comes from the parameter rows checked against the signature, counts a
  method's receiver where a call can pass it, and joins the ranges of the same-named methods of
  every base type, so a call through a base reaches an override. Every rule and every shape with
  no range are in `docs/argument-counts.md`; change that file with the code.
- Related tests (`get_symbol_context`): callers from relationships, pending calls, and references
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
  computes multi-hop reverse reachability via SQLite recursive CTEs and predicts targeted tests to run.
  Auto-discovers uncommitted git changes when no target is passed. A caller at depth 1 is a
  `direct caller`, a deeper one an `indirect caller [depth N]`. A caller that julie flags as a
  pytest fixture is replaced by the tests that take it as a parameter, by the name in its
  `@pytest.fixture(name=...)` when it has one: in its file, or under the directory of the
  `conftest.py` that defines it. A fixture that takes that fixture is replaced by the tests that
  take it in turn. A setup member in another test framework, such as an xUnit test-class
  constructor or a Python `setUp` or `tearDown` method, is replaced by the tests of its class. A
  fixture or setup member with no such tests stays, labelled `fixture` or `setup`. Every test row is a test that the call graph reaches: a caller, a fixture user, a test of a setup member's class, or a test that reaches the target through a constructor. When the walk finds no test for one symbol, the rows are the `Tests that call it` of its callers report, with the reason `calls it (find_references)`, or `calls it by name only (find_references)` when only `likely` sites with the basis `name only` reach the test; a target that is itself a test is then its own row, `the target is a test`. The index guesses no test from a shared word in a name, a stem in a file name, or an error handler, and it adds no `possible` row for a call that the runtime makes. When no test is found and a caller lies past the depth (the walk looks one level further, up to the largest depth, 5), the answer says ``No test reaches it within depth N; a larger `depth` may find some. Otherwise run the full test suite.``; otherwise it says `No test reaches it through the index. Run the full test suite.`; a list that `limit` or the 200-row discovery ceiling cut says neither. For a file target, the tests in one test file that reach the target only through a constructor fold into one row for that file, `constructor-only callers (N targets)`. When `limit` cuts a list,
  the answer says how many rows it shows of how many it found.
- Edit check: `check_edit` (CLI `code-kb check [--base REV]`, `edit_check_op` in `ops.rs`) compares the
  working tree with a base revision (the given `base`, else the session base, else `HEAD`) and lists each
  symbol whose shape changed: removed or renamed, its parameters or signature, the fields of a class,
  struct, or record, or the variants of an enum. Each change lists the sites the diff reached, the
  `proven` and `likely` sites it did not reach, a count of the `unresolved` ones, and the tests to run.
  The base symbols come from a `julie-extract scan` of the base content of the changed files into
  `<root>/.code-kb/check/<pid>/`, removed after the call; the main index changes only by the refresh
  of the changed files. A check covers at most 200 files and 10 symbols and starts no step after 20 s, and the
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
  `docs/edit-check.md`; change that file with the code.
- Continuous testing and editing stay in native agent terminal commands. `code-kb` predicts the
  targeted test targets to run before or after a change; filesystem changes automatically refresh indexed files.
- Self-cleaning workspaces: Every repository and git worktree maintains its own isolated database at
  `<root>/.code-kb/artifact.db`, and `code-kb` writes a `.gitignore` with `*` inside that directory, so no project `.gitignore` edit is needed. Deleting a repository directory or running `git worktree remove`
  automatically cleans up the AST index database with no orphaned external state. Durable tool
  telemetry and token efficiency history are preserved centrally at `~/.code-kb/telemetry.db`.
- Cross-platform agent hooks: `code-kb hook [SessionStart|SubagentStart|PreInvocation]` outputs agent routing instructions
  directly from the native binary without external runtime dependencies (Node.js, bash, python), formatting JSON
  natively for Claude Code/Cursor (`SessionStart`), Copilot, and Antigravity (`PreInvocation` injectSteps).
  `code-kb hook SessionStart` also records the session base, and `code-kb hook [Stop|SubagentStop]` runs the
  edit check from the hook JSON on stdin (Edit check bullet above).

### 6. Index Freshness
- A debounced watcher refreshes filesystem changes, startup reconciles changes made while no server ran,
  and body and skeleton reads refresh their target file before answering.
- Only the active root keeps a watcher; a root the server switched away from loses its watcher by
  the next call. When a call switches roots, the watcher and the offline
  reconcile restart for the new root.
- A full scan passes `HARD_EXCLUDED_DIRS` and `HARD_EXCLUDED_SUFFIXES` (`workspace.rs`) to julie
  as a temporary `--ignore-file`, so a reconcile never deletes what a scan wrote. julie's own list
  lacks `.miller`, `.agents`, and `.razorback`.
- A call waits up to 5 s for its root's index. If the index is not ready, the answer is
  `Indexing <root> started; call again in a few seconds.` The next call for that root checks again.
- Until upstream header updates preserve C++ language detection, a batch of changed `.h` files triggers a
  content-aware workspace scan that only re-extracts changed files.

### 7. Pinned Extractor & Bundled Distribution
- `code-kb` pins the exact extractor version in `scripts/julie-pins.json` (currently `3.8.1`).
- Build guard: `crates/code-kb-cli/build.rs` verifies that `julie-extract` is restored and matches the pinned version. A missing or mismatched extractor fails the build immediately (bypassable for offline packaging via `CODE_KB_ALLOW_MISSING_JULIE_EXTRACT=1`).
- Single-download distribution: Release archives ship `code-kb` and matching `julie-extract` pre-packaged side-by-side. Users download one archive and receive both binaries ready to execute.
- Runtime discovery: `code-kb` checks `JULIE_EXTRACT_BIN`, next to its own executable (`current_exe().parent()`), `.tools/julie-extract` in that directory or any directory above it, and `PATH`. The current directory is never searched, so another repository's vendored extractor is never run. The first candidate whose version matches the pin wins; otherwise the first candidate found is used with a warning.
- Index version guard: the index is rebuilt once when it was written by a different `julie-extract` than the one in use. The guard compares against the binary in use, never against the pin alone, so a non-pinned extractor never causes repeated rebuilds. An index that a newer `julie-extract` wrote (artifact metadata or any file row) is kept rather than rebuilt by an older binary. A rebuild scans into `artifact.db.rebuild` and replaces the index only after the scan succeeds. Servers that start together on one stale index take turns on `artifact.db.rebuild.lock`, so one rebuilds and the others find the index current. When `julie-extract` refuses an incremental `scan`, `update`, or `delete` with `fingerprint_mismatch` (another extractor build wrote the index), `code-kb` runs `scan --force` on the index in place, so callers with an open connection read the new rows.
- New artifacts are built at the extractor's `full` level: everything julie computes, including call and variable-reference identifiers, annotations, and source regions. The callers report reads the call and read identifiers. On this repository the `full` index is 137 MB against 70 MB at `facts`, and a scan takes 3.25 s against 2.6 s. The version guard also rebuilds an index recorded at another level. `execute_julie_extract` sets `JULIE_BULK_CACHE_KIB` to 1 GiB unless the environment sets it: julie sizes its page cache at an eighth of physical memory, so a scan's memory grew with the index (hermes-agent: peak 5.4 GB, now 2.1 GB, for 1.4% more scan time).
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
