# Argument counts

The callers report (`find_references` for callers, CLI `refs`) reads the argument count of each call site and uses it as exclusion evidence. This file holds every rule; `CLAUDE.md` and `AGENTS.md` hold the summary. Change this file with the code.

julie 3.8.0 writes `arg_count` on call identifiers (and `arg_spread` when an
argument is a spread). A site shows `; N args` after its basis, and `--json` adds `arg_count`;
a spread site, and a call with no recorded count, has none. A pending or relationship call takes
the count of its paired call identifier, or of the one call identifier on its line (`new Foo(a)`).
`reference_report/arity.rs` excludes a `likely` or `unresolved` Python, Java, C#, Go, Rust,
Kotlin, or TypeScript call (TSX counts as TypeScript; JavaScript never) in the language of the
target when the count fits no definition of the target: ``3 args; `render` takes 1-2``. A
`proven` site never changes. The range comes from the parameter rows and the signature, and the
target has none unless both list the same parameters. The rows are read in source order; Go
parameters that share one type (`a, b int`) have one position, so their order in the signature
decides. A Python, Rust, C#-extension, or Go method also accepts the receiver as one more
argument (`Base.method(self, x)`), and `takes` shows the bound count. A C# extension method is a
method whose first parameter has `this`, after any attribute groups and any `ref`, `in`,
`scoped`, or `readonly` modifiers, which may come before or after `this` (`[NotNull] this Lib l`,
`this ref int x`); the owner rule for extension methods reads the extended type the same way.
A `*`, `**`, `...`, `params`, or `vararg` parameter removes the upper bound. A TypeScript
parameter typed `void`, or a union with `void`, is optional.
A call through a base type reaches an override, and an override may drop a default value, take
an array for `params`, or take fewer parameters. So the range of a Python, Java, C#, Kotlin, or
TypeScript method is the union of its own range and the ranges of the methods of the same name
in every base type of its owner. The walk is the one of `Overrides of`: resolved and pending
`extends` and `implements` rows, to depth 8 and 50 types, within 1 s. A pending row also names its
base by an alias that an import of the subtype's file binds (`import { Base as B }`, `from m
import Base as B`, Kotlin `import a.Base as B`, C# `using B = N.Base1;`, where the last segment
of the target is the name); `Overrides` reads no alias. When a limit cuts the walk,
the time runs out, or one of those methods has no range, the method has no range. A base method
keeps its own range: a call that passes a count the base method does not accept runs only an
override, or fails. Rust and Go need no walk, since a trait or interface method has the same
count.
No range: a class, field, property, or variable; a Python decorator other than `staticmethod`,
`classmethod`, `abstractmethod`, `override`, `final`, or `overload`; a Python function that a
class body binds under its own name (`render = render`, `wrapped = staticmethod(wrapped)`); a
destructured, unnamed, or TypeScript arrow-property target; a TypeScript abstract or interface
method, because julie starts its body at the parameters; every Java and Kotlin constructor; a
C# constructor of a struct, a record, or a class with a primary constructor. A Go call with one
argument is never excluded for a target that takes more, since `f(g())` passes every value of
`g`.
