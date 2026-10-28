# Reach sections

The callers report (`find_references` for callers, CLI `refs`) adds these sections after it labels the sites. This file holds every rule of each section; `CLAUDE.md` and `AGENTS.md` hold the summary. Change this file with the code.

`reach_sections` in `reference_report/sections.rs` adds up to six sections
after the sites are labeled. They print after the site groups and before the text-only lines,
each only when it has rows. They never change a site's label or basis or a count, so the reach
gate scores the same labels. A heading counts every row; `limit` caps the listed rows of a
section, and one fold line counts the rest by file. `offset` selects the next page of each
section independently. Counts remain complete; restart at offset 0 after edits or an index
rebuild. CLI `--json` adds the arrays `overrides`,
`overridden`, `constructions`, `variant_uses`, `registrations`, and `tests`, and leaves out an
empty one. A section reads indexed rows, opens source only for the files of the sites it
needs, and keeps nothing after the call.

## Overrides

`Overrides (N): calls through `Owner.name` can reach these` applies to a method or function
(never a constructor) whose parent is a class, struct, record, interface, trait, protocol,
enum, or union. A private Java, C#, Kotlin, or Scala method is never dispatched: it has neither
override section, and it is never a row of one. Its rows are the same-named method and function members of every type that
extends or implements the owner, through resolved `extends` and `implements` rows and through
pending rows that name the owner. A type of any of these kinds can be a row, so an enum or a
Rust union that implements a trait or an interface is listed. Only a class, struct, record,
interface, trait, or protocol can be the owner that is extended or implemented. A Python enum
can also be that owner: julie gives every class with an `Enum` base the kind `enum`, and an
enum without members can have subclasses. No other enum can. A pending row counts only when
its type is in the same language family as the owner (JavaScript, TypeScript, TSX, JSX, and
Vue; Java, Kotlin, Scala, and Groovy; C, C++, Objective-C, Objective-C++, and QML; C#,
Razor, VB.NET, and F#; every other language is its own family) and the callers report does not exclude it (a qualified name or a
JavaScript or TypeScript name that an unindexed import binds, a nearer scope). In that check a
constructor with the owner's name is not another definition of the name, because no `extends`
or `implements` clause names a constructor. Then what the
file writes about the name decides, even against a nearer type of that name. A qualifier
written before the name decides first. In Python it needs two or more names
(`pkg_a.base.Base`) whose first name is bound by no import or by a plain `import a.b`, and it
counts only for a class whose module path ends with it (the classes around the base belong to
that path); a single name (`pb.Base`) stays with the callers report. In Java, Kotlin, Scala,
and Groovy it counts only for a type whose package and enclosing types it names: as written
(`b.x.Base`, `b.Outer.Inner`), through an import of its first name (`import b.Outer;` with
`Outer.Inner`), or under the package of the file or a star import. An import of the name
decides next: it counts when it reaches only the owner, and it does not when it reaches other
definitions and not the owner. A Java, Kotlin, Scala, or Groovy import is read as its written
package, enclosing types, and name, so `import b.x.Base;` never ties `b.Base`, and
`import b.Outer.Inner;` ties only an `Inner` nested in `Outer` of package `b`. A Python
`from M import Base` where `M` names no indexed file or folder (the test the callers report
uses for an unindexed import) ties the name outside the project, so no project class counts.
The callers report labels such a bare name `unresolved`; the section follows the import and
leaves that label alone. A star import decides when no import names the name. A Python
absolute `from M import *` counts only for a class whose module path holds `M` as whole names
(the module, or a package above it). The Java, Kotlin, Scala, and Groovy star lines are read
from the source file, because julie records no row for a Java one. A line inside a comment
(`//` or `/* */`) or a string or character literal is not read. A `"""` or `'''` string spans
lines; any other literal ends at its closing quote or at the end of its line. A Kotlin or Scala
block comment nests; a Java or Groovy one ends at its first `*/`. A Groovy slashy string is not
skipped. A type in the package of
the file wins, else only the package or outer type that a star import names counts
(`import b.*;`, `import b.Outer.*;`). These forms stay ambiguous, and the nearest type of that
name wins: a relative Python import or star import, two imports of the name (`try` and
`except`), a name that a Python star module re-exports from outside it, an owner in a Java,
Kotlin, Scala, or Groovy file with no package line, and, in every other language, a qualified
name and a star import. The walk goes 8 levels down and stops at 50 types; when that cuts it,
the `Not visible to this index` line says so. A Rust member counts only when it is in
an `impl Trait for Type` block of the owner trait; an inherent method, the impl method of
another trait, and a method declared in a subtrait are not overrides. The impl header is read
from the source (the nearest `impl` line above the method with less indentation), one file at
a time, and its trait path is kept as written. A path with modules
(`impl crate::ovr::a::Draw for T`) matches only the trait it names. A bare name
(`impl Draw for T`) is read as Rust reads it, as far as the index shows. A trait in the module
of the impl (its file and the inline `mod` blocks around it) wins. Else the `use` rows of that
module that import the name decide, each read as the path it imports from, so
`use std::io::Write;` names no project trait. A `use` row whose path reaches no trait decides
only when it is a std path or an external path; any other path may go through a chain that the
index cannot follow, and it says nothing. A path is external when its first segment is a
dependency name in the nearest `Cargo.toml` (`[dependencies]`, `[dev-dependencies]`,
`[build-dependencies]`, and their `[target.…]` forms; `-` read as `_`; a rename
`a = { package = "b" }` is known as `a`) and no project module, folder, file, package, or crate
has that name. So `use serde::Serialize;` names no project trait. Else the glob `use` rows of
that module decide
(`use crate::shapes::*;`, `use super::*;`), when one reaches a trait of that name. When the
module says nothing, the bare name matches every trait of that name. Every
Rust type has a module path: its file below the package's `src/` (`src/ovr/a.rs` and
`src/ovr/mod.rs` are `ovr::a` and `ovr`; `lib.rs` and `main.rs` are the crate root), then the
inline `mod` blocks around it, which julie records as the namespace parents of the type. An
impl has a module path the same way, from the inline `mod` blocks that enclose its line. The
modules written before the name must equal the module path of the trait, so `crate::ovr::Draw`
names the `Draw` in `ovr` and not the one in `ovr::a`. A leading `crate` names the root of the
package of the impl. `self` names the module of the impl, and `super` its parent; each further
`super` goes up one more. A leading crate name names a package: the `[package]` name in the
`Cargo.toml` of the package folder, else the folder name (`-` read as `_`). The files in
`tests/`, `benches/`, `examples/`, `src/main.rs`, and `src/bin/` of a package name its library
by that crate name. Any other path is relative: it is read from the module of the impl first, and from the crate root
when that names no trait. A relative path that names no trait by either reading names the one
trait whose module path ends with it, and none when two or more end with it. The walk follows
one `use` hop: when the written modules name a module that holds a `use` row for the trait
name (`pub use crate::a::Draw;` in `x`, then `impl crate::x::Draw for T`), the source of that
`use` is read the same way from the module that holds it, and names the trait. It does not
follow a second hop, a glob (`pub use crate::a::*;`), or a `use … as` alias: such a path names
no trait, and its impl is not listed. A path rooted at `std`, `core`, or `alloc` names nothing.
Visibility is not checked. Two limits: a `[lib] name` in `Cargo.toml` is not read, and a bare
std prelude name (`Clone`) with no `use` row still matches every trait of that name. A pending
`extends` or `implements` row of a Rust type decides by the same rules (a path with modules, or
a bare name that its module explains), before the callers report's labels, imports, and the
nearest type.

