# 025: Qt and QML first-class support

## Status

2026-09-21. Parts A (tasks 1 to 18) and B (B1 to B6, plus B2b and B3b)
implemented on `feat/qt-first-class` (julie 3.2.0, not yet released) and
`feat/v1.5-qt`; B7 waits for the julie 3.2.0 release assets; Part A tasks 19 to
22 (Qt C++ macros) are not started.

Measured on the pinned corpora with the branch binaries (code-kb `28ce7c3`,
julie-extract 3.2.0):

- `skeleton shell/Ui/Button.qml` in Omarchy: 71 lines.
- `refs BarWidget --file shell/Ui/BarWidget.qml` in Omarchy: 12 `extends` rows.
- `refs Color --file shell/Commons/Color.qml` in Omarchy: 53 files.
- `refs Page --file src/controls/Page.qml` in Kirigami: 3 `extends` rows.

The "Status: draft" note below describes revision 2 at the time it was written.
This section supersedes it.

> **For agentic workers:** REQUIRED SUB-SKILL: Use razorback:subagent-driven-development whenever delegation is available and permitted, including for one task; serialize dependent tasks. Use razorback:executing-plans only when delegation is unavailable or the user/session explicitly selected single-agent execution.

Date: 2026-09-21. Status: draft, waiting for approval. Revision 2 after a Codex
review the same day (see "Review record" at the end). Nothing in this plan is
implemented yet.

**Goal:** Make the claim "code-kb gives first-class support to Qt developers
(KDE, Omarchy, Quickshell)" true and provable. Today the QML parser is solid and
the basics work, but the everyday Qt questions (who extends this component, who
reads this singleton, who handles this signal, what does this component look
like as a tree) come back empty, and Qt C++ headers are badly extracted.

**Architecture:** Fixes split between the extractor (`julie-extractors`, QML,
JavaScript, and C++ modules) and code-kb. code-kb changes stay
language-agnostic: render a symbol with children as a container, match
member-access receivers when the target is a type, accept import aliases as
receivers when resolving references, know the Qt test file names, add fact
aliases, and document the support. No `artifact.db` schema change; identifier
metadata already has a `metadata_json` column.

## Corpora used for the audit

Audit tooling: code-kb v1.4.1 (`target/release`), julie-extract 3.1.3,
`code-kb scan` defaults, Linux.

| Corpus | Pinned commit | QML files | qmldir | JS | C++ files |
| --- | --- | ---: | ---: | ---: | ---: |
| Omarchy shell (Quickshell) | `~/source/omarchy` at `49306774` | 106 | 3 | 27 | 0 |
| KDE Kirigami | `github.com/KDE/kirigami` at `ca7d636` | 225 | 1 | 1 | 102 |
| KDE plasma-workspace | `github.com/KDE/plasma-workspace` at `a45871a` | 222 | 0 | 4 | 1030 |
| Quickshell examples | `github.com/quickshell-mirror/quickshell-examples` at `c6d1236` | 14 | 0 | 0 | 0 |

Reproduce:

```bash
git clone https://github.com/KDE/kirigami && git -C kirigami checkout ca7d636
git clone https://github.com/KDE/plasma-workspace && git -C plasma-workspace checkout a45871a
git clone https://github.com/quickshell-mirror/quickshell-examples && git -C quickshell-examples checkout c6d1236
cd kirigami && code-kb scan
```

The Omarchy checkout now has its own index at `~/source/omarchy/.code-kb`
(gitignored by code-kb itself).

## What is true today (can be claimed now)

- **All 567 QML files were indexed.** `julie-extract check` accepts every one
  of Omarchy's 106 QML files. The four indexes hold two QML parse diagnostics in
  total, both from a Kirigami project template with `@PLACEHOLDER@` text. The
  check command was not run on the three cloned corpora.
- **QML symbols:** the root component (named from the file stem, signature
  `extends Base`), declared properties with their modifiers (`required`,
  `readonly`, `default`, `alias`), functions with parameters and locals,
  signals, enums, `id:` bindings, signal handlers (`onClicked:`), and imports
  with module, version, alias, and directory or JavaScript source metadata
  (`import "x.js" as X` included).
- **`qmldir` modules:** module name, object types, singletons, plus 13 fact
  kinds. `.qmltypes` files index as QML up to the 1 MiB source limit.
