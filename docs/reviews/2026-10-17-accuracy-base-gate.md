# Accuracy base: reach gate, latency, and memory

Date: 2026-10-17. Branch `feat/accuracy-base`. Design: `docs/plans/2026-10-17-accuracy-base-design.md`.
Plan: `docs/plans/2026-10-17-accuracy-base.md`.

The labeled callers report passes the reach gate. The v2.6.0 baseline fails it. Overall recall goes
from 0.529 to 0.803, and proven precision is 1.000 in every language. The cost is a
larger index, slower scans, slower `find_references` calls, and more memory in `serve`.

## Gate result

Reports: candidate `target/reach-gate/report.md`, baseline
`target/reach-gate/baseline-v2.6.0-final/report.md`. Baseline binary: code-kb 2.6.0. The baseline
labels nothing, so the gate scores each of its rows as proven, and its accounted share equals its
recall.

Proven precision / recall / accounted share, dev and holdout together:

| Language | v2.6.0 | Candidate |
|---|---|---|
| csharp | 0.993 / 0.698 / 0.698 | 1.000 / 0.899 / 1.000 |
| go | 0.697 / 0.074 / 0.074 | 1.000 / 0.871 / 1.000 |
| java | 0.789 / 0.947 / 0.947 | 1.000 / 0.950 / 1.000 |
| javascript | 0.171 / 0.032 / 0.032 | 1.000 / 0.423 / 1.000 |
| python | 0.798 / 0.895 / 0.895 | 1.000 / 0.900 / 0.996 |
| rust | 0.971 / 0.902 / 0.902 | 1.000 / 1.000 / 1.000 |
| typescript | 0.303 / 0.358 / 0.358 | 1.000 / 0.430 / 0.992 |

Candidate by split (recall, dev / holdout): csharp 0.960 / 0.824, go 0.806 / 1.000, java 0.984 /
0.900, javascript 0.441 / 0.407, python 0.970 / 0.800, rust 1.000 / 1.000, typescript 0.555 /
0.357. Proven precision is 1.000 on both splits in every language.

The pass marks are proven precision, excluded precision, and accounted share, each >= 0.98 per
language. Excluded precision is new (owner, 2026-10-17): excluded sites that cover no truth line,
divided by all excluded sites. The candidate scores at least 0.998 in every language. The recall
target of 0.90 is reported and does not decide the result.

## Holdout before and after the fix loop

The first candidate (probe 1) had wrong proven sites on dev and holdout symbols. The fix loop
changed rules only for causes found on dev symbols. Holdout precision of probe 1 against the final
run:

| Language | Probe 1 | Final |
|---|---|---|
| csharp | 1.000 | 1.000 |
| go | 1.000 | 1.000 |
| java | 0.978 | 1.000 |
| javascript | 1.000 | 1.000 |
| python | 0.963 | 1.000 |
| rust | 1.000 | 1.000 |
| typescript | 0.984 | 1.000 |

The fixes moved holdout sites from proven to unresolved, never to excluded:

- Python: proven-true 77 of 80 became 72 of 72; unresolved-true went from 14 to 19.
- TypeScript: proven-true 183 of 186 became 123 of 123; unresolved-true went from 176 to 237.

## Causes the fix loop removed (dev symbols)

- A package import that reaches other top-level definitions of the name proved the site. Now the
  site is unresolved, with "the import matches N definitions".
- A module receiver proved a class member. Now it proves only a top-level definition.
- A Python absolute import proved a file in a folder of the same name deeper in the tree
  (`import json` and `src/flask/json/`). Now the import must name the top package or a folder
  above it.
- A JavaScript read that only scope tied to the target was proven. julie records no callback
  parameters, so the read can be a parameter. Now it is likely.
- A bare name proved a JavaScript method assigned to a property (`req.accepts = function`). Now
  the site is excluded.
- A decorator line counted as a site, because the definition span began below the decorators.
- julie links a call inside a method to that method by name alone. That link proved the site.
- A subclass write to an inherited attribute (`self.debug = …`) counted as a definition and hid
  the base class definition.

## Wrong exclusions

The gate scores only proven precision and accounted share, so a true site labeled `excluded`
still passes. Such a label tells an agent that the site calls another definition. The tier table
of the first passing run showed 67 true C# sites, 34 TypeScript sites, and 4 Java sites as
excluded. Four causes on dev symbols, each with a regression test:

- A C# extension method (`FromCanceled(this CancellationToken token)`) lives in a static class,
  so a receiver of type `CancellationToken` excluded every call (67 sites). The type of the `this`
  parameter now counts as an owner.
- julie stores `test("strictObject", …)` as a function named `strictObject`. Every call of the
  real function then resolved to the test (29 sites). A test label is no longer a definition.
