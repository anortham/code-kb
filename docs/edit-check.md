# Edit check

`check_edit` (CLI `code-kb check`) compares the working tree with a base revision after you edit. It lists the symbols whose shape changed and every site of each that your diff did not reach. The `Stop` and `SubagentStop` hooks run the same check. This file holds every rule; `CLAUDE.md` and `AGENTS.md` hold the summary. Change this file with the code.

The code is `edit_check.rs` and `edit_check/` in `code-kb-core` (`edit_check_op` in `ops.rs`, `format_edit_check` and `format_stop_reason` in `formatters.rs`) and `hooks.rs` in `code-kb-cli`.

## Calls

- MCP `check_edit(project_root, base?)`. `base` is a git revision, optional.
- CLI `code-kb check [--base REV] [--json]`. The text equals the MCP text. `--json` prints the whole check: `rev`, `source` (`given`, `session`, or `head`), `files_changed`, `changes` (each with `kind`, `display`, `path`, `line`, `change`, `in_diff`, `proven`, `likely`, `unresolved`), `tests`, `cuts`, and `diff_hash`. A site has `path`, `line`, `caller`, and, when the note applies, `note`.
- `code-kb hook Stop` and `code-kb hook SubagentStop` read the hook JSON on stdin. `code-kb hook SessionStart` records the base.

## Text

```
check_edit: working tree against fb5628e809 (HEAD), 2 files changed
1 shape change:
  function `area` (src/geometry.rs): parameters 2 → 3
    In your diff (1): src/lib.rs:4
    Not in your diff, likely (1):
      src/lib.rs:8  `second`  passes 2 args; takes 3
Not checked: calls inside macro arguments, functions passed as values.
```

- Header: `check_edit: working tree against {rev} ({source}), {n} files changed`, then `{k} shape changes:`. One file or one change uses the singular.
- No shape change: `check_edit: working tree against {rev} ({source}): no symbol changed shape.`
- Each change: `{kind} \`{display}\` ({path}): {change}`, then `In your diff ({a}): {path}:{line}, …`, then `Not in your diff, proven ({p}):` and `Not in your diff, likely ({l}):` with one row `{path}:{line}  {caller}  {note}` per site, then `Unresolved outside the diff: {u}`. A list shows 12 rows and folds the rest. The `In your diff` line ends with `, … N more`. A `proven` or `likely` list ends with a row `… N more proven` or `… N more likely`.
- `Tests to run: {command}; …` lists the commands of the tests that call a changed symbol, as the `Tests that call it` section of `find_references` gives them (`docs/reach-sections.md`). It shows 8 and folds the rest; the check keeps 50.
- `Not checked: calls inside macro arguments, functions passed as values.` ends every answer that lists a change.
- A cut prints as a `Cut: …` line after the changes and before the tests. A check that stops early still prints what it has.

## The base

- `base` given: that revision. Else the session base (below). Else `HEAD`.
- `{source}` is `given`, `session base`, or `HEAD`. `{rev}` is the revision as written for a given base, a 10-character short sha for the others, and `no commits` for a repository with no commit (every file is then new, so no symbol changed shape).
- An empty `base` string means no base. A revision that git does not know, or that starts with `-`, is an error that names it. A folder that is not in a git repository is an error.
- A session base that git no longer has (a rebase dropped it) counts as none, and the check uses `HEAD`.

## Changed files