- **Structural facts:** imports, property declarations, signal declarations,
  bindings, object instantiations. `facts signal` and `facts import` work
  through the substring fallback. Plasma applet `metadata.json` files index as
  JSON facts.
- **Edits:** `edit_file` and `replace_symbol_body` validate QML through the
  extractor and refuse a broken body. Verified on `shell/shell.qml`.
- **Search and lookup:** `search "reload plugins"` ranks `reloadPlugins` first;
  `lookup` finds components, functions, and ids.
- **Callers and callees of QML functions** work inside a component
  (`refs reloadPlugins` lists 3 call sites; callees list the three unload calls).
- **Blast radius across components** follows unqualified nested instantiations:
  `blast-radius --file shell/Ui/Button.qml` lists 11 downstream components and
  stem-matches `tst_pagerow.qml` for Kirigami's `PageRow.qml`.
- **Qt Quick Test detection** in the extractor: `test_*`, `benchmark_*`,
  lifecycle names, `TestCase` containers, `tst_` file names (171 test symbols
  in Kirigami).
- **Speed:** the Omarchy repository (882 unsupported files, 106 QML) indexes in
  1.6 s.

## Gaps that block the claim

Ranked by how often a Qt developer hits them. "Where" names the repository
that owns the fix.

### G1. The skeleton flattens the object tree (HIGH, julie + code-kb)

Nested objects (`Timer { id: t; interval: 150 }`) emit no symbol, so their
bindings attach to the root. `skeleton shell/shell.qml` prints
`id: localPluginReloadTimer; interval: 150; onTriggered: ...` as root members,
and `Color.qml` prints three unrelated `property color background` lines in a
row. The skeleton is also not dense: `Button.qml` shrinks from 209 lines to
only 132 because every private binding (`color:`, `anchors.fill:`) is a symbol.
Omarchy has 2,046 object instantiations for 106 components. Property-value
sources (`Behavior on color { ... }`, `ui_object_definition_binding`) are a
second object form with the same problem.

### G2. Inline components are invisible (HIGH, julie)

`component SpeedDial: Item { ... }` emits no definition symbol; its
descendants still attach to the enclosing component. `traverse_node` in
`qml/mod.rs` has no `ui_inline_component` arm. Counts: Omarchy 48, Kirigami 21,
plasma-workspace 17, definition symbols 0.

### G3. Qualified type names never resolve (HIGH, julie + code-kb)

`Kirigami.Page {`, `QQC2.Button {`, `KL.ColumnView {` produce a `type_usage`
identifier named `Kirigami.Page`, which no name query matches. The pending
`instantiates` rows already split the name (`target_terminal_name = Page`,
`target_receiver = Kirigami`, `target_import_context = org.kde.kirigami`), but
code-kb's `pending_target_predicate` only accepts a receiver that names a
parent symbol or a typed variable, never an import alias, so `refs Page --file
src/controls/Page.qml` drops every qualified use. In Kirigami 864 of 2,591 type
usages (33%) are qualified. `blast_radius` reads relationships and pending
rows only, never identifiers, so fixing the identifier alone would not repair
impact.

### G4. The base type of a component has no reference (HIGH, julie + code-kb)

The root object's type (`BarWidget {` at the top of `clock/BarWidget.qml`) emits
no identifier and no relationship; only `base_types` metadata on the class row.
Result: `refs BarWidget --file shell/Ui/BarWidget.qml` and `refs Panel --file
shell/Ui/Panel.qml` return 0 although 12 and 10 components extend them
(`base_types[0]` query on the Omarchy index). "Which components extend X" is
unanswerable. Once a base-type edge exists, code-kb's "closer candidate" rule
would still resolve a file named like its base (`BarWidget.qml` extending
`BarWidget`) to itself.

### G5. Singleton and attached receivers have no reference (HIGH, code-kb)

`Color.foreground`, `Style.font`, `Kirigami.Units.gridUnit` record only the
member (`foreground`) as an identifier, so `refs Color` and `refs Style` return
0 in Omarchy, where 72 of 106 files import the `qs.Commons` singletons. The
data to answer is already there: the CLI mapper stores the source receiver on
every member-access row (`metadata_json.receiver`), and the Omarchy index holds
249 rows in 53 files with receiver `Color` and 739 rows in 63 files with
receiver `Style`. code-kb's caller query never matches on it.

