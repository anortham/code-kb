# Argument counts

The callers report (`find_references` for callers, CLI `refs`) uses argument counts
as exclusion evidence. Edit checks use the same accepted ranges to find stale
calls. Neither an accepted count nor a missing caller proves that an edit is safe.

Julie records `arg_count` and, for some languages and forms, `arg_spread` on call
identifiers. A site shows `; N args` when it has a known count. A relationship or
pending call takes the paired identifier's count, or the one call identifier on
its line. Edit checks can also read a bounded source slice when the count is
absent. Unbalanced calls, unsupported argument forms, and spreads stay unproven.

## Supported range readers

Readers cover Python, Java, C#, Go, Rust, Kotlin, C, C++, TypeScript/TSX, Swift,
PHP, Ruby, Scala, Dart, F#, VB.NET, R, Zig, GDScript, Elixir, and Erlang. A reader
returns a range only when the indexed parameter rows agree with the written
signature and the callable form is supported. The real-extractor tests in
`edit_check_arguments_test.rs` exercise these boundaries; see
[language support](language-support.md) for observed coverage and gaps.

- Defaults and optional parameters lower the minimum. Variadic parameters remove
  the maximum. TypeScript `void` parameters and unions containing `void` are optional.
- PHP user-defined functions accept extra positional arguments, so their maximum
  is open even without a variadic formal. Required parameters still impose a minimum.
- Dart's positional range excludes named arguments. Required named parameters
  need their own binding check. Optional named additions impose no new requirement.
- Ruby keyword arguments need label checks as well as argument counts. Unknown
  `**kwargs` cannot prove that a required keyword is missing. The count of a
  Ruby call includes its keyword arguments, so a range counts keyword slots.
  Every Ruby method takes a block, so the count of a call leaves out a block
  argument (`&blk`, `&:name`), and a `&block` parameter holds no slot in the
  range. `*rest` and `**opts` remove the maximum.
- An Elixir pipe (`x |> f(a)`, `x |> Mod.f(a)`, or a pipe at the end of the
  code line above the call) passes its left value as the first argument, so the
  count of a piped call is one more than its written arguments. Trailing
  `key: value` pairs form one keyword-list argument (`f(1, a: 1, b: 2)` passes
  2). A pair with a quoted key still counts as its own argument. A capture with
  placeholders (`&f(&1, 2)`) counts its written arguments; a capture by arity
  (`&f/2`) has no count.
- R can defer evaluating a missing argument, so count alone imposes no minimum.
- An empty C declaration such as `int f();` supplies no count information without
  a known language standard. `int f(void)` and C++ `int f()` specify zero arguments.
- F# curried signatures without a parenthesized parameter list have no strict
  range. A valid partial application must not become a proven arity failure.
- Erlang overload groups with gaps between accepted arities have no single range.

A `likely` or `unresolved` call is excluded only when its known count fits no
candidate definition in the same supported language. A `proven` binding keeps its
label under this rule; the overload rule below can exclude it. A Go one-argument call is not excluded for too few arguments because
`f(g())` can pass multiple return values. JavaScript permits missing and extra
arguments, so it has no strict count rule.

## Overloads

A target with overloads (the symbols with its name, owner, kind, and file) is
judged one overload at a time. `reference_report/overloads.rs` decides which
overloads a call can bind from the facts on its call identifier. It serves the
callers report, the `blast_radius` walk, and `check_edit`. It covers Java,
Kotlin, Scala, C#, C++, Swift, and VB.NET.

The facts and what each one rules out:

- `arg_count`: an overload whose own range does not take the count. The range
  of a method is joined with the ranges of the methods of its name in the base
  types, because a call through a base reaches the override with the base's
  defaults. An overload with no range takes every count.
- `arg_names` (Kotlin, Scala): an overload that has no parameter with a given
  name. A named argument fills the parameter of its name, and a positional one
  the parameter at its position. A method whose base types have a method of
  the name skips this check, because a call through a base uses the base's
  parameter names.
