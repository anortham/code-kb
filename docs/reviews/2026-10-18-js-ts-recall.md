# JS/TS recall: honest likely labels and the module resolver

Date: 2026-10-18. Branch `feat/js-ts-recall`. Design: `docs/plans/2026-10-17-accuracy-base-design.md`.
Phase 1 review: `docs/reviews/2026-10-17-accuracy-base-gate.md`.

The callers report now meets the recall target. Overall recall goes from 0.803 (main, 6de37ae) to
0.911. Proven precision stays 1.000 in every language. No proven site that covers a truth line was
lost in any of the 7 projects. The `likely` label is now right on most sites: on TypeScript it
was right on 28 of 558 sites, and it is right on 11 of 12.

## Gate result

Reports: candidate `target/reach-gate/report.md`, baseline
`~/.cache/code-kb-accuracy/reach-gate-6de37ae/report.md` (main at 6de37ae).

Recall, main against the candidate:

| Language | All | Dev | Holdout |
|---|---|---|---|
| go | 0.871 → 1.000 | 0.806 → 1.000 | 1.000 → 1.000 |
| javascript | 0.423 → 0.995 | 0.441 → 1.000 | 0.407 → 0.992 |
| python | 0.900 → 0.904 | 0.970 → 0.978 | 0.800 → 0.800 |
| typescript | 0.430 → 0.651 | 0.555 → 0.881 | 0.357 → 0.517 |

- C# (0.899), Java (0.950), and Rust (1.000) did not change.
- TypeScript proven sites: 227 → 375. Proven precision is 1.000 on both splits in every language.
- Excluded precision: TypeScript 0.998 → 0.999, TypeScript dev 0.994 → 0.998. No language fell.
- Accounted share did not change in any language.

## Label tiers

Cells are sites that cover a truth line / all sites, all splits, main against the candidate:

| Language | likely | unresolved | excluded |
|---|---|---|---|
| typescript | 28/558 → 11/12 | 331/1492 → 200/1330 | 2/1292 → 2/1852 |
| javascript | 57/110 → 183/238 | 127/203 → 1/75 | 0/128 → 0/128 |
| go | 580/606 → 688/695 | 108/116 → 0/27 | 0/1 → 0/1 |

On main, `likely` meant "only the name ties the site to the target", and it also counted the
target's own overloads as other definitions. On the dev symbols that label was wrong on 393 of
393 sites that said "nearest of N definitions". Now `likely` means that no other definition can
reach the site. A site that another definition can reach is `unresolved`.

## What changed

- **Rivals a site can reach.** Each site counts only the other definitions that can reach it, and
  never the target's own overloads. A Go, Java, or Rust call cannot reach a field, property,
  variable, or constant, except a Java record component, a Go func-typed field or variable, and a
  Rust static or variable of a function type. A JavaScript member access cannot reach a function
  that is not exported, not placed in an object literal, and not assigned on an object.
- **The JS/TS module resolver** (`reference_report/js_modules.rs`). It follows `export … from`,
  `export * from`, and namespace re-exports to a relative file or to a workspace package, through
  `package.json` `exports`, `module`, `main`, and `types`. `require()` and an ES import follow
  their own conditions. The nearest `tsconfig.json` or `jsconfig.json` decides first. A
  declaration file and the script with its stem are one module. So `z.strictObject()` through
  `import * as z from "zod/v4"` is proven.
- **Callback bindings** (`reference_report/js_bindings.rs`). julie records no symbol for a
  callback parameter, a `for … of` variable, or a `catch` parameter. The report reads them from
  the source. When one of them has the name, an import, same-file, scope, or qualified-path proof
  becomes `likely` and names the line. A typed arrow function (`(f: T): R => …`) counts when the
  `=>` comes within 200 bytes of the return type's `:`, across lines, generics, tuples, unions,
  quoted types, and comments. Against the TypeScript parser on 6,761 typed arrows in local
  projects, the scan misses 6, and julie records the parameters of all 6 itself.
- **Hidden receivers.** julie records no receiver for a call on a call result, an index result,
  or a string literal. A call after such a `.`, on a line that starts with `.`, or on the line
  below a line that ends with `.` is never a bare call.
- **Object receivers.** A JavaScript call whose receiver names the object that the target is
  assigned on in its file (`res.redirect()` for `res.redirect = function`) is `likely`.