### G6. Signals lose their parameters and their handlers (MEDIUM, julie)

`ui_signal` sets no signature: the skeleton prints `closeRequested;` for
`signal closeRequested()`. Binding-form handlers (`onClicked:`) carry
`handled_signal` metadata; `function onClicked()` inside `Connections` does
not. Neither form emits an identifier, so `refs clicked --file
shell/Ui/Button.qml` lists the four emit sites and none of the consumers.
Kirigami has 352 handlers for 31 signals. Property-change handlers
(`onIsOperationFinishedChanged`, 248 in Omarchy) link to nothing because no
`xChanged` signal row exists.

### G7. Qt C++ macros wreck header extraction (HIGH for KDE, julie)

tree-sitter-cpp has no Qt awareness and the extractor has none either. In
Kirigami 70 of 102 C++ files carry 981 parse diagnostics; in plasma-workspace
636 of 1,030 files carry 4,502. Not every diagnostic is macro-caused, but the
sampled ones are. Symptoms in `src/layouts/columnview.h`:

- `Q_PROPERTY(int index READ index ...)` becomes a method named `Q_PROPERTY`;
  the header's 38 `Q_PROPERTY` lines (297 in Kirigami, 914 in plasma-workspace)
  yield 0 property symbols.
- `Q_SIGNALS:` becomes a field with an empty name; the signals after it are
  plain methods. `public Q_SLOTS:` (line 627) is the two-token form.
- Every method in the class loses its return type and gains a false
  `override` prefix (`override setIndex(int index)` for `void setIndex(int index);`).
- Constructors and destructors are emitted twice.
- Forward declarations (`class ColumnView;`) are class rows, so
  `refs ColumnView` is ambiguous between line 15 and the real class at line 276,
  and `lookup Units` returns three forward declarations.

### G8. `qmldir` rows duplicate every component in lookup (LOW, julie)

`qmldir` object types are `class` rows, so `lookup Button` lists the `.qml`
class and the `qmldir` line. Single-symbol resolution already prefers
definition kinds, so `refs` is affected only when two real classes share a
name; the duplicate is a lookup-list problem.

### G9. `pragma` lines are dropped (MEDIUM, julie)

`pragma Singleton` (4 in Omarchy) and `pragma ComponentBehavior: Bound`
(50 in Kirigami, 80 in plasma-workspace) leave no trace; a singleton service
looks like any other `QtObject`.

### G10. Qt test files are not hidden by path (LOW, code-kb)

`is_test_path` in `queries.rs` knows `/tests/` and `_test.` but not Qt's
`tst_*` names or KDE's `autotests/` directory, and the SQL twin
`test_path_predicate` has a `test`/`spec` substring guard that a bare
`tst_foo.qml` never passes. Helpers in test files stay visible in prefix and
concept search: `lookup waitForWindowActive` returns three rows from
`autotests/tst_*.qml`. Exact-name lookup returns test rows by contract and must
keep doing so. Kirigami has 44 `tst_` QML files and 67 QML files under test
directories.

### G11. The QML JavaScript dialect fails syntax checks (LOW, julie)

`.pragma library` and `.import` directives are legal in Qt JavaScript
resources but fail the JavaScript grammar. `shell/Commons/BorderGeometry.js`
is indexed (264 symbols, one diagnostic at line 1), but `edit_file` refuses
every edit to it because the check path rejects the file. Rare in the corpora
(1 of 32 JS files), common in older Plasma code. Extraction and `check` parse
through separate paths (`pipeline.rs` and `syntax/mod.rs`), so a fix must
cover both.

### G12. Fact aliases have no Qt vocabulary (LOW, code-kb)

`CATEGORY_ALIASES` knows `sql`, `route`, `config`, `model`. `signal`, `import`,
`binding`, `component`, and `module` reach the facts only through the raw
substring fallback and never appear in the alias line of the category list.

### G13. Nothing documents Qt or QML (LOW, required for the claim, code-kb)

README, `docs/site/index.html`, and the routing block never say QML, Qt, KDE,
or Quickshell. The site says "40+ languages" only.

### G14. Multi-line property values truncate badly (LOW, julie)

`property var shellValues: ({ ... })` prints as `property var shellValues: (;`
because the signature is the whole node and code-kb keeps its first line.