- `arg_labels` (Swift): an overload whose external labels do not match the
  labels in order. A parameter with a default value or a variadic one can be
  left out. An argument with no label and no type can be a trailing closure,
  which leaves out its label, so it can also fill a labeled parameter that can
  take a closure: a function type, or a name that is not a built-in value type
  (`Int`, `String`, `Double`, `Bool`, an array, a tuple), since a type alias can
  name a function type. So `run(make())` never binds `run(a: Int)`, and
  `run(1) { 2 }` binds `run(_: Int, with: () -> Int)`.
- `arg_types` (one entry per argument, or null): a type rules an overload out
  only where the language rule is certain. Java compares primitives, boxed
  primitives, and `String` by the method-invocation conversions (identity,
  widening primitive, boxing, unboxing then widening): `long` does not fill
  `int`, and `int` does not fill `String` or `Long`. Kotlin compares its final
  built-in types (`Int`, `Long`, `String`, `Boolean`, the unsigned types, and
  the others). julie writes `Int` for an integer literal, which also fills a
  `Long`, `Short`, or `Byte`, so that pair stays open. Scala compares its value
  types and `String` by numeric widening (`Int` fills `Long` and `Double`, and an
  integer literal fills `Byte`, `Short`, and `Char`): `String` does not fill
  `Int`. A conversion that the code defines between these types (an `implicit
  def`, a `given Conversion`) is not seen. Any other type pair keeps the
  overload: a subclass, an interface, a generic type parameter, `Object`, `Any`,
  a Scala implicit conversion of another type, and every C#, C++, Swift, and
  VB.NET conversion (a C++ `short` that promotes to `int`, a VB.NET `Integer`
  that widens to `Long`).

An exact match decides alone. When every argument type names the parameter
type it fills, and every parameter the call leaves out has a default value, the
overload is exact. The language picks the one exact overload over every other
overload the call fits, so the call binds only it: every one of these
languages ranks an identity conversion above every other. An exact overload
that leaves out no default wins over one that does. An exact overload that
leaves out a default decides only when every other overload the call fits has
an argument whose known type differs from its parameter's, because an overload
with unknown types (a C# `in` parameter, a varargs parameter) can take the call
without a default and then wins.

Scala ranks the other way: it picks among the overloads that apply without a
default first (Scala 2.13 spec 6.26.3). So in Scala an exact overload that
leaves out a default decides only when the call fits no other overload. When
another overload certainly applies without a default (every parameter takes an
argument, and each argument has the parameter's type or fills an `Any`
parameter), the exact overload is excluded and the call binds one of the
others: `r(i)` with `val i: Int` binds `r(x: Any)` and never
`r(x: Int, extra: Int = 0)`. Otherwise the call keeps every overload it fits.

julie writes some nullable arguments without their marker: `Int` for a Kotlin
`Int?` value, `int` for a C# `int?` value (a local, a field, a property, or a
parameter; only a cast keeps `int?`), `Pt` for a C# `Pt?` value, and `Int` for
a Swift `Int!` value. So in these languages a call whose exact overload cannot
take the nullable value binds that exact overload or another fitting overload
that takes the nullable value. Every other overload stays excluded, because the
call binds it neither with the value nor with the nullable value:

- Kotlin: a parameter written with `?`, a type parameter, or a parameter with
  no type fact takes it; every other Kotlin parameter does not.
- C#: an exact parameter can hide a nullable value unless its type is a
  reference type: `string`, `object`, `dynamic`, an array, a name that the
  index defines only as a class, an interface, or a delegate, or a name that
  the index does not define and that names a .NET framework class or interface
  (`Uri`, `Type`, `Stream`, `List`, `IEnumerable`, a name that ends in
  `Exception` or `Attribute`, and the others in `CSHARP_FRAMEWORK_REFERENCES`).
  Every other parameter takes the nullable value (it boxes to `object` and to
  an interface, a generic type parameter takes it, and a user-defined
  conversion can take it) except a value keyword (`int`, `double`, and the
  others), `string`, an array, a framework class of that list, and the value
  type of the exact parameter itself (`int?` never converts to `int`, and an
  enum `Mode?` never to `Mode`).
