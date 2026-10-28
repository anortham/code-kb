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
  `**kwargs` cannot prove that a required keyword is missing. The written count
  of a Ruby call includes its keyword arguments and a block argument (`&blk`,
  `&:name`), so a range counts keyword slots, and a `&block` parameter is an
  optional slot: a call with a `do … end` block or no block does not fill it.
  `*rest` and `**opts` remove the maximum.
- An Elixir pipe (`x |> f(a)`) passes its left value as the first argument,
  and the count between the parentheses leaves it out. So an Elixir call with N
  written arguments can pass N or N + 1. A call fits the target only when both
  counts fit, it is a stale call only when neither fits, and otherwise the
  count proves nothing. A capture with placeholders (`&f(&1, 2)`) counts its
  written arguments; a capture by arity (`&f/2`) has no count.
- R can defer evaluating a missing argument, so count alone imposes no minimum.
- An empty C declaration such as `int f();` supplies no count information without
  a known language standard. `int f(void)` and C++ `int f()` specify zero arguments.
- F# curried signatures without a parenthesized parameter list have no strict
  range. A valid partial application must not become a proven arity failure.
- Erlang overload groups with gaps between accepted arities have no single range.

A `likely` or `unresolved` call is excluded only when its known count fits no
candidate definition in the same supported language. A `proven` binding keeps its
label. A Go one-argument call is not excluded for too few arguments because
`f(g())` can pass multiple return values. JavaScript permits missing and extra
arguments, so it has no strict count rule.

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