### G15. Bare `required` declarations emit nothing (LOW, julie)

`required index` (plasma-workspace `DeviceItem.qml:23`, `AbstractItem.qml:31-33`)
is a `ui_required` node with no arm.

### G16. Attached binding names have no type reference (MEDIUM, julie)

`Layout.fillWidth:`, `Keys.onPressed:`, `WlrLayershell.layer:`,
`Kirigami.FormData.label:` are binding names, not member expressions, so the
attached type never appears as an identifier. Omarchy: `Layout` 56, `Keys` 54,
`WlrLayershell` 50, `Component` 33.

### G17. Qt build and designer files are out of scope (documentation, code-kb)

`.ui` files (1 in Kirigami, 13 across the KDE corpora) and `CMakeLists.txt`
(`qt_add_qml_module`) are unsupported. The claim must say so. QML inside C++
string literals is a literal, not QML.

## Work plan

### Part A: julie-extractors (release 3.2.0, then 3.3.0 for the C++ work)

Companion task list with file pointers:
`~/source/julie-extractors/docs/plans/2026-09-21-qt-first-class-gap-closure.md`.

- **A1 Nested objects become symbols.** Every non-root `ui_object_definition`
  and every `ui_object_definition_binding` (`Behavior on color`) emits a
  symbol: name = its `id` when present, else the type name; kind `field`
  (a leaf in code-kb unless it has children, never skipped as a local, and
  not excluded from identifier containment like `variable`); signature
  `t: Timer` or `Rectangle` (code-kb cuts skeleton signatures at `{`, so the
  id must come before any brace); parent = the enclosing object or inline
  component; metadata `object_type`, `binding_kind: "object"`, and
  `value_source_property` for the `on` form. The root component keeps its
  `id:` property row (nothing else represents the root id) and nested ids drop
  their separate property row. Relationship and pending `from_symbol` values
  stay the enclosing component or function, never the object row, because
  code-kb's impact walk skips edges from `field` rows. Reuse
  `object_id_binding` in `qml/relationships.rs`.
- **A2 Plain property bindings stop being symbols.** `color: "red"` stays a
  `qml.binding.v1` fact only. Signal handlers and declared properties remain
  symbols. Accepted consequences: binding text leaves symbol search (facts are
  not searched); property-target relationships in `qml/relationships.rs` must
  re-anchor to the enclosing object; `tests/qml/bindings.rs`, the capability
  matrix, and every QML golden change. (Alternative if the owner rejects this:
  keep the rows and have code-kb hide `binding_kind: property_binding` rows in
  skeletons. Recommendation: drop them.)
- **A3 Inline components.** `ui_inline_component` emits a `class` symbol named
  from the `name` field, signature `component SpeedDial: Item`, `base_types`
  metadata, children parented to it.
- **A4 Signal signatures.** `signal closeRequested(string reason)` becomes the
  signature; parameter names and types go to metadata.
- **A5 One reference contract for qualified names, base types, and handlers.**
  (a) A `nested_identifier` type usage records the terminal segment as the
  identifier name (`Page`) and the mapper's `receiver_before_identifier` runs
  for `type_usage` as it does for calls, so `metadata_json.receiver` holds
  `Kirigami`. (b) The root object's type emits the same `type_usage` plus a
  structured pending relationship of kind `extends` from the component to the
  base type (terminal name, receiver, import context), so `blast_radius` can
  walk it. (c) Each `on<Signal>` handler, binding form and `function
  on<Signal>()` inside `Connections`, records `handled_signal` metadata and
  emits a `member_access` identifier named after the signal with the handler's
  owner as receiver (`Connections.target` id, the enclosing object's type, or
  none). Property-change handlers (`onXChanged`) point at the property `X`
  with `metadata.change_handler = true`. Unowned matches are candidates, not
  proof; code-kb reports them as such. Identifier metadata needs a field on
  the extractor's `Identifier` struct and the mapper in
  `julie-extract-cli/src/extraction.rs`; SQLite `metadata_json` and JSONL
  already carry it.
- **A6 Attached binding names.** A binding whose name is a
  `nested_identifier` with a capitalized head (`Layout.fillWidth`,
  `Kirigami.FormData.label`) emits a `type_usage` identifier for the attached
  type with the same terminal-name-plus-receiver shape as A5.