- Swift: an exact parameter that is not optional can hide an `Int!` value. A
  parameter written with `?` or `!`, a type parameter, or a type that is not a
  built-in value type or an array takes it.

The source of an argument can show that it is no nullable value. Then the
exact overload decides alone. `reference_report/plain_values.rs` reads the
argument list of the call at its line and column and accepts:

- a literal: a number, a character, a string, `true`, or `false`;
- a C# `new T(…)` or `new T { … }` whose type has no `?` (not a target-typed
  `new()`);
- a Swift construction `T(…)` of a project struct or class that has no
  `init?` or `init!`, in its body or in an extension, when no other symbol has
  the name (a library initializer can be failable, as `Int("5")` is);
- a name whose nearest declaration in the caller, its enclosing symbols, or its
  type (a local, a parameter, a field, or a property) writes a type without `?`
  or `!` that is no type parameter of those symbols. C# and Kotlin read the
  type from julie's signature; Swift reads it from the declaration text,
  because julie's signature of a Swift property drops the `!` of `Int!`.

Every other argument (an expression, a call result, `var` or an unannotated
`let`, a member access) can still be a hidden nullable value.

So `Nul(local)` with `int? local` keeps `Nul(int?)` beside `Nul(int)`, and
`Lit(x)` with `int? x` binds `Lit(int)` or `Lit(int?)` but never `Lit(long)`
or `Lit(Uri)`. `Nul(5)`, `Nul(k)` with `int k = 3`, and `Sp(new Pt())` bind
only `Nul(int)` and `Sp(Pt)`; Swift `f(5)`, `f(v)` with `let v: Int`, and
`g(Pt(x: 1))` bind only `f(_: Int)` and `g(_: Pt)`, while `f(w)` with
`w: Int!` keeps `f(_: Any)`. `B(box)` with `Box? box` still binds only `B(Box)` over
`B(object)` when `Box` is a class, and `Ext(u)` with `Uri? u` binds only
`Ext(Uri)` over `Ext(Uri?, int extra = 0)`. The class test reads the simple
name, so a struct outside the index with the name of a project class
(`System.Drawing.Color` beside a project `class Color`) reads as a class. A C#
`dynamic` argument binds at run time, so no overload is exact for such a call.
A VB.NET `Object` argument does not make a call late-bound when an overload
takes `Object` there: overload resolution drops every overload that needs a
narrowing conversion, whether `Option Strict` is on or off, so `Run(o)` with
`o As Object` binds only `Run(Object)` beside `Run(String)`. The call binds
late only when every overload needs a narrowing conversion from `Object`
(`Take(Integer)` and `Take(String)`), and then it keeps every overload it fits.

A type is compared by its written name without type arguments, whitespace, or
a Kotlin trailing `?`, and without the language package (`java.lang.`,
`kotlin.`, `kotlin.reflect.`, `scala.`, `Swift.`). A Swift `Int?` keeps its
`?`. A Kotlin or Scala parameter type comes from julie's type fact, which drops
a qualifier and a Kotlin `?`; a qualified declared type (`n: b.Node`) keeps its
qualifier, and the basis shows a Kotlin nullable parameter with its `?`
(``bind only `f(Int)` or `f(Any?)` ``). A C# type keeps its namespace, and an explicit framework name reads as its
keyword (`System.String` is `string`). A C# reference type drops its `?`, which
only annotates it (`object?` is `object`); a C# value type keeps it (`int?` is
`Nullable<int>`). A VB.NET type ignores case, and a
framework name reads as its keyword (`System.Int32` is `Integer`). A C++ type
is read the way julie spells an argument type: without a reference, without a
top-level `const` or `volatile`, which a call never sees, and with the
pointee's `const` first (`const Point&` is `Point`, `char const* const` is
`const char*`). The parameter type of C++ and Swift comes from the written
parameter, since julie's type facts drop the `const` of `const char*` and the
`?` of a Swift optional. A Java varargs parameter, a C# `params`, `ref`,
`out`, `in`, or `this` parameter, and a C++ array or function-pointer parameter
have no type to compare. julie writes null for a C# named, `ref`, `out`, or
`in` argument and writes no types for a VB.NET call with a named argument
(`Run(a:=1)`).