## Overrides of

`Overrides of (N): calls to these can reach `Owner.name`` lists the same-named members of the
owner's ancestors, nearest first, at most 5, under the same rules; a Rust impl method lists
only the trait of its own impl block, and an inherent method lists none. julie writes no
method-level `overrides` edge, so both lists come from type edges and member names. The
`Not visible to this index` line names
dynamic dispatch only for a Go target (``calls through a Go interface that the index does not
record reach methods this list does not show``), because Go records `implements` only for a
`var _ I = T{}` assertion.

## Constructions

`Constructions (N): path: line, line · path: line` applies to a class, struct, or record
target. N counts lines, and the lines fold per file, files in the order of their first site.
A site that the report labels `excluded` does not count, with one exception: a `type_usage`
site whose basis is ``resolves to `Name` (path:line)`` with the target's name and the target's
file. That basis names another definition of the same name in the target's own file, so the
line still names the target type. A line counts when it holds a counted site of one of three
forms: a call, a `new` (`instantiates`), or a site that the constructor step labels
`builds the class`; a literal of the type (below); or a constructor call that the source
confirms (below).

Literals. In Rust, Go, C, C++, Zig, F#, and Elixir files a `type_usage`, `variable_ref`, or
`uses` site counts when its line opens a literal of the type. The site needs a column, except in
Zig and Elixir files and for a C or C++ `uses` site; a site with no column is read from the start
of its line. The scan looks for the target's name and, when the name has dots (`Probe.Point`),
also for its last segment. The report keeps one site for a caller, a name, and a line, and that
site can be a pattern (`|Foo { a }| Foo { a: a + 1 }`, `Foo { a } => Foo { a: 1 }`). So the
test covers every whole-word occurrence of the name on the line of a site, in code only. The
scan skips a `//` comment to the end of the line, a `/* */` comment (Rust nests them, the other
languages do not), a double-quoted string, and a character or rune literal (`'"'`, `'\''`). It
also skips a Go backtick string, a Rust raw string (`r#"…"#`), a C++ raw string (`R"x(…)x"`,
also with a `u8`, `u`, `U`, or `L` prefix), an Elixir `#` comment, and an F# `(* *)` comment,
which nests. A Rust lifetime (`'a`) is not a character literal. Any of them that does not close
on the line runs to its end. The line counts when any occurrence opens a literal. A literal
opens in these forms:

- Rust and Go: after the name and any generic arguments (`<…>` or `::<…>` in Rust, `[…]` in
  Go), the text starts with `{`. julie writes `Foo { a: 1 }` as a type use, and `Foo` inside
  `vec![…]` or as a field value as a variable read.
- C++: the same form with `<…>` arguments (`Point{.x = 1}`, `ns::Point{…}`, `Point<int>{…}`),
  or a variable declaration: the name, any `<…>` arguments, any `*` or `&`, a variable name, an
  optional `=`, and then `{` (`Point point{.x = 1}`, `Point point = {…}`). The declaration form
  counts after `struct` too (`struct Point point{…}`).
- C: a declaration with an initializer, where the text between the name and `{` holds a
  variable name, then `=`, and no `;`, `}`, `(`, or `)` (`struct Point p = {.x = 1};`,
  `Point p = {…};`). Or a compound literal, where the parentheses around the name hold only the
  name and `const`, `volatile`, `struct`, `union`, or `enum`, and the `(` follows the start of
  the line, `=`, `,`, `(`, `:`, `?`, `return`, `throw`, `sizeof`, or `_Alignof`
  (`return (struct Point){.x = 1};`, `(Point){…}`).
- Zig: `{` follows the name (`Point{ .x = 1 }`), and the name is not in the return type of a
  `fn` signature on the line (`fn make() Point {`).
- Elixir: the name follows `%` and a module path of identifier characters and dots, and `{`
  follows the name (`%Probe.Point{x: 1}`, `%Point{…}`).
- F#: a typed record literal. A `let`, `use`, `member`, or `val` declaration whose type
  annotation names the type and whose value starts with `{` (`let p : Point = { X = 1 }`), or a
  declaration whose value is braces with the type after them (`let p = { X = 1 } : Point`). A
  copy-and-update expression (`{ p with X = 1 }`) names no type and does not count.