- `git diff --name-status -M --relative` against the base lists the tracked files (renames paired by git's default 50% similarity). `git ls-files --others --exclude-standard` adds the untracked files. Git never writes to the repository.
- A file counts when the index holds it, or holds a file with its extension. Excluded paths, folders (submodules), and binary base files are skipped.
- At most 200 files are checked. `Checked 200 of N changed files.` states the cut.
- New-side hunks come from `git diff -U0`; an untracked file is one hunk of the whole file. A site is "in your diff" when its lines overlap a hunk.
- The check refreshes each changed file in the main index first, like a body read. Nothing else writes to `artifact.db`.

## Base symbols

- Each changed file that existed at the base is written from `git cat-file blob` into `<root>/.code-kb/check/<pid>/tree/<path>`. One `julie-extract scan --level references` runs on that tree into `<root>/.code-kb/check/<pid>/base.db`.
- The folder is removed after the check, also after an error. A folder older than 1 hour that a dead process left is removed at the next check. `.code-kb/` holds a `.gitignore` with `*`, so none of it enters a commit.
- One check runs at a time in a process.

## Shape changes

Symbols of both sides are keyed by the names of their parents, their kind, and their name. When the counts of a key are equal, they pair in source order. Otherwise symbols with an equal signature pair first, so a new overload does not shift the others. A symbol that stays unpaired is removed or added. An added symbol is never a change. Local variables and parameters are not symbols for this step, and neither is a Python lambda, because julie names it after its line (`lambda_363`) and a line shift would read as a rename. A constant, a variable, or a type alias is a symbol for this step only at the top level of its file, when julie marks it `public` (another file can import it: a TypeScript `export`, a Rust `pub`, a capitalized Go name, a Python name without a leading `_`), and not in a document language.

A removed symbol whose name stays in the same scope as another kind of the same group is no change, because the uses of the name still compile: `type X` became `interface X`, or `const f` became `function f`. The groups are class, struct, interface, trait, enum, record, and type alias; and constant, variable, and function. A removed type alias whose name a removed constant or variable of the same scope also has (`const X` beside `type X = keyof typeof X`) reports once, under the value, whose sites cover the uses of both. A removed constant, variable, or type alias that the old file defines more than once in one scope (`text_type` in both branches of `if PY2:`) reports once. Removed overloads of a callable each report, since their argument counts differ.

| Change | Symbols | Rule |
|---|---|---|
| `removed` | function, method, constructor, class, struct, interface, trait, enum, record, and an exported top-level constant, variable, or type alias | A base symbol with no pair. Its members are not listed apart. |
| `renamed to \`new\`` | the same | One unpaired current symbol has the same parents, kind, and body hash, and no other symbol on either side has that hash. |
| `parameters {a} → {b}` | function, method, constructor | The accepted argument range changed (the rules of `docs/argument-counts.md`). When one side has no range, the text shows the parameter row counts. When the two range texts are equal (a `self` parameter came or went), the text shows the row counts, and `parameters changed` when those are equal too. |
| `signature changed` | function, method, constructor | The parameters accept the same range of counts, and the signature differs after the normal form below. A return type, a parameter type, a bound, a `where` clause, or a `throws` clause that changed is one. So is a reorder of the parameter names. A bound counts in Rust, Java, C#, and Kotlin. The TypeScript signature in the index holds no type parameter and no bound, so a change of them is no shape change. |
| `signature changed: {words}` | function, method, constructor | A word that changes how a caller must call it came or went: `async` (Rust, Python, JavaScript, TypeScript, C#), `suspend` (Kotlin), `static` (Java, C#, JavaScript, TypeScript, C++, PHP, Swift, Dart; in C++ only for a member of a class or struct, because a file-scope or namespace-scope `static` gives internal linkage and no caller sees it). The text lists each word as `async added` or `static removed`, joined by `, `. The words are judged before the rest of the signature, so a change of a word and of a type shows the words. See "Signatures: the normal form". |
| `parameter renamed: {old} → {new}` | function, method, constructor in Python, Kotlin, C#, and Swift | The signatures are equal in the normal form, and only the parameter names differ. Every rename is listed (`a → x, b → y`). In every other language a rename is no shape change. |
| `fields +{a} (names), -{b} (names)` | class, struct, record | Children of kind `field` or `property` gained or lost. A Python `@dataclass` also counts annotated class-body variables. A `self.`, `this.`, or `cls.` attribute is not a field. |
| `variants +{a} (names), -{b} (names)` | enum | Children of kind `enum_member` gained or lost (`constant` for a C enum). |

A Rust method keeps its owner when its `impl` block is in another file than its type.

## Sites

The sites of a change come from the callers report of the current symbol (`reference_report_for_edit`: no text scan, and no argument-count exclusion, because a call with the old count must stay visible). Excluded sites never count.

| Change | Sites listed |
|---|---|
| `parameters` | Call sites (`calls`, `call`, `instantiates`). A call that spreads its arguments (`f(*args)`, `f(**kw)`, `f(...xs)`) has no count: it is counted as unresolved and not listed, unless the diff reached it. A site that carries a count the new range accepts is dropped unless the diff reached it. A site with a count the range rejects has the note `passes {n} args; takes {range}`, and it is never "in your diff", even when a hunk holds its line (a new file that still calls with the old count, or an edited line that keeps a second call with it). A Go call with one argument counts as rejected when the new minimum is above 1; the callers report keeps such a call because `f(g())` may spread. Then the overrides below. |
| `signature changed` | Call sites, then the overrides below. Every call is a site and carries no count note, because the call must change whatever its count is. A call that a hunk reached is "in your diff". |
| `parameter renamed` | The call sites that pass an old name as a named argument (below). Never "in your diff": a call that still names the old parameter is stale even when a hunk holds its line. |
| C# overloads | For a `parameters` or `signature changed` row of a C# method, a call outside the diff is dropped when julie gives the type of every argument (`arg_types` on the `call` identifier rows of its line) and the types equal the parameter types of another overload of the same name and owner, and not those of the changed one: C# binds that overload. A type name is compared without `global::`, its namespace, or whitespace, and a framework name reads as its keyword (`System.String` is `string`). An overload with a default value or a `params`, `ref`, `out`, `in`, or `this` parameter takes part in no comparison. |
| overrides | The `Overrides` section of the callers report: the same-named members of the types that extend or implement the owner of a changed method. The section gives no label per row, so a row is `likely`; its caller is `Owner.name`. It is "in your diff" when a hunk touches a line of the member. |
| `removed`, `renamed` | Every site of the old name that the report does not exclude, and the import lines of the old name that name the file of the symbol (the note is ``imports `name` ``, the label `likely`, no caller; a Rust `use` line is read from the pending `imports` rows, by the last module segment; a JavaScript or TypeScript source with a script extension, `./util.ts`, names the module `util`, and a relative source must resolve to the file of the symbol, so `import … from "./helpers/parseUtil"` in `src/types.ts` is not an import line of `deno/lib/helpers/parseUtil.ts`). A top-level symbol only: a member cannot be imported, except in Java. julie writes no source for a Java import, so its path is read from the line: a removed class lists `import p.G;` and `import static p.G.m;` or `import p.G.Inner;` of its members, and a removed member lists `import static p.G.name;` or `import p.G.name;`, when the path names the class file of the symbol. A removed symbol that nothing uses is a change with no site and no cut. A pytest fixture (a Python function under a `fixture` decorator) also lists the tests and fixtures in its scope (its file, or the folder of its `conftest.py` and below) that take a parameter with its name (its `name=` when the decorator gives one): the parameter line with the note ``takes the fixture `name` `` and each line of that function's body that reads the name, all `proven`. A user that a fixture with that name in its own file or in a deeper `conftest.py` serves is left out. When other overloads of the name remain, see "Removed overloads" below. |
| `fields` | The `Constructions` rows of the callers report that the change breaks (below). A construction is "in your diff" when any line from its name to its closing bracket changed. |
| `variants` | The `Variant uses` rows (`docs/reach-sections.md`). For an added variant, only a function that needs a new arm (below). For a removed variant, the functions that still name it (below). A function is "in your diff" when a changed line inside it names an added variant. A function that names a removed variant is never "in your diff". |

### Removed overloads

A removed or renamed function, method, or constructor can leave overloads under the same name in the same owner (Java, C#, Kotlin, and C++ overloads; a Python `@overload` stub). The key is the one that pairs symbols: parents, kind, and name. The old report listed every call of the name. Now a call is judged against the overloads that remain:

- **Fits only a remaining overload.** The removed overload did not take the count. The call is not listed.
- **Fits no overload.** The removed overload did not take it either, so the removal did not break it. It is not listed.
- **Fits only the removed overload.** The call is a site with the note `passes {n} args; no remaining overload takes it` (`arg` for 1). It is never "in your diff". A site that the callers report labels `unresolved` is counted as unresolved and not listed, also here (a Kotlin or C# constructor shares its name with its class; a C++ free function or a Python `@overload` stub leaves another definition of the name).
- **Fits the removed overload and a remaining one.** The count does not say which one the call ran (`b.norm("x")` when `norm(int)` stays and `norm(String)` goes). The call is counted as unresolved and not listed, never dropped and never a site. A removed overload with no range takes every count.
- **No count, or a spread.** The call, and any site that is no call (a type use, a read), is counted as unresolved and not listed. A count that the remaining overloads cannot judge counts the same.
- **Import lines.** None are listed, because the name is still defined.

When overloads remain and the index gives them no argument range, every site of the name is counted as unresolved and none is listed.

A Java or Kotlin constructor has a range when the index lists every constructor of its class: a Java class that is no record, and a Kotlin class whose header holds no parenthesis (no primary constructor, no call of a superclass). A Java record has an implicit canonical constructor and a Kotlin primary constructor is no symbol. When the class has one of these, the constructor that stays has no range, so every site is counted as unresolved, also when no listed constructor remains. A Java class whose only constructor is removed keeps the old behavior (every site). julie writes no argument count for a Java `new Foo(a)`, so the count is read from the source line of the site, as for a record.

The rule is off, and the old behavior stays (every site of the name, with the import lines), when the symbol is not a function, method, or constructor, when the index gives a method no owner (a Go method shares its key with the methods of every type), and when no overload remains. The count of a call comes from the callers report, else from the `call` identifier rows on its line (the count and the spread flag that julie writes), else, for Java, from the source line.

### Signatures: the normal form

The check compares the text of the two signatures. Before it compares, it drops what a caller cannot see:

- Whitespace, modifiers (`pub`, `public`, `override`, and the like), decorators, and `pub(…)`. Three words stay: `async`, `suspend`, and `static`, in the languages of the "Shape changes" table. A caller must call an `async` function with `await`, a `suspend` function from a coroutine, and a `static` method on the type and not on a value. The check reads them from the source: up to 1024 bytes from the start of the symbol, cut at the name (or at the first `(` when the name stands elsewhere, as in `const f = async (a) => a`), with comments and strings blanked. A TypeScript signature in the index leaves `async` out, which is why the source is read. When the file cannot be read, the signature serves.
- A Java or Kotlin parameter annotation (`@Deprecated`, `@Size(min = 1)`, `@param:Foo`), in the parameter list.
- A C# attribute (`[Obsolete]`, `[HttpGet("/a")]`, `[A, B(1)]`) and a PHP attribute (`#[Pure]`) at the start of the signature or of a parameter (`[Range(0, 1)] int a`). Every group in a row goes, and a group ends at its matching bracket, also with a string inside it. A `[` that does not start the text stays (`int[] a`). The `async` and `static` words are read after the attributes, so an attribute that holds the name of the method hides nothing.
- A Go result list written with names: `(n int, err error)` equals `(int, error)`, `(x, y int)` equals `(int, int)`, and `(n int)` equals `int`.
- In the parameter list, for each parameter: its own name, the other names of a Go group (`v, k int` reads as `int`, so `a, b int` equals `a int, b int`), a Rust binding `mut` (`mut a: i32`; the `mut` of `&mut T` stays), a Java `final`, and the default value (the text after a top-level `=`).
- In Python also every annotation and the `-> T` return annotation. A Python parameter keeps only its marker: `*`, `**`, and the bare `*` and `/` that make parameters keyword-only or positional-only. So a Python return type change is no shape change.
- A Go receiver name, and the body of a Kotlin or C# function written as `= expr` or `=> expr`.

The text before the parameter list (generics, bounds) and after it (return type, `where`, `throws`) stays, as far as the signature in the index holds it. The TypeScript signature leaves out type parameters and bounds. The check reads the list piece by piece and matches each piece to a parameter row of the symbol (a Go group is one piece for all its rows). When a piece has no row (a Rust pattern such as `(a, b): (i32, i32)`, a TypeScript destructured parameter), or a row has no piece, the whole signature compares as plain text, as before. A Rust `_: u8` parameter has no row; the check reads it as a piece with the name `_` and counts it in the parameter total, so `_: u8` and `_unused: u8` are no change. A symbol with no parameters is normal-formed too, so a Python function that gains `-> int` is no shape change.

Names are compared only after the normal form is equal. The same names in another order (a swap) is `signature changed`. Different names are `parameter renamed` in Python, Kotlin, C#, and Swift, and no change elsewhere.

### Renamed parameters: the named argument

A call is a site of `parameter renamed` when it passes an old name as a named argument: `old=` in Python (not `old==`), `old =` in Kotlin, `old:` in C# and Swift. The check reads the source lines of the call (its site span, or up to the bracket that closes the call when the report gives only the first line), from the callee name on, with comments and strings blanked. The name must stand right after `(` or `,`. Only the parentheses of the call itself are read: a named argument of another call on the same line (`g(1); f(2); h(old=4)`) does not count. Only the arguments of the call itself count, split at the commas outside brackets: an argument is named when it starts with the old name and the separator. A named argument of a nested call counts only when that call has the name of the changed function (`f(f(old=1))` counts, `f(g(old=1))` and `f(1, b=[g(old=1)])` do not). The note is ``names parameter `old` ``. A Swift parameter with a label (`label name: Int`) is compared by its inner name. Renaming only the inner name lists no call.

### Fields: which constructions break

The sites of a field change are the `Constructions` rows that must name every field:

| Type | Field added | Field removed |
|---|---|---|
| Rust struct | A literal `Foo { … }` with no `..base` element (`..Default::default()` is one). A `Self { … }` literal in a method that the index parents to the struct, judged the same. A destructuring pattern `Foo { a, b }` with no `..`. | A literal that names the field, as `b: 2` or the short form `b`. A `Self { … }` literal that names it. A destructuring pattern that names it. |
| Go struct | A literal with at least one element and no key (`Foo{1, 2}`). A keyed literal (`Foo{X: 1}`) and `Foo{}` are not sites. A slice, array, or map literal (`[]Foo{{1, 2}}`) is judged by its `{…}` elements. | An unkeyed literal, or a keyed one that keys the field. |
| Python `@dataclass` | A call that builds it, when an added field has no default. `c: int = 0` has one. A `field(…)` call has one only when it passes `default=` or `default_factory=`; `field(init=False)` keeps the field out of the constructor, so it needs no argument; `field(repr=False)` has no default. An annotation of `ClassVar` (also `typing.ClassVar[int]` and the quoted form) is no field. | Every call that builds it. |
| Java record | A call whose argument count equals the old component count, when the record gained a component. A line that starts with `this(` is a constructor handing its arguments to the canonical one: it is a site when its count equals the old component count, and no site with another count. When the source line gives no count, the call stays a site. | The same. |
| Any other class (TypeScript, Java, C#, Kotlin, and a plain Python class) | None. | None. |

A constructor that takes the new field is a parameter change of its own, listed on its own. A Rust or Go construction is read from its source lines (comments and strings blanked); a literal that does not close within 80 lines counts as a site. A Rust call such as `Foo(1, 2)` (a tuple struct) and `Foo::new(…)` are not literals of the name. A Rust `Self { … }` and a pattern are read like a literal (the sites of `Self` take the label `proven`, and the caller is `Struct.method`).

A Go collection literal is judged by its elements, also when the element type is qualified by a package: `[]geom.Point{{1, 2}}`, `[]*geom.Point{{1, 2}}`, and `map[string]geom.Point{"a": {1, 2}}`.

A Java record with an extra constructor: a call that passes another count than the old canonical one is no site, whether it is `new Point(1, 2)` or `this(x, 0)`. The count is read from the source of the call, from its name to its closing bracket, with comments and strings blanked. A type argument list after the name (`new Pair<>(…)`, `new Box<String>(…)`) is skipped. The arguments split at the commas outside round, square, and curly brackets, and a string argument counts.

### Variants: the new arm

An added variant needs a new arm only in a `match`, `switch`, or `when` in which every arm names a variant of the enum and no arm is a fallback. The check reads the lines of each function that has a `Variant uses` row (its span in the index, with comments, strings, and character literals blanked) and finds each such construct:

| Language | Construct | Fallback arm |
|---|---|---|
| Rust | `match … { pattern => … }` | `_`, a lone lowercase name (`other`, `ref other`), or `name @ _`, in an arm with no guard |
| Java, C#, C, C++, JavaScript, TypeScript, Go, Swift, Dart | `switch … { case … }` | `default:`, `default ->`, or `case null, default` |
| C#, Dart | `switch { pattern => … }` | `_`, in an arm with no guard |
| Kotlin | `when { … -> … }` | `else ->` |
| Python | `match x:` with `case` lines | `case _:` or a lone lowercase name, with no guard |

An arm names a variant when one of its patterns or labels holds a variant of the enum written after the enum's name, after `Self`, or after a bare `.` (Swift `.red`), or written bare where the scope allows it: always in Java, C, and C++, and in Rust when the file holds a `use` item with `Enum::`. A guard does not change this: `Color::Red if ok =>` names a variant and is not a fallback. A guarded wildcard (`_ if ok =>`, `_ when ok =>`, `case _ if ok:`) is not a fallback either, but it names no variant. A C# pattern that starts with `not` (`not Kind.Circle =>`, `not (Kind.A or Kind.B)`) matches every other value, so it is no fallback arm and names no variant. A nested construct is judged on its own.

- **Site.** Some construct has no fallback arm and every arm names a variant. The function is listed with its label and placed in the diff as for any site.
- **Unresolved.** Every other function. Its construct has a fallback arm, or an arm that names no variant (`Some(_)`, `None`, `(_, _)`, `(_, false)`, `_ if ok`, C# `var other`, `Kind other`, or `not Kind.A`, Python `case (_, _):` or `case [x, y]:`, Kotlin `is Other ->`, a `case 7:` label). Or a construct cannot be delimited (its braces do not close, a Python `match` has no `case` line). Or the function holds no such construct: it builds a variant, or uses `matches!`, `if let`, `==`, an `if` chain, a dictionary or a `Record` keyed by variants, or a return value. Or the function has no span in the index, or its file cannot be read, or its language has no reader above. The function is counted and never listed, so it never blocks the Stop hook. One that the diff reached is not counted.

### Variants: the removed variant

The check does not read constructs for a removed variant. It runs the `Variant uses` finder again with the old names of the removed variants. The base index holds them. The finder reads every spelling it reads for a report: `Enum::V`, `Enum.V`, `Self::V` in an impl of the enum, a bare `V` that a Rust `use` brings in, Java `case V`, and the bare enumerators of a C or C++ enum.

- **Site.** A function that names a removed variant. The row is at the line of its first use of a removed variant. A function that names only surviving variants is not a site.
- **Placement.** Never "in your diff". The name is still in the working tree, so the use is stale, also when the diff touched other lines of the function. A function that dropped every use of a removed variant is not a site.

When one change adds and removes variants, the functions that name a removed variant come first. Each other function is judged as for an added variant. A function is never listed twice.

A removed or renamed symbol is no longer in the index, so its report is judged as if the symbol were still defined: `reference_report_for_edit` takes its name, kind, file, and owner (`parent_symbol_id`, the owner in the current index) from the symbol, and the judge reads those columns instead of joining its row. A site is `proven` when its evidence names the old owner type (a receiver typed as the owner, `Owner.name(…)`), its file, or an import that reaches it, and `likely` otherwise. A JavaScript or TypeScript import that names the old file, directly or through `export *` and `export … from` rows, reaches a removed top-level symbol, also when the edit deleted the file. The old file takes its own place in the resolver's order, so `./util` from a JavaScript file names a deleted `util.js` before the `util.ts` beside it, and `./util` from a TypeScript file names `util.ts`. A removed JavaScript method keeps the object it was assigned on (`req` for `req.param = function`), read from its signature, so a call through that receiver stays `likely` as in the callers report. A site that another definition of the name takes is `excluded` (a receiver typed as another class that keeps the name, an import of the name from another file), and a site that another definition can reach and the index cannot choose is `unresolved`. A removed top-level C# type whose file no longer declares its namespace takes a namespace of that name in the index as its owner, so a bare use whose file neither holds nor opens the namespace is `excluded`, as in the callers report. A removed C# symbol keeps the `#if` condition that its base scan records (`preprocessorIf`), so a use of a fallback type kept from `#if !X` is `unresolved` when its file opens a namespace under `#if X`, as in the callers report. A Go method has no owner in the index, so a call through a receiver is `likely`, or `unresolved` while another type keeps a method of that name.

A site that the diff did not reach goes under its label. `proven` and `likely` sites are listed. `unresolved` sites are counted and not listed.

## Limits

- At most 30 changed symbols are checked, those with the most uses of their name first. `Checked N of M changed symbols, those with the most uses of their name first.` states the cut.
- The check starts no new step after 20 s (`Stopped at the 20 s time budget …`). It reads the clock between its steps (the file list, the refresh of each file, the shape comparison, and each changed symbol), not inside one. A scan of the base, a refresh, or the sites of one symbol that is already running can pass 20 s.
- Changes sort by path, then line, then name.
- `diff_hash` is a hash of the base commit, the list of changed files, and the content of each. It stays the same between two checks of one working tree. The hash needs no scan of the base: the Stop hooks compute it first and skip the diff they checked last and the diff they blocked on last.

## Session file

`code-kb hook SessionStart` prints its routing text first. Then it reads the hook JSON on stdin, when stdin is not a terminal and sends within 2 s, and writes `<root>/.code-kb/sessions/{session_id}.json`:

```
{"base":"fb5628e8091fdfcceb4bf0fc8ce7a31d6c2c94aa"}
```

The Stop hooks add three fields to the file:

```
{"base":"fb5628e8…","checked_diff":"3f1c…","blocked_diff":"3f1c…","blocked_sites":[{"path":"src/lib.rs","line":8,"text":"9a0c…","count":1}]}
```

- `session_id` holds only letters, digits, `_`, and `-` (at most 128 characters). The root is the payload's `cwd`, else the process folder, resolved like any `project_root`.
- The root needs a git repository with a commit. It needs no index. In a new clone or worktree with no index yet, the hook creates only `.code-kb/`, its `.gitignore` (`*`, written by the same helper as the index), and `sessions/{session_id}.json`. It writes no `artifact.db`; the server or the CLI builds the index on its first call. Outside a git repository, or before the first commit, no file is written.
- A file that has a base keeps it, so a resume, a `clear`, or a compaction does not move it. A file that has none (a Stop hook made it before `SessionStart` ran) gets the base from the next `SessionStart`, and it keeps its other fields. `SessionStart` takes the session lock for this, waits up to 2 s, and writes nothing when it cannot get the lock.
- The same call removes session files older than 7 days.
- `checked_diff` is the `diff_hash` of the last check that a Stop or SubagentStop hook ran in this session, blocked or not. `blocked_diff` is the `diff_hash` of the last diff a hook blocked on. A hook that finds no file creates one with these fields and no `base`; see above for how it gets one.
- `blocked_sites` holds the sites that the last block listed (at most 15): the path, the line, a hash of the trimmed line text (never the text), and how many lines of the file had that text. The next Stop hook of the session reads it and removes it (see "Telemetry" below).
- `{session_id}.lock` sits next to the file while a hook of the session runs (below). It is no session file, and `check_edit` does not read it.
- `check_edit` and `code-kb check` have no session id. They use the base of the session file written last within the past day. Two sessions on one root can therefore see each other's base; pass `base` to pick one.

## Stop and SubagentStop

`hooks/claude-codex-hooks.json` runs `code-kb hook Stop` and `code-kb hook SubagentStop` through the launcher, timeout 60, with no matcher.

The hook reads `session_id`, `cwd`, and `stop_hook_active` from stdin and works in this order:

1. It computes the `diff_hash` of the working tree against the session base (else `HEAD`). This needs git and a read of the changed files, and no scan of the base.
2. It takes the session lock, reads the session file, and ends with no output when the hash equals `checked_diff` or `blocked_diff`. No scan and no report run.
3. Otherwise it runs the whole check, writes `checked_diff` (and `blocked_diff` when it blocks), and prints, with exit 0:

```
{"decision":"block","reason":"Before you finish: 1 sites still use the old shape of `area`.\nsrc/lib.rs:8  area: parameters 2 → 3\nEdit them or say why they need no change."}
```

- It blocks only when a `proven` site is outside the diff. A `likely` site, a `signature changed` row, and a parameter change of a JavaScript function (`.js`, `.jsx`, `.mjs`, `.cjs`) never block: on clean commits of real projects they blocked stops that needed no edit. A signature change is often a type change that every call still fits (a nullable `?`, a type alias, a return type), and JavaScript ignores the argument count. `check_edit` still lists them. The first line counts the blocking sites (it keeps the word `sites` for one) and names each changed symbol once. The site lines follow, by path and line, at most 15, as `{path}:{line}  {display}: {change}`.
- It skips two diffs: the one it checked last (`checked_diff`) and the one it blocked on last (`blocked_diff`). So a diff that blocked stays silent, and the agent can answer with its reason. It stays silent when it returns after clean diffs, until a different diff blocks. The file keeps no longer list: a block on D1, then a block on D2, then a return to D1 blocks again.
- A skipped diff runs no check. A turn that changes no file, and a `SubagentStop` after a `Stop` on the same diff, cost one `git diff`, a hash of the changed files, and a read of the session file. On a zod copy with 17 changed files the first check took 5.35 s and each repeat 0.07 to 0.08 s.
- **The lock.** Steps 2 and 3 run under `{session_id}.lock`. The hook creates it exclusively, removes it when it ends, and waits for it up to 2 s. A hook that cannot get it prints nothing. So parallel hooks of one session (a `Stop` and several `SubagentStop`s) block once between them. A lock file older than 60 s, the hook timeout, belongs to a hook that died, and the next hook takes it over. Hooks of other sessions on the same root do not wait for it.
- A site that the check does not list never blocks: an `unresolved` site, which the check only counts; for an added variant, every function that is not a site (see "Variants: the new arm"); for an added field, a construction that still compiles (see "Fields: which constructions break").
- It prints nothing, and exits 0, when `stop_hook_active` is true, when no site is outside the diff, when no symbol changed shape, outside a git repository, with no index, with a missing or bad `session_id`, when it cannot get the lock in 2 s, on stdin that is a terminal or stays silent for 2 s, and on any error or panic.
- It also prints nothing when another `julie-extract` version or extraction level wrote the index. It uses the check of the index version guard (`index_written_by_other_extractor` in `sync.rs`) before it reads or writes the index, and it starts no scan or rebuild: the first write after an upgrade would rescan the whole index in place and could pass the 60 s timeout. The index file stays as it is. The next server start or `code-kb` command rebuilds it.
- Claude Code and Codex both run these two events and read the same `decision` and `reason` output. The hook was not run inside a live harness in this branch.

### Telemetry

The Stop hooks record two adoption measures in the `hook_telemetry` table of `~/.code-kb/telemetry.db`, under the same switches as the tool telemetry (`CODE_KB_NO_TELEMETRY`, `CODE_KB_DISABLE_TELEMETRY`, `CODE_KB_TELEMETRY_DIR`). No row holds source text.

- Each check that runs adds a row: the time, the root, the event (`Stop` or `SubagentStop`), the `diff_hash`, the number of blocking sites, and whether it blocked. A skipped diff adds no row.
- The first Stop hook after a block, with `stop_hook_active` true or false, sets `followed` on that block's row: 1 when an edit changed a site that the block listed, else 0. A site counts as changed when its file now has fewer lines with its text, so an edit above the site that moves it does not count, and a deleted file does. Then it removes `blocked_sites`. A block that no later Stop hook follows keeps `followed` empty.
- The MCP server records `clientInfo.name` of the `initialize` request in the `client` column of `tool_telemetry`.
- On a zod copy, a repeat Stop took 0.081 s against 0.080 s in 2.7.0 (median of 20).

## Known limits

Sites the check does not list:

- Calls inside macro arguments and functions passed as values are not sites. The answer says so.
- A removed field lists only the constructions that name it. Reads of the field (`p.z`) are not listed.
- A call through an alias of a removed or renamed symbol is not listed: Python `from m import area as ar`, TypeScript `import { area as ar }`, and Rust `use m::area as ar` followed by `ar(1)`. The aliased import line is listed in Rust (``imports `area```) and not in Python or TypeScript. TypeScript `export { area } from` and `export { area as ar } from` rows are listed.
- A Rust function imported with `use` has `likely` callers, not `proven`, so they never block a stop. A call through its module path (`crate::geometry::area(…)`) is `proven`.
- A line that holds two calls of one name is one site. When one of them passes a count the new range rejects, the whole line is listed under "Not in your diff".
- A Java record call whose count the source line does not give is a site, even when a constructor with another count takes it.
- A removed overload is judged only when the index gives the owner of the method. A Go method falls back to every site. Remaining overloads with no range count every site as unresolved. A TypeScript overload signature is no symbol in the index, so removing one is not seen.
- A Java argument count read from the source splits at commas outside round, square, and curly brackets, so a generic type argument with a comma (`new Foo(new HashMap<String, Integer>())`) adds a count.
- A Rust `Self { … }` literal in an associated `const`, and a `for Point { x, y } in v` pattern, are not constructions the check reads: they are neither listed nor counted.
- A Go struct-keyed map literal (`map[Point]int{{1, 2}: 3}`) is neither listed nor counted. Its keys are not read as literals of the struct.
- A call of a member of a removed class or struct is not listed when it names the member, not the class: a Java call through a static import (`checkArgument(…)` after `import static p.GsonPreconditions.checkArgument;`) or a call through a receiver (`cache.GetOrAdd(…)`). The class row lists the uses of the class name and the import lines of the class and its members.
- A C# call through an abstract or virtual method of a base type (`filter.ExecuteFilter(…)` on a `PathFilter`) is `excluded` for an override, which it reaches at run time.
- A C# call binds the overload that its argument types select, and julie gives the type of an argument only for the forms it can read (a literal, `new T(…)`, a cast, `as`, `typeof`, `nameof`, `this`, and a typed local, parameter, or field). A call with an argument of another form stays a site, and so does a call whose types fit another overload only through a conversion (`Equals(jvalue)` when the other overload takes `JToken`).
- C# code under `#if` keeps the first branch of each group. A use of a fallback type kept from `#if !X` (`ConcurrentDictionary` under `#if !HAVE_CONCURRENT_DICTIONARY`) is `unresolved` only when its file opens a namespace under `#if X`; a file that opens the library namespace with no condition keeps the use `likely`.
- julie records a JavaScript property that is assigned a value other than a function (`res.locals = …`) only on a plain object name, once per file. A read of a property that a member chain (`res.locals.user = …`) or a prototype assigns is still a `likely` site of a removed function with the same name.

Shapes the check does not see:

- A TypeScript `interface` and a `type` alias are not field owners, so a field added to or removed from one is not detected. Object literals of that type stop compiling and the check is silent.
- A Rust tuple struct (`struct P(i32, i32)`) has no field rows in the index, and its signature stops at the name, so a change of its element count is no shape change.
- A field added to or removed from the payload of an enum variant (`Circle(f64)` to `Circle(f64, f64)`, or a field of a struct variant) is no shape change. The enum gains no variant.
- A new method on a trait or interface (Rust, Java, TypeScript) is no shape change, because an added symbol never counts. The implementors that now lack the method are not listed. A change to the parameters of an existing method does list the overriding members.
- A TypeScript type parameter or bound (`<T extends string>` to `<T extends number>`, or `<T>` added) is no shape change: the TypeScript signature in the index leaves both out. A bound change is seen in Rust, Java, C#, and Kotlin.
- A parameter rename is listed (with the call sites that name the old parameter) only in Python, Kotlin, C#, and Swift. In any other language with named arguments (Ruby keywords, Scala, PHP, Dart) it is no shape change, and a call that names the old parameter is not listed. A Dart list of named parameters (`{required int w}`) reads as `signature changed` instead.
- A change of a Python `@staticmethod` or `@classmethod` decorator is not seen: the normal form drops decorators. A toggle of `static` is seen in the languages above, where it is a keyword (in C++ only for a member of a class or struct).
- A removed class attribute, `self.` attribute, or `@property` of a class that is not a field owner here (a plain Python class: `Flask.session_interface`, `AppContext._after_request_functions`, `Flask.env`) is no shape change, and its reads are not listed.
- A Python class counts fields only when it is a `@dataclass`. Other field forms are not read in every language. The dataclass reader knows `ClassVar` and `field(default=, default_factory=, init=False)`. It does not know `KW_ONLY`, `InitVar`, or a field that a base class declares.
- A C# record, a Kotlin data class, a Scala case class, and a Swift struct build every field by position or name, but only Rust, Go, a Python dataclass, and a Java record list constructions.
- A Rust lifetime rename (`<'a>` to `<'b>`) is a signature change. Only the forms in "Signatures: the normal form" are dropped.

Variants:

- For a removed variant, every function that still names it is a site, with or without a `match`. A spelling the `Variant uses` finder does not read is not found (`docs/reach-sections.md`).
- A new variant is judged only in the constructs above. A Kotlin arm head is the text since the last line break, so a condition list that continues on the next line is read from that line. A construct over a value that is not the enum counts when it names a variant of the same name written bare or after `Self`.
- A `match` over a value that wraps the enum is counted as unresolved and never listed, because one arm names no variant. A `match` over `Option<Kind>` with `Some(Kind::A)`, `Some(Kind::B)`, and `None` is one. A guarded wildcard makes its construct unresolved, though it is no fallback.
- A function that builds a variant, compares it, looks it up in a dictionary, or walks it in an `if` chain is counted as unresolved. The check cannot say whether it needs a change.
- A removed unit variant that a function names only in a `match` arm spelled with braces (`Kind::Circle { .. } => …`) is not found by the `Variant uses` finder, so that function is neither listed nor counted.

Git and sessions:

- A file moved with a plain `mv` and not added to git is a deleted file and an untracked file, so each function in it reports as removed. Run `git add` on the new path (or use `git mv`) and git pairs the rename.
- Against `HEAD`, a change that was committed is no longer in the diff. The session base keeps it.
- A `SubagentStop` block stores the diff hash in the session file, as a `Stop` block does, and both hooks skip the diff they checked last or blocked on last. A later `Stop` on the same diff is silent, so the stop of the main agent does not repeat the reason.
- `check_edit` and `code-kb check` have no session id. They use the base of the session file written last within the past day, so two sessions on one root can see each other's base. Pass `base` to pick one.
- After a `julie-extract` upgrade, the hooks check nothing until the server or a `code-kb` command rebuilds the index.
- The hooks block only on `proven` sites of a removal, a rename, a parameter count, a parameter name, a field, or a variant. A call that a type change breaks, a JavaScript call that a new parameter count breaks, and a `likely` site reach the agent only through `check_edit`.