- **A7 Pragmas.** `pragma Singleton` sets `singleton: true` on the root class
  and prefixes the signature (`singleton extends QtObject`); every pragma also
  emits a `qml.pragma.v1` fact with name and value.
- **A8 Bare `required` declarations** emit a `property` symbol with
  `required: true` metadata that points at the inherited property name.
- **A9 `qmldir` type rows** change kind from `class` to `export` (the kind
  exists in `base/kinds.rs`), keeping the version and file metadata. Lookup
  still lists both rows; that is honest and stays.
- **A10 QML JavaScript dialect.** Recognize `.pragma` and `.import` directive
  lines at the top of a `.js` file, blank them with same-length spaces so byte
  spans hold, and parse. The pre-pass runs in one shared function used by both
  the extraction path (`pipeline.rs`) and the check path (`syntax/mod.rs`), so
  `julie-extract check` accepts the file and `edit_file` works. Record each
  directive as an import symbol or a `javascript.qml_directive.v1` fact. Tests
  cover comments before the directives, CRLF, and UTF-8.
- **A11 Qt C++ macros (3.3.0).** A token-level pre-pass shared by extraction
  and `check` that keeps every byte position and newline: `Q_OBJECT`,
  `Q_GADGET`, `QML_ELEMENT`, `QML_SINGLETON`, `QML_ANONYMOUS`, `Q_INVOKABLE`,
  and `*_EXPORT` between `class` and the name become spaces; `Q_PROPERTY(...)`,
  `Q_ENUM(...)`, `QML_NAMED_ELEMENT(...)`, `QML_UNCREATABLE(...)`,
  `QML_ATTACHED(...)` become spaces after their balanced argument text is
  captured; `Q_SIGNALS`, `Q_SLOTS`, `signals`, `slots` are recognized as
  section labels, including the `public Q_SLOTS:` form, without rewriting the
  existing access label. Then `Q_PROPERTY` lines become `property` symbols
  with `read`, `write`, `notify`, `member`, `constant`, `final` metadata and a
  `cpp.qt_property.v1` fact; methods in a signals section become `event`
  symbols; slots get `qt_slot: true`; `Q_INVOKABLE` sets `qt_invokable: true`;
  `QML_ELEMENT` and `QML_NAMED_ELEMENT(X)` set `qml_element` on the class.
  Fix the parse first, then re-measure the false `override` prefix, lost
  return types, and doubled constructor rows before patching signatures.
  Forward declarations (`class X;`) stop being emitted as symbols.
- **A12 Multi-line property signatures** keep the declaration head only
  (`property var shellValues`) when the value spans lines.
- **A13 Goldens, capability rows, contracts, docs.** Extend `fixtures/qml`
  with an inline component, a singleton with `pragma`, qualified usages, a
  `BarWidget.qml` that extends `BarWidget`, a `Behavior on`, attached
  bindings, a `Connections` handler, a bare `required`, and a `.pragma
  library` JS file; a Kirigami-style C++ header for 3.3.0; a large generated
  `.qmltypes` near the 1 MiB limit. Classify the output changes per
  `docs/contracts/extraction-output-changes.md` and advance the extraction
  identity epoch for **both** releases. Update `docs/languages/qml.md`; add
  `docs/languages/cpp-qt.md`.

### Part B: code-kb (v1.5.0 after the 3.2.0 pin bump, v1.6.0 after 3.3.0)

- **B1 Skeleton renders objects.** In `formatters.rs`, a `field` (or any
  non-skippable symbol) with children renders as a container
  `localPluginReloadTimer: Timer { ... }` with its children nested; without
  children it stays a leaf. Functions keep their body hidden; `variable` and
  `parameter` rows stay skipped, so other languages do not change. Acceptance:
  `skeleton shell/Ui/Button.qml` under 60 lines, every handler and property
  under its object, and unchanged skeletons for the Rust, TypeScript, and C#
  fixtures.
- **B2 Reference resolution accepts import aliases and base-type edges.** In
  `pending_target_predicate` and the identifier caller query
  (`queries.rs` ~2263 and ~2584): a `target_receiver` or `metadata.receiver`
  that equals the `alias`/`local_name` of an import symbol in the same file
  counts as satisfied, and the target then ranks by name and proximity. An
  `extends` pending row never resolves to its own `from_symbol`. The impact
  walk follows `extends` edges. Acceptance: `refs BarWidget --file
  shell/Ui/BarWidget.qml` lists 12 components; `refs Page --file
  src/controls/Page.qml` in Kirigami includes `Kirigami.Page` roots;
  `blast-radius --file src/controls/Page.qml` lists them as downstream.
