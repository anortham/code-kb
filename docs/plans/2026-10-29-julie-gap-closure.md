# Julie gap closure (julie 3.10.0, code-kb 2.11.0)

The user asked on 2026-10-29 to close the julie gaps that code-kb records, in the
same julie release as the C/C++ subscript fix, and to take the time to do it right.

## Evidence

Probes of julie 3.9.1 (`scratchpad/gap-probe.sh`, `gap-probe2.sh`) show:

| Gap | julie 3.9.1 output |
| --- | --- |
| Import source | `source` metadata only for JS/TS, Python, Zig, Lua, QML. C/C++ keep `path`, CSS `url`, Bash `sourcePath`; PowerShell, R, Ruby, F#, Dart keep the path only in the name or not at all (Ruby `require_relative "lib/helper"` is the name `helper`; PowerShell `. ./lib/Common.ps1` is `Common`). Ruby `load` writes nothing. |
| Missing import rows | PHP `require`/`include` (and `_once`), GDScript `preload`/`load`, and Java `import a.b.*;` write no import row. |
| Anonymous classes | Java `new T() { … }`, Kotlin `object : T { … }`, Scala `new T { … }`, and JS/TS class expressions without a name hang their members on the enclosing function or property. PHP already writes `anonymous_class_L<line>` (kind `class`, `type: anonymous_class`). |
| Scala local def | A `def` inside a `def` is kind `method` with the outer method as parent, the same as an anonymous-class member. |
| Overloads | Calls carry `arg_count`; only C# has `arg_types`. Java, Kotlin, Scala, C++, Swift, and VB.NET calls cannot be tied to one overload. Swift labels are not recorded. |
| Dart library | `library blog;` after a doc comment is a `variable` row; without the comment it is a `namespace` row (`type: library`). |
| C Unity test | `test_adds` is a test only when the file name starts with `test_`; `unit/calc_runner.c` that includes `unity.h` is not. |
| R `<<-` | `registry <<- list()` writes no symbol; `balance <<- balance - v` writes no write site. |
| Swift package, Zig build | `Package.swift` targets and `build.zig` `b.addModule("name", …root_source_file…)` write no fact. |
| CSS custom functions | Unicode and escaped calls have malformed identifier names (`--`, `name_css`); `result: --foo名()` writes no identifier; a U+2000 name is lossy. |

Out of scope for this release (recorded as remaining gaps): Go block scopes, Go
composite-literal receivers, method-level `overrides` edges, Rust bare names in
patterns, and SQL dialect and parameter contracts. code-kb has working source
readers for the overrides and Rust pattern cases.

## Julie wave (parallel, one worktree per task)

Base: julie `fix/subscript-callee` (dd09da8c). Worktrees:
`/home/murphy/source/julie-extractors/.worktrees/gap-<task>`, branch `gap/<task>`.
Tasks do not edit `EXTRACTION_CONTRACT_VERSION`, the marker list in
`src/tests/api_surface.rs`, `docs/contracts/extraction-output-changes.md`, the
version, or release notes. Each task reports its marker name and a ledger paragraph;
the lead adds them at integration.

| Task | Scope | Marker |
| --- | --- | --- |
| `import-source-a` | `source` + `form` on import rows: C, C++, CSS, Bash, PowerShell (dot-source, `Import-Module`, `using module`), R (`source()`, `library()`, `require()`) | `import-source-v1` (shared) |
| `import-source-b` | `source` + `form`: Ruby (`require`, `require_relative`, `load`), F# (`#load`, `open`), Dart (`import`, `export`, `part`); new import rows: PHP `require`/`include`(`_once`), GDScript `preload`/`load`, Java star import | `import-source-v1` (shared), `java-star-import-v1` |
| `anonymous-class` | Anonymous class owner rows in Java, Kotlin, Scala, JavaScript, TypeScript, TSX (PHP naming); base types; Scala local `def` becomes kind `function` | `anonymous-class-owner-v1` |
| `overload-jvm` | `arg_types` for Java, Kotlin, Scala calls; `arg_names` for named arguments | `call-argument-types-v1` (shared) |
| `overload-native` | `arg_types` for C++ and VB.NET; `arg_labels` for Swift | `call-argument-types-v1` (shared) |
| `small-fixes` | Dart library after a doc comment; C Unity test detection; R `<<-` | `dart-library-directive-v1`, `c-unity-tests-v1`, `r-superassignment-v1` |
| `package-facts` | Structural facts for Swift package targets and Zig `build.zig` modules | `package-module-facts-v1` |
| `css-calls` | CSS custom-function call identifiers with the declared names, and the U+2000 name | `css-custom-function-calls-v1` |

Import metadata contract: `source` is the module or path string as written,
without quotes; a string with interpolation keeps the text as written. `form` is
the directive keyword (`#include`, `@import`, `source`, `.`, `Import-Module`,
`using module`, `source()`, `library()`, `require`, `require_relative`, `load`,
`#load`, `open`, `import`, `export`, `part`, `require_once`, `include`, `preload`).
Existing keys stay.

## Integration (lead)

1. Cherry-pick each task branch onto `release/3.10.0` in julie; add markers, ledger
   entry `## 3.10.0`, release notes `docs/release-notes/v3.10.0.md`, version 3.10.0.
2. Run julie: fmt, `cargo clippy --workspace --all-targets --all-features`,
   `cargo test -p xtask`, `cargo xtask test default`, `cargo xtask test contract`,
   `cargo xtask compat-check` against 3.9.1.
3. Build the release binary and copy it into the code-kb worktree `.tools/`.

## code-kb wave (after julie integration)

| Task | Scope |
| --- | --- |
| File-path imports | Read `source`/`form` for the new languages in `import_path_patterns` and the outside-import rule; C/C++ includes; PHP `__DIR__`, GDScript `res://`. |
| Anonymous owners | Scala back in `LOCAL_FUNCTION_LANGUAGES`; drop the Kotlin `override` special case; class chain and reach sections through anonymous owners; display. |
| Overloads | Tie a call to one overload when `arg_count`, `arg_types`, `arg_names`, or Swift labels fit exactly one; walk, callers report, and `check_edit`. |
| Module names | Swift package targets, Zig `addModule`, Dart `library`. |
| Small fixes | R `<<-` rows, C Unity tests, CSS adapter kept only for old indexes or removed. |
| Pin and docs | Pin 3.10.0 with real checksums after the julie release; `docs/language-support.md`, CLAUDE.md/AGENTS.md. |

## Release order

1. code-kb dogfood round with the local julie 3.10.0 build; fix wrong answers.
2. User approval: push and release julie 3.10.0.
3. Pin bump with checksums, full suite, reach gate (alone), corpus check.
4. User approval: push and release code-kb 2.11.0.