Two types whose names differ without their qualifiers are different types.
Otherwise `reference_report/type_names.rs` compares the types the names name,
since a qualified name and a simple name can name one type (`App.Box` and
`Box`), one simple name can name two project types (`a.Node` and `b.Node`), and
one simple name can name a project type in one file and a library type in
another, or two library types (`java.util.Date` and `java.sql.Date`). The
argument type is read in the caller's file and the parameter type in the
overload's file, and they are the same type, different types, or unknown.

Java, Kotlin, Scala, C#, and VB.NET read every name by the scope rules of the
language. A keyword type (`int`, `string`, VB.NET `Integer`) is always the same
type. Rows with one dotted name (a partial type) are one project type. A name
names one of these:

- A project type. A qualified name names the one project type whose namespaces
  or package and enclosing types end with it. A simple name names a type nested
  in a type that encloses the code; else, when no project type of the name is
  nested in another type or a function (an inherited nested type could take the
  name), the type the language reaches:
  - Java, Kotlin, Scala: a type of the file, or the type that an import of the
    name writes (`import a.Node`), else the one type of the file's package,
    else the one type of a package that a star import names. Scala ranks a
    wildcard import above the package of another file in some versions and
    below it in others, so a Scala name that both reach with different types is
    unknown.
  - C# and VB.NET (VB.NET ignores case): the one type of the innermost
    namespace that holds the code and has such a type, else the one type of a
    namespace that a `using` or `Imports` line of the file, a `global using`,
    or the build project opens (the `<Using Include>` or `<Import Include>`
    items of the nearest `.csproj` or `.vbproj`, and the SDK's implicit usings
    when it sets `ImplicitUsings` and removes none).
- A library type by its full name: an explicit import of a type that the index
  does not have (`import java.sql.Date`); a Java name of `java.lang`
  (`Process`, `String`); a Java name that a star import of `java.util` or
  `java.io` reaches; a C# or VB.NET name that exactly one opened namespace of a
  known list holds, with the same arity (`Exception` in `System`,
  `JsonException` in `System.Text.Json` or `Newtonsoft.Json`, `List<T>` in
  `System.Collections.Generic`); and a Kotlin or Scala built-in name (`Int`,
  `String`, `Any`). The lists are in `reference_report/library_types.rs`. A
  second opened package or namespace that holds the name would make the name
  ambiguous, which does not compile, so the list need not be complete. Kotlin
  and Scala rank a star import above the default imports, so their built-in
  names rest on one assumption: no library that a star import opens defines a
  type with a built-in name.
- A library type known only by what the code opens: the star imports, or the
  `using` and `Imports` lines and enclosing namespaces, without the packages
  and namespaces the project declares. Two such names are one type in Java,
  C#, and VB.NET when one set holds the other (a name that two opened places
  hold does not compile), in Kotlin and Scala when the sets are equal, and in
  C# and VB.NET only inside one build project, whose project-wide usings both
  files see. A library type by its full name is that type when its package or
  namespace is in the set.
- A qualified name of no project type, known only by its text.
- Unknown: a `using` or `Imports` alias of the name or of a qualifier, a
  Kotlin import alias (`import a.Other as Node`), a Scala import with
  selectors, a static import of the name, a `using` inside a namespace block
  that opens a namespace with a project type of the name, an MSBuild `<Using>`
  item beside a project type of the name, and more than 100 types of the name.

A project type and a library type by its full name, two different project
types, and two different full names are different types. C++ and Swift keep
the text rule: equal text names one type when the index has at most one
project type of the name (in Swift two rows of one name are two types, since
the index names no module), and a qualified name names the one project type it
ends with; every other comparison is unknown.