- **B3 Type targets match member-access receivers.** When the target symbol
  is a type-like kind (`class`, `struct`, `module`, `enum`, `namespace`), the
  caller query also returns `member_access` rows whose
  `metadata_json.receiver` equals the target name, grouped by file.
  Acceptance: `refs Color --file shell/Commons/Color.qml` lists 53 files;
  `refs Style` lists 63.
- **B4 Test path rule.** `is_test_path`, `test_path_predicate`, and the
  predicate's substring guard add file names starting with `tst_` and the
  `/autotests/` directory; the shared test cases cover both. Acceptance:
  `lookup waitForW` (prefix) and `search "wait for window active"` return
  nothing without `--include-tests`; exact `lookup waitForWindowActive` still
  returns the three rows by contract.
- **B5 Fact aliases.** Add `signal`/`signals`, `import`/`imports`,
  `binding`/`bindings`, `component`/`components` (object instantiation and
  inline components), `module`/`modules` (`qmldir.module.`), and `pragma`,
  each mapped to explicit pattern-id families; the substring fallback stays.
- **B6 Handler candidates.** `refs <signal>` labels handler rows whose
  receiver did not resolve as `candidate` in the output, so unowned
  `clicked` matches are visible but not claimed.
- **B7 Pin bumps.** Pin julie-extract 3.2.0 (later 3.3.0) in
  `scripts/julie-pins.json` with all six archive checksums; the version guard
  rebuilds old indexes; the Windows guest runs the changed tests before push.
- **B8 Docs and claim.** README gains a "Qt and QML" section listing exactly
  the bullets under "What is true today" plus the closed gaps, with the corpus
  commits and numbers; `docs/site/index.html` names QML, `qmldir`, `.qmltypes`,
  Qt C++, and the `.ui`/CMake exclusion; release notes cite the reproduction
  commands above.

### Part C: the claim

Say "first-class" only when every line below passes on the four pinned
corpora through both the CLI and the MCP tool, with positive and negative
cases and unchanged non-QML skeleton fixtures:

1. `skeleton` of any component shows its object tree with ids, declared
   properties, signals with parameters, functions, inline components, and the
   `singleton` marker, and nothing else.
2. `refs <Component>` lists unqualified and qualified instantiations and the
   components that extend it; `blast_radius` follows the same edges.
3. `refs <Singleton>` lists every file that reads it.
4. `refs <signal>` lists emit sites, owned handlers, and candidate handlers
   labelled as such.
5. Test helpers in `tst_*.qml` and `autotests/` stay out of prefix and
   concept search by default and `blast_radius` names the `tst_` file.
6. Qt C++ headers: zero parse diagnostics on `columnview.h`, one `property`
   symbol per `Q_PROPERTY` (38 in that header), signals as events, one row per
   constructor, no forward-declaration rows.
7. `edit_file` succeeds on a `.pragma library` JavaScript file on Linux and
   Windows.

Until item 6 lands, the honest wording is: "Expanded QML and Quickshell
support, validated on pinned corpus revisions; Qt C++ headers are indexed with
known macro gaps; static reference results have documented limits; `.ui` and
CMake files are not indexed."

## Release sequence

1. julie 3.2.0 (A1 to A10, A12, A13 QML parts; epoch advance).
2. code-kb v1.5.0 (B1 to B8 with the 3.2.0 pin).
3. julie 3.3.0 (A11, A13 C++ parts; epoch advance).
4. code-kb v1.6.0 (pin 3.3.0, claim item 6, docs). Publishing julie alone
   changes nothing for code-kb users.

## Verification commands (expected results after steps 1 and 2)