In Rust, Go, C++, and Zig such a literal does not count when the token before the name is `->`,
`impl`, `for`, `struct`, `enum`, `union`, `trait`, `type`, or `dyn`, when the name follows the
generic parameters of an `impl` or, in Go, the parameters of a func, or, in Rust, when the
braces close into a pattern. The C++ declaration form above counts after these words too.
The scan climbs out of the groups around the braces (`(…)`, `[…]`, `{…}`), skipping
the rest of each, and reads what follows the outermost one: `=>`, `=`, `|`, `:`, `if`, or `in`
makes a pattern, so `Some(Foo { a, .. }) = o`, `(Foo { a, .. }, y) = p`, and
`Outer { inner: Foo { a, .. } } = o` are patterns and `Some(Foo { a: 1 });`,
`(Foo { a: 1 }, y);`, and `f(Foo { a: 1 }) == y` are literals. The name stands in the
parameters of a closure when the nearest single `|` before it, at its level, comes where a
closure can start: at the start of the text, after `(`, `[`, `{`, `,`, `;`, `:`, `=`, `=>`, or
another `|`, or after `move`, `async`, or `return`. A `|` anywhere else is a bit-or or an
alternation, and a `=>` at that level comes first for the body of a `match` arm, so the `|` in
the pattern of an arm never counts (`1 | 2 => Foo { a: 1 },` followed by `3 | 4 => …` is a
literal). In the parameters of a closure with more than one parameter, a single `|` ends the
parameters. The second argument of `matches!` is a pattern.
The scan stops after 4096 bytes and takes the braces as a literal.

Constructor calls. In C#, Kotlin, Scala, Swift, and Ruby files a `uses` site, and in Ruby also
a `type_usage` or `member_access` site, counts when its source line holds a whole-word occurrence of the name that
any `<…>` type arguments and then `(` follow. In C# the name must follow `new`, with only a
namespace path between them (`new Point(…)`, `new Ns.Point(…)`, `new global::Ns.Point(…)`).
In Scala and Swift, `.apply` or `.init` may stand before the `(` (`Point.apply(…)`,
`Point.init(…)`), and in Ruby `.new` (`Point.new(…)`). The line is read without comments
(`#` in Ruby; `//` and `/* */`, nested in Kotlin and Swift, elsewhere), `"""` strings, and
single- or double-quoted strings.

The edit check reads the other half: `destructurings` in `sections.rs` lists the lines of a
Rust type use whose braces close into a pattern, one row per line, and the report prints none of
them (`docs/edit-check.md`, "Fields: which constructions break").

## Variant uses

`Variant uses (N functions): a match in them may need a new arm` applies to an enum target.
Its rows are ``- `caller` [path:line] names K of M variants``, most variants first, then by
path; the line is the first use. The words are singular for one (`1 function`, `names 1 of 1
variant`). A function names a variant when it holds an identifier whose name is an
`enum_member` child of the enum and that stands right after the enum's name and `::` or `.`
(a path before the enum is fine, so `crate::m::Status::Warn` counts) on the line of a site of
the enum's name that is not `excluded`, or that is a `member_access` on a receiver with the
enum's name in a file that holds such a site. A name that is not a member access counts only
when `Enum::` or `Enum.` stands before it, so a variant name with no enum name before it
(`Ok(n)` next to `Status::Warn`) does not count. The exceptions are the C and C++ rule here
and the three spellings after it. In a C or C++ file, a bare
enumerator of an unscoped enum counts when it is a variable read on the line of a site of the
enum (`enum Color c = RED;`). It also counts when it is a variable read in a function that
holds a site of the enum, as found from the identifier rows of that function (`case RED:` in a
`switch (c)` under the parameter `enum Color c`), unless that function declares a symbol of the
same name (a local `int RED`). That rule also holds on the line of a site, so a parameter
(`int f(enum Color c, int BLUE)`) or a local that has the name hides the enumerator in the
whole function. An enum is unscoped when
the source at its start does not read `enum class` or `enum struct`; a scoped C++ enum and
every other language keep the qualified rule. julie stores the enumerators of a C enum as `constant` rows, so a C enum takes its
`constant` children as its variants. Code outside any symbol shows as `(no enclosing symbol)`.
The edit check (`docs/edit-check.md`) runs the same finder with the old names of a removed variant.