## Review rounds

Six fix rounds ran after the first gate pass. Rounds 1 to 5 each had one fixer and one
adversarial reviewer. The reviewer built fixtures, ran mutation checks, and compared the old and
new builds on the gate projects, hermes-agent, and eight other real projects. In round 6 the lead
fixed the round 5 findings and a reviewer checked the fix. The lead then fixed that reviewer's
findings, and 40 mutation checks show that a test pins each new rule.

| Round | Commit | Items | Main causes |
|---|---|---|---|
| 1 | 0a57801 | 13 | rivals in every rule, declaration files last, the line-below receiver |
| 2 | 6452293 | 11 | tsconfig paths, `require` and `import` conditions, callback bindings |
| 3 | this review's commit | 10 | package entries, tsconfig `extends` lists, catch methods, ternary groups, cost |
| 4 | this review's commit | 6 | `require()` of a `module`-only package, `${configDir}`, Java fields |
| 5 | this review's commit | 5 | typed arrows after `return`, object return types, line ends without `;`, tests |
| 6 | this review's commit | 8 (1 kept as a gap) | return types on several lines, comments and quotes in return types, false bindings after `await` or `typeof`, tests |

Round 2 made a file of `x=a?(t):b;` lines take 8.2 s (2.5 s before). Round 3 bounded every
look-ahead in the binding scan, and the same file takes 2.4 s again. The rest of that cost is
older and outside the binding scan.

## Wrong exclusions

Four true sites stay excluded, the same as on main: Java 2 and TypeScript 1 on holdout, TypeScript
1 on dev. The phase 1 review lists their causes.

## Known gaps (not tuned)

- TypeScript holdout recall is 0.517. Most of the missing sites are `unresolved`.
- JavaScript holdout `likely` is right on 99 of 148 sites.
- Go methods are not linked to their types across files, the Go receiver typer is missing, and
  Java chain typing is missing.
- The binding scan does not read a typed arrow whose return type is a conditional type
  (`A extends B ? C : D`) or that has a comment before its `:`. A call to its parameter can then
  stay a wrong `proven`. No gate project has this shape.
- A ternary that ends a line with `:`, with an arrow function on the next line, makes the names
  in the group before the `:` count as bindings. A true `proven` call to such a name becomes
  `likely`. The same rule reads a return type that starts on the line after its `:`.
- A Java field still makes a call to a method of the same name `likely`, not `proven`, because the
  class-chain check sees the field.

## Latency

Live `serve` processes, main (6de37ae) against the candidate, each on its own copy of the
project, calls interleaved, 5 timed rounds after one warm-up call. Script:
`~/.cache/code-kb-accuracy/bench/refs_latency_js.py`.

| Project | Call | p50 ms, main → candidate |
|---|---|---|
| zod | `object` | 108 → 82 |
| zod | `string` | 353 → 193 |
| zod | `parse` | 794 → 751 |
| express | `get` | 91 → 82 |
| express | `use` | 137 → 196 |
| express | `render` | 40 → 42 |
| express | `send` | 31 → 31 |
| hermes-agent | `ActionRow` | 99 → 101 |
| hermes-agent | `_Lazy.get` | 3217 → 3203 |
| aspnetcore | `ThrowIfNull` | 175 → 178 |

- express `use` is 59 ms slower. Round 1 added the cost: its 652 sites keep the same labels, but
  each one now gets the object-receiver and rival checks, which name the basis.
- hermes-agent `_Lazy.get` stops at the 3 s check limit in both builds.
- With CLI `refs`, zod `z` takes 0.91 s against 0.87 s on 6452293 (about 5% more), from the
  round 4 to 6 checks.

## Memory in `serve`

Resident memory of the same processes, in MB:

| Project | At start, main / candidate | Largest after a call, main / candidate |
|---|---|---|
| zod | 14.6 / 14.3 | 22.9 / 22.8 |
| express | 13.2 / 13.2 | 18.0 / 18.1 |
| hermes-agent | 23.5 / 23.4 | 45.1 / 41.4 |
| aspnetcore | 31.7 / 32.9 | 42.1 / 43.9 |

No project uses more memory than the 25–40 MB and 50–60 MB bounds in invariant 2.