```bash
cd ~/source/omarchy
code-kb skeleton shell/Ui/Button.qml | wc -l          # 71 lines: declared properties,
                                                      # signals, attached handlers,
                                                      # readonly properties, and the
                                                      # nested object tree
code-kb refs BarWidget --file shell/Ui/BarWidget.qml   # 12 extending components
code-kb refs Color --file shell/Commons/Color.qml      # 53 files
code-kb refs clicked --file shell/Ui/Button.qml        # emit sites + onClicked handlers
code-kb lookup SpeedDial                               # class, component SpeedDial: Item
# The point is that the `.pragma library` file passes the syntax check.
# The second command reverts the first, so the file is left unchanged.
code-kb edit-file --old "function clampAlpha(" --new "function clampAlpha( " shell/Commons/BorderGeometry.js
code-kb edit-file --old "function clampAlpha( " --new "function clampAlpha(" shell/Commons/BorderGeometry.js
cd <kirigami>
code-kb refs Page --file src/controls/Page.qml         # includes Kirigami.Page roots
code-kb lookup waitForW                                # nothing without --include-tests
```

After step 4:

```bash
code-kb skeleton src/layouts/columnview.h | head -30   # 38 Q_PROPERTY rows as properties, no parse error banner
```

## Effort (agent sessions)

- julie 3.2.0: 4 to 5 sessions (A5 is the largest item: identifier metadata
  transport, mapper, goldens, epoch).
- code-kb v1.5.0: 2 sessions plus the release preflight.
- julie 3.3.0: 2 to 3 sessions.
- code-kb v1.6.0: 1 session.
- Human time: the A2 decision (drop binding symbols), plan approval, and four
  release approvals.

## Evidence table

| Check | Omarchy | Kirigami | plasma-workspace | Quickshell examples |
| --- | ---: | ---: | ---: | ---: |
| QML parse diagnostics | 0 | 2 (template placeholders) | 0 | 0 |
| QML symbols | 15,800 | 9,570 | 13,738 | 352 |
| Inline components in source / as definitions | 48 / 0 | 21 / 0 | 17 / 0 | 0 / 0 |
| Qualified type usages | n/a | 864 of 2,591 | n/a | n/a |
| Signal handlers / signals | n/a | 352 / 31 | n/a | n/a |
| Member accesses with receiver `Color` / `Style` | 249 in 53 files / 739 in 63 files | | | |
| Components extending `BarWidget` / `Panel` (`base_types`) | 12 / 10 | | | |
| `refs Color`, `refs Style`, `refs BarWidget`, `refs Panel` today | 0, 0, 0, 0 | | | |
| Attached binding heads `Layout` / `Keys` / `WlrLayershell` | 56 / 54 / 50 | | | |
| `onXChanged` handlers | 248 | | | |
| C++ files with parse diagnostics | 0 | 70 of 102 (981) | 636 of 1,030 (4,502) | 0 |
| `Q_PROPERTY` lines / property symbols | 0 | 297 / 0 | 914 / 0 | 0 |
| `tst_` QML files | 0 | 44 | 4 | 0 |
| JS files with `.pragma library` | 1 of 27 (indexed, edit blocked) | 0 | 0 | 0 |
| `.ui` files (all unsupported) | 0 | 1 | 12 | 0 |

## Review record

Codex (`codex exec`, read-only, 2026-09-21) reviewed revision 1 of this plan
and the julie companion. Accepted and folded in: skeleton cannot render
`variable` rows and cuts signatures at `{` (A1 kind and signature, B1);
`slots:` cannot become `public:` byte for byte and `public Q_SLOTS:` exists
(A11); pending rows already split qualified names and `blast_radius` ignores
identifiers (G3, A5, B2); identifier metadata needs a struct field and mapper
work (A5); `variable` rows are excluded from identifier containment and the
root id would vanish (A1); binding removal changes search, relationships,
tests, and goldens (A2); receiver capitalization and name-only handler links
overclaim, so unowned matches are candidates (A5, B6); export rows still show
in lookup and single-symbol resolution already prefers definitions (G8, A9,
former B5 dropped); preprocessing must cover the check path (A10, A11);
exact-name lookup keeps test rows and the SQL guard blocks `tst_` (G10, B4);
`BorderGeometry.js` is indexed, only edits fail (G11); 38 not 17 `Q_PROPERTY`
lines; 12 and 10 extending components, not five; three helper rows; two
template diagnostics qualify the parse claim; pinned commits added; a code-kb
release must follow julie 3.3.0; epoch advance for both releases; all six
checksums. Missed gaps added: G15 bare `required`, G16 attached bindings and
property-value sources, G17 `.ui`/CMake exclusion, property-change handlers in
G6. Not adopted: nothing material; Codex proposed no code.
