# Release review, 2026-10-21

Reviewed code-kb `v2.7.0..v2.8.0` and julie-extractors `v3.8.0..v3.9.0`, including their integration. Four independent, read-only reviewers covered caller reports, edit checks and hooks, extraction, and artifact compatibility. The lead reviewed indexing, query/body changes and the replay harness, reproduced the material findings, and reviewed the fixes.

## Findings and fixes

| Severity | Finding | Fix and evidence |
|---|---|---|
| High | A C# local in a completed nested block could override the type of a visible field, giving `Pick(item)` the wrong `arg_types` and hiding its real caller. | Julie now checks lexical scope before choosing a declaration. C# extraction regressions cover completed blocks and loop/catch/using scopes, active bindings, and unknown types. A related `typeof` correction emits `System.Type`, preventing confusion with a project class named `Type`. |
| High | C# integer literals were classified by suffix alone. For example, `2147483648` was called `int`, which could hide the call of another overload. | Julie now considers value, radix and suffix, preserving unknown values it cannot classify. The literal is `uint` under [C# integer-literal rules](https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/builtin-types/integral-numeric-types) and can convert to `long`; it is not `int`. |
| High | C# overload pruning erased custom namespaces. An `A.Token` argument could be assigned to a `B.Token` sibling overload after the original overload changed. Argument-count pruning could hide it independently. | code-kb retains custom qualification and requires exact overload type evidence before using an accepted count to omit a site. Regressions cover namespace collisions, a custom `String`, and calls that still fit the changed overload. |
| High | Java static-import verdicts depended on the first import. Competing imports could falsely prove or exclude both calls. | All explicit and wildcard imports are considered together. Reachable competing owners stay unresolved. Regressions cover both orders and mixed import kinds; direct CLI checks confirm order independence. |
| Medium | Two sessions blocking on one diff could receive the same follow-through outcome. | Telemetry stores session identity and scopes updates to root/session/hash. Inserts remain under the session lock. The two-session regression and legacy-database migration test pass. Old rows without identity stay unattributed. |
| Medium | Resuming a replay output directory after rebuilding a binary silently reused the old results, even when the version string stayed unchanged. | All replay runners verify the executable's SHA-256 before resuming. Changed binaries and unverified legacy runs require a fresh output directory; old results remain readable by `score`. Red/green tests cover all three runners. |
| Low | Julie's schema-v8 docs instructed users to force-scan an existing v7 database, but that command refuses the old schema. | Docs now specify a fresh database path. A direct CLI probe returned exit 3 for the existing v7 path and exit 0 for the fresh v8 path. |

## Dispositions

- A callback containing a regex brace was proposed as a false JavaScript proof. The direct CLI reproduction returned `likely` with an explicit callback-binding basis. It remains a conservative heuristic limit, not the claimed false-proof defect.
- `ExtractionLevel::References` adds a public Rust enum variant. Downstream exhaustive matches need updating. The release documents an incompatible artifact contract; this review does not alter published version numbers or remove the requested level.
- The exact caller and extraction regression tests are completion evidence. Historical reach/corpus scores in the release notes were not rerun or treated as fresh results.

## Verification

Linux verification on the final source:

- `JULIE_EXTRACT_BIN=/home/murphy/source/julie-extractors/target/release/julie-extract cargo test --workspace` in code-kb: 1,550 passed, zero failures.
- `cargo xtask test default` in Julie: 6,929 passed, seven existing ignored tests, zero failures; wall clock 163 seconds.
- C# golden fixture check: passed. The focused C# argument metadata target passed all five tests.
- code-kb workspace strict Clippy and Julie extractor strict Clippy across all targets: passed.
- code-kb plugin tests: 20 passed. Python replay tests: 64 passed.
- Formatting, guideline byte equivalence, diff checks, and both repositories' `check_edit`: passed; every changed API caller is reached by the diff.
- Direct CLI reproductions: the C# scope caller is restored, Java labels are independent of import order, and the documented fresh-v8 scan succeeds.

No fresh Windows run or large-corpus release replay was performed. Julie fixes are committed as `f9a4fa1f` on `review/v3.9.0-followups`. code-kb fixes are on `review/v2.8.0-followups`; their commit is recorded in the completion checkpoint and git history. No push, PR, release, or published pin change is part of this review.
