# Language support and observed fixture coverage

`code-kb` accepts every language reported by its pinned `julie-extract`. The
language conformance test reads the installed `julie-extract languages --json`
report at runtime, then checks that both the fixture directory set and
`evals/languages/fixtures.json` match it. The current pinned report is Julie
3.9.1 with 42 language IDs.

Fixture sources live under `evals/languages/tests/<language>/` so they are
excluded from this repository's default searches. Paths in the table and
manifest's `file` field are copied to production-like paths inside a temporary
project before scanning.

Julie capability flags and code-kb behavior are separate evidence. The
capability column records only the booleans in Julie's `actual_capabilities`
report. The observed column lists operations the real-extractor fixture runs
through code-kb's shared core APIs. A listed operation proves the fixture case
works; it does not establish complete grammar or project-wide language support.

| ID | Fixture | Primary extracted symbol | Julie capability flags | Observed fixture behavior |
| --- | --- | --- | --- | --- |
| `rust` | `rust/sample.rs` | `anchor_rust` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `c` | `c/sample.c` | `anchor_c` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `cpp` | `cpp/sample.cpp` | `anchor_cpp` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `go` | `go/sample.go` | `AnchorGo` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `gomod` | `gomod/go.mod` | `example.com/conformance` (`module`) | `SPR` | `L/O/B/C/U`; caller and impact checks are N/A for this directive fixture |
| `gosum` | `gosum/go.sum` | No symbol is expected | `—` | `L∅/O/U`; symbol body, context, caller, and edit checks are N/A because a checksum file has no declarations |
| `zig` | `zig/sample.zig` | `anchor_zig` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `typescript` | `typescript/sample.ts` | `anchor_typescript` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `tsx` | `tsx/sample.tsx` | `anchor_tsx` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `javascript` | `javascript/sample.js` | `anchor_javascript` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `jsx` | `jsx/sample.jsx` | `anchor_jsx` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `html` | `html/sample.html` | `anchor_html` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `css` | `css/sample.css` | `.button` (`property`) | `SIPR` | `L/O/B/C/R/I/U/E`, plus custom-property navigation and ASCII/Unicode/escaped custom-function calls resolved from bounded indexed evidence |
| `vue` | `vue/sample.vue` | `anchor_vue` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `python` | `python/sample.py` | `anchor_python` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `java` | `java/Sample.java` | `anchor_java` (`method`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `csharp` | `csharp/Sample.cs` | `AnchorCsharp` (`method`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `vbnet` | `vbnet/Sample.vb` | `AnchorVbnet` (`method`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `php` | `php/sample.php` | `anchor_php` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `ruby` | `ruby/sample.rb` | `anchor_ruby` (`method`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `swift` | `swift/sample.swift` | `anchor_swift` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `kotlin` | `kotlin/sample.kt` | `anchor_kotlin` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `scala` | `scala/sample.scala` | `anchor_scala` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `dart` | `dart/sample.dart` | `anchor_dart` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `elixir` | `elixir/sample.ex` | `anchor_elixir` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `fsharp` | `fsharp/sample.fs` | `anchor_fsharp` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `erlang` | `erlang/sample.erl` | `anchor_erlang` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `lua` | `lua/sample.lua` | `anchor_lua` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `qml` | `qml/Sample.qml` | `anchor_qml` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `qmldir` | `qmldir/qmldir` | `Conformance` (`module`) | `S` | `L/O/B/C/U`; caller and impact checks are N/A for this module directive |
| `r` | `r/sample.R` | `anchor_r` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `bash` | `bash/sample.sh` | `anchor_bash` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `powershell` | `powershell/sample.ps1` | `AnchorPowershell` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `gdscript` | `gdscript/sample.gd` | `anchor_gdscript` (`method`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `razor` | `razor/Sample.razor` | `AnchorRazor` (`method`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `sql` | `sql/sample.sql` | `anchor_sql` (`function`) | `SIPRT` | `L/O/B/C/R/I/U/E` |
| `regex` | `regex/sample.regex` | `anchor_regex` (`function`) | `SIRT` | `L/O/B/C/U`; caller and impact checks are N/A for a named capture group |
| `markdown` | `markdown/sample.md` | `anchor_markdown` (`module`) | `SPR` | `L/O/B/C/U`; caller and impact checks are N/A for a heading |
| `json` | `json/sample.json` | `anchor_json` (`module`) | `SPR` | `L/O/B/C/U`; caller and impact checks are N/A for a data key |
| `toml` | `toml/sample.toml` | `anchor_toml` (`module`) | `SPR` | `L/O/B/C/U`; caller and impact checks are N/A for a table |
| `yaml` | `yaml/sample.yaml` | `anchor_yaml` (`module`) | `SIPR` | `L/O/B/C/U`; caller and impact checks are N/A for a data key |
| `xml` | `xml/Project.csproj` | `anchor_xml` (`function`) | `SIPR` | `L/O/B/C/R/I/U/E` |

The operation codes describe fixture assertions:

- `L`: exact name lookup through `lookup_symbols_op`; `L∅` is a path-scoped empty lookup for the intentional no-symbol case.
- `O`: `file_skeleton_op` includes the expected symbol or says that the file has no symbols.
- `B`: `get_symbol_body_op` returns a non-empty body and any configured body marker.
- `C`: `get_context_slice_op` returns the selected symbol and a non-empty target body.
- `R`: `reference_report` includes the fixture caller or reference source.
- `I`: `blast_radius_op` includes the fixture caller or reference source.
- `U`: `update_file` refreshes the changed fixture; every symbol name is replaced and the no-symbol fixture stays empty.
- `E`: `edit_check_op` reports the configured definition change and caller evidence. Separate-line callers must be proven or likely; a caller on the same physical line as the edit may be reported as `in_diff`. CSS separately checks custom-property and selector changes.

## Capability and fixture boundaries

- `S`, `I`, `P`, `R`, and `T` in the table mean Julie reported `symbols`, `identifiers`, `pending_relationships`, `relationships`, and `types`, respectively. `—` means all five flags were false. These report values are not inferred from the core behavior checks.
- Every symbol-bearing fixture checks exact lookup, skeleton visibility, body retrieval, context retrieval, and an incremental refresh. Caller fixtures additionally check caller references, impact, and edit detection. The structural fixtures intentionally have no caller or cross-symbol reference to exercise; those specific checks are semantic N/A for the fixture, not claims that the language can never express references.
- The XML fixture uses a valid MSBuild project file so the extractor has script symbols and a same-file call to navigate. It does not stand in for every XML vocabulary.
- The XML edit fixture is one physical line, so the line-based diff reports its `caller_xml` site as `in_diff`; this is expected for a call on the changed line. Separate-line callers in the other fixtures must still be proven or likely.
- CSS uses a selector, a public custom property, a `var()` reference, and explicit `@function` custom functions. ASCII, Unicode, and escaped function declarations are checked through lookup, skeleton, body, and context APIs. The ASCII, Unicode, and escaped calls are checked across references, context, and impact; all resolve through the same-file indexed evidence rule. Julie 3.9.1 emits malformed raw call identifier names for the Unicode and escaped forms (`--` and `name_css`); code-kb recovers the canonical names only when the exact raw token is inside the indexed caller body, contains the extracted identifier span, is immediately followed by `(`, and has one matching same-file declaration. The adapter preserves each identifier's original ID and span and does not add a stored relationship. The fixture reports nine CSS parse diagnostics while still extracting the selected symbols; this is limited syntax coverage. The fixture's ordinary declarations are not expanded into standalone symbols.
- Erlang's exported function name occurs in both the export attribute and declaration. Its refresh changes both declaration-bearing occurrences while leaving the caller's old call in place so edit-check evidence remains meaningful.
- `gosum` is an intentional no-symbol fixture. Its checksum is changed and rescanned; no symbol, body, or caller is expected.

## Targeted semantic coverage beyond the matrix

The 42-language matrix checks one representative declaration and refresh path
per language. These dedicated real-extractor suites add deeper cases for
selected language families; they do not establish the same coverage for every
language or every grammar form.

| Area | Dedicated cases | Test file |
| --- | --- | --- |
| Named arguments and accepted parameter ranges | Renamed named parameters, positional-only changes, required versus defaulted formals, optional and variadic ranges, external labels, language-specific name matching, and permissive calls. Examples: `kotlin_required_formal_after_a_default_reports_calls_that_leave_it_unbound`, `php_variadic_formals_do_not_narrow_extra_argument_acceptance`, and `swift_external_label_renames_report_the_old_call`. PHP extra positional arguments remain accepted; the variadic case does not imply a strict maximum. | [`edit_check_arguments_test.rs`](../crates/code-kb-core/tests/edit_check_arguments_test.rs) |
| Construction and field edits | Kotlin/Scala/C# primary constructors, Swift memberwise initialization, C/C++ aggregates, Zig structs, F# records, Elixir structs, and Ruby keyword structs; required/defaulted additions and named/positional removals; Python and Java reads of a removed field through a typed parameter or `self`, proven only when the receiver binds the owner. Examples: `a_kotlin_data_class_reports_only_a_missing_required_primary_property`, `a_cpp_designated_initializer_reports_removed_members_but_accepts_additions_and_custom_constructors`, `an_fsharp_typed_record_literal_needs_added_fields_but_a_copy_update_does_not`, and `a_removed_python_attribute_read_through_a_typed_parameter_is_proven`. | [`edit_check_fields_test.rs`](../crates/code-kb-core/tests/edit_check_fields_test.rs) |
| Member reads and callable values | Python and JavaScript member reads, inherited/same-name owner negatives, CSS custom properties and selector composition, embedded HTML calls, C# constants, and callback-value dependencies. Examples: `removing_an_fsharp_record_field_reports_only_its_typed_literal`, `removing_a_css_custom_property_reports_its_reference`, `a_resolved_call_inside_html_is_a_reference_site`, and `a_changed_callable_reports_a_value_dependency_without_proving_a_broken_call`. | [`edit_check_members_test.rs`](../crates/code-kb-core/tests/edit_check_members_test.rs) |

These rows describe the cases with dedicated assertions. An unlisted
language/construct pairing is untested here, not semantically N/A. The base
matrix's N/A labels apply only to the selected fixture when its format has no
declaration or caller to exercise.

Julie reports an F# curried function such as `let add x y = x + y` with the
body text as its signature and `x`/`y` as parameter-role children. A partially
applied use such as `let addOne = add 1` has no extracted `arg_count`. The
argument suite covers tupled methods and constructors, but these curried calls
do not support a strict invalid-arity claim; edit-check advice remains likely
and advisory. Callback
invocation contracts are also untested: indexed callable-value dependencies
remain advisory and do not prove that a changed invocation is invalid.

### CSS call with no extracted identifier

The passing Unicode and escaped CSS cases have malformed identifier names that
the bounded adapter can recover from their preserved spans. A separate pinned
Julie 3.9.1 case emits no identifier row at all:

```css
@function --foo() { result: 1; }
@function --foo名() { result: 2; }
@function --caller_css() { result: --foo名(); }
```

| Expected | Julie 3.9.1 actual | Consumer result |
| --- | --- | --- |
| `--caller_css` has a call site to `--foo名`, with a proven reference, a callee signature in context, and an impact edge. | Three raw `namespace` declarations (`@function --foo()`, `@function --foo名()`, and `@function --caller_css()`); zero `identifiers` rows. | `ReferenceReport`: 0 sites for `--foo名` and the shorter `--foo`; caller context has no callee signatures; impact has 0 callers for either target. |

The test records this as a missing extractor fact, not N/A. The compatibility
adapter leaves it unlinked because no indexed span anchors the source token.
Julie must emit a call identifier with its source range before code-kb can bind
this form without scanning or guessing. The exact limitation fixture is
[`no-call-identifier.css`](../evals/languages/tests/css/no-call-identifier.css),
covered by [`css_custom_function_call_evidence_agrees_and_missing_identifier_stays_unresolved`](../crates/code-kb-core/tests/language_conformance_test.rs) test.

A separate declaration fact remains lossy for U+2000 (EN QUAD) in a CSS
identifier. For source `@function -- _css() { result: 1; }`, Julie preserves
the U+2000 spelling in the signature but constructs the extracted namespace
name as `@function -- _css()`. Code-kb requires the indexed name and signature
token to agree before normalizing a custom function, so it leaves this row
unchanged. The compatibility test
[`extractor_compat_test.rs`](../crates/code-kb-core/tests/extractor_compat_test.rs)
checks CSS token handling; the mismatched extractor fact remains a gap that
requires Julie to preserve the declaration name consistently.

## Consumer gaps recorded from the initial fixture run

These are baseline reproductions from the same source files used by the
conformance test. The test asserts the expected behavior and reports every
consumer gap it reaches; it does not preserve the broken response as an
expected result.

| Fixture | Expected API result | Initial observed result |
| --- | --- | --- |
| `erlang/sample.erl` | Skeleton includes `anchor_erlang/1(Value)` at line 3 and `caller_erlang/0()` at line 4. | Skeleton contained only `-module(conformance);` although both functions were extracted, lookupable, and body-readable. |
| `gdscript/sample.gd` | Skeleton includes `func anchor_gdscript(value: int) -> int:` at line 2 and `func caller_gdscript() -> int:` at line 4. | Skeleton contained only `extends Node` although both methods were extracted, lookupable, and body-readable. |
| `html/sample.html` | Caller report for `anchor_html` includes `caller_html` from the resolved `calls` relationship on line 3. | The relationship exists, but the caller report returned zero sites and counted the source occurrences as text only. |
| `html/sample.html` | Edit check for renaming `anchor_html` reports the unchanged `caller_html` call as proven or likely. | The extracted `ShapeChange` had no proven or likely sites for `caller_html`. |
| `css/sample.css` | Caller report and impact for `--accent-color` include `.button` from the `references` relationship on line 2, columns 21–35. The skeleton includes the nested custom-property row. | Julie emitted the property and relationship, but the callers report returned zero sites; the skeleton omitted the property row. |
| `css/sample.css` | Lookup shows `--anchor_css` and `--caller_css` as functions; caller context, references, and impact connect `--caller_css` to `--anchor_css`. | Julie's rows were named `@function --anchor_css()` and `@function --caller_css()`, both `namespace`; the call identifier `--anchor_css` had no relationship. |
| `css/sample.css` | Edit check for renaming public custom property `--accent-color` includes its `.button` use; edit check for renaming selector `.button` reports the changed named definition. | The initial selector rename returned no `ShapeChange` for `.button`; the conformance test keeps this as an applicable named-definition check. |
| `css/sample.css` | Calls from `--caller_unicode_css` and `--caller_escaped_css` resolve to their respective custom functions. | Julie 3.9.1 emits call identifier names `--` and `name_css`, not the declared names, so it does not produce matching relationships. This is an extractor missing-fact gap, not N/A. |

The CSS custom-function source is:

```css
@function --anchor_css() { result: 41; }
@function --caller_css() { result: --anchor_css(); }
@function --名_css() { result: 42; }
@function --escaped\ name_css() { result: 43; }
@function --caller_unicode_css() { result: --名_css(); }
@function --caller_escaped_css() { result: --escaped\ name_css(); }
```

The expected semantic names are unambiguous in Julie's preserved signatures:
the token following `@function` names each function. The ASCII caller contains
the identifier `--anchor_css` with kind `call`. The Unicode and escaped call
rows remain in the real-extractor fixture; their expected canonical identifiers,
original extracted spans, proven reference sites, caller contexts, and impact
edges are asserted. Julie's initial call facts lose the intended names, while
code-kb repairs only these exact bounded cases through indexed declaration and
caller evidence. Calls that do not meet those constraints remain unlinked. An absent identifier
produces no indexed site, so it is not counted as an `unresolved` site. A
separate tokenizer regression confirms CSS non-ASCII token handling; when
Julie loses a U+2000 whitespace-like character from the extracted declaration
name so it disagrees with the preserved signature, code-kb leaves that
ambiguous declaration unnormalized.

The baseline rows remain here as regression history. Current status is
determined by the conformance test and the implementation at the pinned
revision; a recorded baseline is not evidence that the gap remains open.

Assertions are in `crates/code-kb-core/tests/language_conformance_test.rs`:
`real_extractor_fixtures_cover_navigation_slices_references_impact_and_refresh`
covers all 42 IDs and refreshes, while
`css_custom_function_call_evidence_agrees_and_missing_identifier_stays_unresolved`
checks the supported ASCII call across reference, context, and impact, and
records the separate missing-identifier limitation without inventing an edge.

## SQL argument contracts

SQL navigation and caller fixtures pass, but strict argument checks remain an
applicable gap. Julie 3.9.1 marks function parameter variables with
`isParameter=true`, while procedure parsing can emit error-recovery rows. It
does not identify the database dialect. A reusable dialect and parameter
contract is needed before code-kb can apply a database's defaults, modes, and
overload rules without guessing. CSS custom functions also lack formal-parameter
facts; repaired navigation does not establish argument-check support.

## Cross-language consumers and test scope

The controlled pilot exposed consumer errors that the language inventory alone did not catch. Same-named Go/C#/TypeScript/Java/Rust methods could contaminate impact and context output. The shared call policy now retains same-language candidates for every indexed ID and preserves the existing JS, JVM, .NET, and C-family interoperation groups. Name-only identifiers and pending type uses keep their narrower rules, including directional QML-to-C++ matching. Compatibility permits a candidate; owner, receiver, import, or scope evidence must still establish the edge.

`cross_language_graph_test.rs` uses real extraction for the reported collisions, file-seeded impact, context dependencies/tests, and fallback behavior. Some JVM/.NET/C-family policy controls remap an existing extracted call row inside a rolled-back transaction because the fixture call was absent from extraction. These consumer controls do not establish end-to-end extractor interoperation coverage.

Broader runner discovery currently recognizes compatible Cargo, Go module, npm test-script (not the `npm init` placeholder that prints `no test specified` and exits 1), Maven/Gradle (a Gradle build runs through the nearest `gradlew`, or `gradlew.bat` on Windows, at or above its folder: `cd app && ../gradlew test`), pytest, and .NET test configurations. It supports the relevant script/JVM/.NET file variants, requires Rust package evidence for scoped files, and stops at an unconfigured nearer package. Simultaneous runner families or build tools, missing configuration, unknown extensions, and conflicting reported scopes use the manual-suite fallback. Other runner ecosystems remain a command-discovery coverage gap; their languages remain in scope for the engine and conformance suite.

The [pilot report](reviews/2026-10-26-agent-comparison.md) separates source correctness, strict grading, tool adoption, and cost. Its eight used cases are regression data, not all-language agent-performance evidence. Related-type binding has separate real-extractor regressions for lexical owners, relative and aliased imports, Java namespaces, and qualified Go imports tied to the nearest indexed module. A unique owner is required; unsupported module provenance and ambiguous names remain unresolved. Context examines at most 4096 type facts and deduplicates by owner ID. Typed receiver chains now walk exact indexed field/property owners and inheritance through the shared proof. A chain starts at one of three roots: a typed value in the caller's scope; the caller's class for `self`, `this`, or `cls` (Python `self.service.send()`); or the type that the caller's scope names (`Outer.Inner.build()` in Python, Java, and C#, proven as `qualified path`). Each later name steps through a typed field, property, or constant (`Holder.INSTANCE`, C# `static readonly Registry Default`), a unique nested type, or an enum constant without a body to its enum (`Color.RED` in Java, Kotlin, Python, Swift, Scala, Dart, Rust, C#, VB.NET, and PHP). A chain does not exclude a target in a part of its owner that the index does not link: a Kotlin companion object, a Java enum-constant body, a C# or VB.NET partial part, a reopened Ruby class, a Scala companion, a Swift or Kotlin extension, or a C# extension method. Such a site stays unresolved. A chain whose first name binds neither a value nor a type (`app.Outer.Inner.build()`) keeps the name-path rules. A chain that stops at an untyped member stays unproven. The 25 real-extractor regressions cover Go, Java, C#, imported JavaScript/TypeScript bases, decoys, ambiguity, generic declarations, removed targets, and impact agreement. Report evidence is bounded by chunk, owner, edge, and depth limits; incomplete ancestry stays uncertain. Full integration verification is recorded in the pilot report.

Go block-local shadowing exposes a remaining extractor-fact gap. A receiver parameter and an inner-block local can share the same enclosing-method ID; use identifiers do not name the chosen declaration or block extent. Source-line recency cannot establish visibility after the block ends. Receiver-chain analysis must leave that candidate set unproven until binding or scope facts distinguish it. This is missing coverage, not a language exclusion.

A Go composite-literal result call such as `(&Handler{}).Handle()` can also lack a bound receiver and relationship edge. A nearby type-usage identifier does not establish the called method. The chain regression retains that site as unresolved beside a competing owner; an explicitly typed variable supplies the positive test-reach control.

The final Linux gate at `5f998c4` passes all 42 language IDs and 1,889 workspace tests. Receiver/impact regression fixtures include plain→typed→plain transitions across graph depths. Shape probes preserve legacy qualified/import matching; malformed or uncertain facts retain full proof. Related-type owner queries reuse one local statement within the bounded context operation. The pilot report records query costs separately from the original agent outcomes and records Windows verification.

The Windows NTFS gate at `f355247` also passes all 42 language IDs and 1,865 Rust tests. Platform counts and declared plugin skips are retained in the verification record. These fixture results establish tested cases, not complete grammar or agent-task performance for every language.
