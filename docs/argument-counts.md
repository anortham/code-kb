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
  `Long`, `Short`, or `Byte`, so that pair stays open. Any other type pair keeps
  the overload: a subclass, an interface, a generic type parameter, `Object`,
  `Any`, a Scala implicit conversion, and every C#, C++, Swift, and VB.NET
  conversion (a C++ `short` that promotes to `int`, a VB.NET `Integer` that
  widens to `Long`).

An exact match decides alone. When every argument type equals the parameter
type it fills, and every parameter the call leaves out has a default value, the
overload is exact. The language picks the one exact overload over every other
overload the call fits, so the call binds only it: every one of these
languages ranks an identity conversion above every other. An exact overload
that leaves out no default wins over one that does. Kotlin also picks it, but
julie writes `Int` for an `Int?` value. So in Kotlin the exact overload decides
only when no other fitting overload takes a null where it does not: a parameter
written with `?`, a type parameter, or a parameter with no type fact.

A type is compared by its written name without type arguments, whitespace, or
a Kotlin trailing `?`, and without the language package (`java.lang.`,
`kotlin.`, `kotlin.reflect.`, `scala.`, `Swift.`). A Swift `Int?` keeps its
`?`. A C# type keeps its namespace, and an explicit framework name reads as its
keyword (`System.String` is `string`). A VB.NET type ignores case, and a
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