Three more spellings count.

- Rust `Self::V` counts in a method of an impl of the enum, an inherent impl or a trait impl.
  The method is found as a symbol whose parent is the enum. julie writes a `type_usage` row for
  `V`. `Self::V` in an impl of another type, or in a free function, does not count.
- A bare Rust `V` counts when an import in the function's scope names the variant (`use
  …Enum::*;` or `use …Enum::{…V…};`) and the function holds a site of the enum that is not
  `excluded`, or is a method of the enum. A parameter or a local typed as the enum is such a
  site. An item renamed with `as` brings in no name. julie writes no row for a bare name in a
  pattern, so the lines of the function are read, with comments, strings, and character
  literals blanked. The name must stand bare: not after `::` or `.`, not before `::` or `!`,
  not after `struct`, `enum`, `fn`, `type`, `mod`, `trait`, `union`, or `as`, and not inside a
  `use` item. An import is in scope when it is inside the function, or in the module that
  holds the function, or, for a function with no module around it, at the top of the file. So
  a `use` at the top of a file does not reach a function in a `mod` block, and a `use` in a
  `mod` block does not reach a function outside it. The scope of a method of the enum is the
  scope of the enum.
- A Java bare `V` counts when `case` comes right before it on its line, or before a
  comma-separated list of names that ends with it (`case A, B ->`), in a function that holds a
  site of the enum that is not `excluded`. A parameter or a local typed as the enum is such a
  site. julie writes a `variable_ref` row for it. A function that declares a symbol of that
  name does not count it, as for a C enumerator. A bare name that does not follow `case` does
  not count.

Not read: a Rust `use` item that renames a variant, a `mod` block that holds an impl but not
the enum, a local type or explicit import that hides a glob-imported name, and a bare name in
a `when` or `switch` of any other language. Those languages keep the qualified rule.

## Registrations

`Registrations (N):` rows are ``- {what} [path:line]``, with ` (by name)` after a name match.
For a fact whose pattern id holds `.route.`, `{what}` is `route {VERB} {template}`, where the
template is `effective_route_template`, else `route_template`, else the pattern id, and the
verb is left out when the fact has none; for any other fact it is the pattern id. The facts
read are the route-family facts (`ROUTE_FAMILIES`) plus the Spring `.request_mapping.` and
Django `.url_pattern.` facts. A fact registers the target by id when its containing symbol is
the target and its `api_style` is `decorator_routing`, `attribute_routing`,
`annotation_routing`, or `attribute` (Flask and FastAPI decorators, ASP.NET attributes, Spring
annotations, actix attributes). The containing symbol of a controller-level route is its
class, so the row belongs to the class. A `call_routing` fact never registers its containing
symbol, because that symbol is the code that registers the route. A fact in any file also
registers a target of any kind by name when the metadata key `view_target`, `handler`,
`handler_name`, `controller_action`, `view_name`, or `function_name` equals its name or ends in
`.` and its name (`views.cart_view` names `cart_view`), or `handler_names` holds such a value;
a value that starts with `self.`, `cls.`, or `this.` (`self.index`) names a member of the class
around the fact, so it names only a target whose parent is a class, struct, record,
interface, trait, protocol, enum, or union. That row says `(by name)`. A by-name row is always kept when a `proven` or `likely` site of the
target is on the lines of the fact. Otherwise it is dropped when every site of the report on
those lines is `excluded`, or when the target is in another file and the file of the fact
defines a function, class, or variable of that name (a constant counts as a variable) that a
bare name can reach: at module level, or local to a function. A symbol whose parent is a
function, a method, or another member that is not a type, a module, or a namespace is local to
that parent, and it counts only when the parent holds the first line of the fact. A member of
a class or another type (a method, a class attribute) and an import never drop a row. So a
same-named function in another file gets no row from a file that has its own, a
`Service.list_users` method does not hide the registration of an imported `list_users`, and a
parameter `list_users` of another function in the file does not either. Rows tied by id come first. Every path
in a section uses `/`. The query skips through the distinct route pattern ids instead of
scanning the facts.

## Tests that call it

`Tests that call it (N):` rows are ``- `test` [path:line] run: command``, by path and line,
without ` run: …` when no command is known. The text row writes each control character of a
test name as an escape (`\n`, `\t`); JSON and the site rows keep the name as it is. A row
shows ` (name only)` before ` run:` when only `likely` sites with the basis `name only` tie the
test to the target, and JSON then sets `name_only: true`. A test is
the caller of a `proven` or `likely`
site when julie flags it as a test or its name reads like one in a test path
(`names_a_test`), or else the test function that holds a helper caller, up to 4 levels up.
A Python or Go function inside another function is a helper even when its name reads like a
test: pytest does not collect it, and julie names a Go `t.Run` closure by its label, which
`go test -run` cannot select alone. A helper folds into the test around it. A class defined
inside a function is part of that function, so its tests and its other methods fold into the
function. Setup, teardown, and fixture members are not tests,
and neither are helpers inside them or the target itself. `CommandFinder` in
`test_commands.rs` builds the command from marker files under the project root. It reads each
folder once, never looks above the root, and returns no command for an unknown language or
runner. A command runs from the project root. When its marker file is in a folder below the
root, the command starts with `cd dir && ` (`dir` is quoted like a path). The markers are
`go.mod`, `package.json`, `Cargo.toml`, `pom.xml`, `build.gradle`, and `build.gradle.kts`, and
a root `Cargo.toml` with a `[workspace]` table needs no `cd`, because `cargo test -p package`
finds the member from the root. `pytest` reads no marker, so it takes no `cd`, except for a
folder that a node id cannot hold (below). A path in a command stays as written when it holds only
`A-Z a-z 0-9 _ . / -`; any other path is wrapped in single quotes, and each `'` in it is
written `'\''`. That covers the file path of pytest (`'tests/a b.py'::name`), vitest, jest,
mocha, and node, the `--test` stem of cargo, and the `./dir` of go. The commands of Maven,
Gradle, and dotnet hold no path, but their class and test names are quoted the same way:
Maven as shell words, Gradle in single quotes, and dotnet in double quotes with `"`, `\`, `$`,
and a backtick escaped. Python is `pytest path::Outer::Inner::name`: every enclosing class,
outermost first, and none for a module function. A test has no command when pytest would not
collect it under its defaults (`python_classes = Test*`; a config that changes them is not
read): every class around it must be named `Test…` or derive from `unittest.TestCase`, and no
class around another may be a `TestCase`, because pytest takes only the methods of one. A
class derives from `TestCase` when a base name ends in `TestCase` (`unittest.TestCase`,
`APITestCase`), or a project base class of that name does, through `extends` rows. pytest
rejects a node id whose path holds `[`, and `-k` does not help, because the path argument is
rejected too. It also splits an id at `::`. So a test in a folder with `[` or `::` runs from
that folder (`cd dir && pytest file::name`), with that folder as the working directory. A file
name with either has no command. Rust is
`cargo test -p package --test stem name` for a file directly under a package's `tests/`, else
`cargo test -p package name`, where `package` is `[package] name` of the nearest
`Cargo.toml` (`cargo test name` without one). A test marked `#[ignore]` or `#[ignore = "…"]`
in the attribute lines above its function ends with ` -- --ignored`, because cargo skips it
otherwise and exits 0 with 0 tests run. Go is
`go test ./dir -run '^name$'`, with `dir` relative to the nearest `go.mod` folder (`.` for
that folder), for a top-level function in a `_test.go` file whose name is `Test`, `Fuzz`, or
`Example` alone or followed by a character that is not a lowercase letter; a `Benchmark…`
function is `go test ./dir -bench '^name$' -run '^$'`. A function that `go test` does not run
by name has no command, because `go test` prints `ok … [no tests to run]` and exits 0 for it:
a method (a testify suite method), a helper with a `*testing.T` parameter, `TestMain`, and an
`Example` with no `// Output:` or `// Unordered output:` comment in its body (`go test`
compiles it and runs nothing). JavaScript and TypeScript follow the nearest `package.json`:
`vitest`, `jest`, or `mocha` in its dependencies or `scripts.test` gives
`npx vitest run path -t "name"`, `npx jest --runTestsByPath path -t "name"`, or
`npx mocha path --fgrep "name"`, `node --test` in `scripts.test` gives
`node --test-name-pattern "^name$" path` (the anchors stop `plain one` from also running
`plain one more`), and anything else gives no command; `path` is
relative to that folder. `jest` reads a bare path as a regular expression and `node --test`
reads it as a glob, so a path with `( ) [ ] + $` or `{ } * ? !` could run 0 tests;
`--runTestsByPath` and running the file itself avoid that. Java is
`mvn test -Dtest=Class#name` under a `pom.xml` and `./gradlew test --tests 'Class.name'` under
a Gradle build (`gradle` without a `gradlew`). Both run from the folder of the nearest build
file (`cd dir && …`), and the wrapper is named by its path from that folder (`./gradlew`,
`../gradlew`). `Class` is the enclosing classes joined with `$`, outermost first
(`Outer$Inner`), and the quoting stops the shell from reading the `$`. C# is `dotnet test --filter "FullyQualifiedName~Class.name"` with the innermost
class. Java and C# need the class.
julie gives a JavaScript or TypeScript test name as the text of its source literal, so the
command first cuts `name` at each `${…}` hole of a template literal and decodes the escapes in
each part. A hole runs to its matching `}`. It counts nested braces and skips the strings
inside it (`'…'`, `"…"`, and `` `…` `` with holes of its own), so `${"}"}` is one hole. A `\$`
before `{` is a literal `$`, and a `${` with no matching `}` is literal text. A backslash
before a line break (`\n`, `\r\n`, `\r`, U+2028, or U+2029) is a line continuation, and both
characters vanish. In a part, `\n`, `\t`, `\r`, `\b`, `\f`, and `\v` become the control character; `\xHH`, `\uHHHH` (a pair of surrogates is one
character), `\u{H…}`, and the legacy octal escapes `\0` to `\377` become the character they
name; and any other `\c` (`\'`, `\"`, `` \` ``, `\\`) becomes `c`. A malformed `\x` or `\u`
loses its backslash and keeps the letter. A decoded NUL or a lone surrogate is a character a
command line cannot carry, so it cuts the name like a hole does. `vitest` and `jest` take `-t`
as a regular expression, and so does `node --test-name-pattern`, so the command joins the parts
with `.*`, puts a `\` before each of
`\ ^ $ . * + ? ( ) [ ] { } |` in a part, writes a newline, tab, or return as `\n`, `\t`, or
`\r`, any other control character as `\xHH`, and U+2028 and U+2029 as `\u2028` and `\u2029`,
and turns each `test.each` placeholder (`%s %d %i %f %j %o %O %p %#`, and `$name` with optional `.path` segments) into `.*`, so the
command runs every case of the table, and turns `%%` into `%%?`: `test.each` prints one
percent sign for it and a plain test keeps both. `mocha --fgrep` matches its string as a plain
substring, so a regular expression character needs no escape, and the command keeps the
decoded name, placeholders and `%%` included. A name with a hole has no substring that spans
it, and neither does a name with a character that would split the command line (a control
character other than a tab, U+2028, or U+2029). So `mocha` gets the longest piece between
them that holds more than blanks (the first of equal pieces), and gets no name filter
(`npx mocha path`) when no such piece is left. In a double-quoted name,
`"`, `\`, `$`, and a backtick then get a `\` before them.