- `z.coerce.boolean()` was claimed by a v3 object member `coerce.boolean` because the report
  ignored the qualifier `z`, an import of v4. The owner name now needs the qualifier's import to
  reach the owner's file.
- julie stores no receiver for a call on a call result (`getAsSingleElement().getAsInt()`), so
  the call looked bare and the class chain claimed it. A call whose name follows `.` after a call
  or index result is no longer bare. The same cause gave the 3 wrong proven gson `toString` sites
  on holdout, and those are now unresolved.

Now 4 true sites stay excluded: Java 2 and TypeScript 1 on holdout, TypeScript 1 on dev. No proven
site that covers a truth line was lost in any of the 7 projects (checked against the build before
these fixes). The gate now fails a language whose excluded precision is under 0.98, and
`reach_gate.py disagreements` lists each excluded site that covers a truth line.

## Known gaps (not tuned)

- Holdout exclusions: gson `getAsInt` in a lambda (`element -> … element.getAsInt()`) and on a
  local `json` that the index types as `String`; zod `ctx.registry.add`, typed by its head `ctx`.
- Dev exclusion: zod `{ check: "overwrite", tx }`. The shorthand property reads the parameter
  `tx` and also names the property `tx`; the report sees only the parameter.
- TypeScript recall is 0.40. The report does not follow re-exports, so zod `z.strictObject` and
  `custom` stay unresolved.
- A Rust bare call through a `use` list is only likely.
- `refs --symbol-id` unions the overloads of the selected name.

## Time limit

Hermes `get` has 42,362 rows after pairing. The first candidate took 56 s, mostly in one judge
query per row group. The report now checks sites nearest the target first, in chunks of 2,000
rows, and stops after 3 s. The target's own checks stop at half of that. A site left unchecked is
likely when the name has one definition and unresolved otherwise, with the basis "not checked
within the time limit". The answer counts those sites. Hermes `get` now labels 1,555 of 37,615
sites (2 proven, 1,551 excluded) and lists the other 36,060 as not checked.

## Latency

`find_references` p50 over 7 interleaved rounds (`~/.cache/code-kb-accuracy/bench/refs_latency.json`),
v2.6.0 against the candidate:

| Project | Symbol | v2.6.0 | Candidate |
|---|---|---|---|
| aspnetcore | `ThrowIfNull` | 8.2 ms | 169.7 ms |
| aspnetcore | `AddHealthChecks` | 15.4 ms | 120.3 ms |
| aspnetcore | `AddAction` | 13.2 ms | 42.2 ms |
| hermes-agent | `get` | 3,698 ms | 3,254 ms |
| hermes-agent | `_build_api_kwargs` | 19.0 ms | 178.0 ms |
| hermes-agent | `ActionRow` | 12.8 ms | 100.1 ms |

The candidate reads every row of the name, runs the judge query on each chunk, and scans the text
of the workspace. v2.6.0 stopped at the row limit.

## Index size and scan time

The candidate indexes at julie's `full` level (call and variable-reference identifiers):

| Project | Scan v2.6.0 | Scan candidate | Peak RSS v2.6.0 | Peak RSS candidate | Index v2.6.0 | Index candidate |
|---|---|---|---|---|---|---|
| aspnetcore | 40.71 s | 57.58 s | 2.81 GB | 4.75 GB | 2.0 GB | 3.6 GB |
| hermes-agent | 55.57 s | 86.77 s | 2.60 GB | 5.38 GB | 1.8 GB | 4.0 GB |

## Memory in `serve`

RSS after the benchmark calls, before the fix: aspnetcore 37.8 MB (v2.6.0) and 88.2 MB
(candidate); hermes-agent 32.2 MB and 113.7 MB. All start at 11–14 MB.

A heap probe (`mallinfo2` after each call) shows that this is not live data. After a `get` call the
live heap is 12–17 MB and SQLite holds 4 MB. glibc keeps 69–88 MB of freed heap. The report peaks
at about 40 MB of live heap for `get`: 18 MB of rows, then the merged sites and the text scan. A
freed 36 MB row buffer also raises glibc's trim threshold to 64 MB, so glibc stops returning free
pages.

Fix (owner approved the one unsafe call, 2026-10-17): on Linux glibc, `release_freed_memory` in
`mcp/server.rs` calls `malloc_trim(0)` after each tool call. The workspace lint is now
`unsafe_code = "deny"`, and this function is the only unsafe code. Other targets get an empty
function. With the fix, RSS after the same benchmark is 58.5 MB on aspnetcore (v2.6.0: 38.3 MB)
and 51.4 MB on hermes-agent (v2.6.0: 31.6 MB). Latency did not change: `get` p50 3,242 ms,
`ThrowIfNull` 174 ms. The remaining 20 MB is heap that live allocations split, which
`malloc_trim` cannot return.