An unknown pair is neither exact nor a known difference, so the call keeps
every overload it fits. So `R(b)` with `Box b` in namespace `App` binds
`R(App.Box)` over `R(Box b, int extra = 0)`, and `take(other)` with `b.Node
other` keeps `take(Object)` beside `take(a.Node)`, while a call that passes an
`a.Node` binds only `take(Node)`. `put(d)` with `java.util.Date d` keeps
`put(Object)` beside `put(java.sql.Date)`, `run(p)` with a `java.lang.Process`
keeps `run(Object)` beside a project `run(q.Process)`, and `Handle(ex)` with a
`Newtonsoft.Json.JsonException` keeps `Handle(Exception)` beside
`Handle(System.Text.Json.JsonException)`, while `Handle(error)` with an
`Exception` binds only `Handle(Exception)`. Known limits: a library type in a
package or namespace that the project also declares, and an inherited nested
type of a library base class, are not seen.

A call written on the line of its caller's definition (`string Parse(string
json) { return Parse(json, null); }`) gets these checks too. The call
identifier on that line is no site of its own, but it gives its argument facts
to the call row of its line.

A constructor's overloads also include the primary constructor of a Kotlin,
Scala, or C# class, and an overload with unknown parameters when the index
lists no symbol for one of the class's constructors: a Java record's canonical
constructor, a Java class with no declared constructor, a C# struct's default
constructor, and every C++ class (copy and move constructors). julie writes a
C++ construction (`Box b(1)`, `Box d{2}`, `new Box("s")`) and a VB.NET
`New Ov(1)` as a `call` of the class name with its argument facts, so these bind
a constructor too.

In the callers report a site of any label is `excluded` when its call binds
only other overloads than the target:
``arguments `(String)` bind only `run(String)` ``. The basis shows each
overload with its written parameter types, and in Swift with its labels
(``arguments `(b: Int)` bind only `run(b: Int)` ``). When another rule also
excludes the line (a C++ construction that the constructor's name ties to its
class), this basis wins, or
``1 arg fits only `run(int, int)` `` when julie writes a count but no types.
A call that fits no overload is not excluded by this rule; the count rule above
still applies to it. The rule is off in the report that `check_edit` reads, so
a call that still passes the old arguments keeps its label there.

In the `blast_radius` walk a step from a caller to an overload is dropped when
every call identifier of the name in the caller binds only other overloads. The
step stays when a call has no facts, or when the caller has more call rows of
the name than call identifiers.

## Receivers and inheritance

Go grouped parameters are ordered by their positions in the signature. Python,
Rust, C# extension, and Go method rules account for receivers where applicable.
A C# extension receiver has `this` after optional attributes and modifiers such
as `ref`, `in`, `scoped`, and `readonly`.

Supported ancestor widening unions the ranges of same-named base methods,
because calls through a base may use defaults that an override omits. The walk
uses indexed inheritance facts and import aliases, to depth 8 and 50 types,
within 1 second. An unknown ancestor range or a cut makes that inference unknown.
Rust traits and Go interfaces require compatible method counts.

## Missing or ambiguous contracts

No range is inferred for unsupported decorators, destructuring or unpaired
parameter rows, or callable forms whose constructors the index cannot enumerate.
Java records, Kotlin data classes, C# records, Swift memberwise initialization,
and other field-driven constructions have separate checks in
[edit checks](edit-check.md).

Primary constructor classes in F#, Kotlin, Scala, and C# use their written header
and indexed parameter evidence when Julie emits no separate constructor symbol.
Stored fields alone do not establish a primary constructor.

SQL remains an applicable gap. Julie's SQL function parameters use
`isParameter=true`, procedure parsing can produce error-recovery rows, and the
index has no database dialect. A generic strict range would incorrectly assume
one database's call and overload rules. CSS custom functions likewise lack
formal-parameter facts at the pinned extractor. Navigation and references do
not establish argument-check support for either language.
