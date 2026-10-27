# Completed-edit protocol corrections

This amendment extends [the original protocol](protocol.md). The original manifest, cases, attempts, patches and strict grades remain unchanged. Record both the original results and corrected results, with explicit lineage.

## Case corrections

- Date-fns: exclude `pkgs/tz/test/engines/node_modules`, which the canonical offline install creates. The first two affected slots stopped before any provider call or task exposure. The task, source snapshot, dependencies, public commands and reference repair remain unchanged.
- HCL: declare `specsuite/tmp_hcldec` and `specsuite/tmp_hclspecsuite` as generated outputs. The protected suite builds them, and its protected `.gitignore` ignores them. Keep the original captured binaries as evidence. Their presence is a capture defect rather than an agent source-correctness failure.
- MiniJinja: accept the accurate help wording `without regard to ASCII letter case`. Continue requiring all three wrapper suffixes and an explicit case-insensitivity explanation in the autoescape section. Distinguish `.jinja` from `.jinja2`. Retain positive and negative matcher checks. Apply the corrected checker uniformly to all four captured MiniJinja patches.
- Test summaries: permit additional passing tests above a verified baseline. The first NodaTime patch passed 296 tests, including 48 additions, but the original literal markers required exactly 248. Keep named tests, success/failure status and ordinary numeric output strict. Require the baseline number to pass, reject empty or skipped-only runs, and preserve separate suite summaries. Apply the corrected grader uniformly to all four C# patches.
- NodaTime source regression: also run the existing protected `NodaTime.Test.Annotations.PurityTest.AllPublicStructMethodsArePure` test on every C# patch. It requires new public struct methods to carry `[Pure]`; `CompareTo` itself is an explicit exception. Uniform independent checks found the missing annotations in native slots 011/024 and passing annotations in code-kb slots 012/023. Keep this genuine regression separate from the test-count false negatives. The versioned case adds that existing check to automate the source-review finding; model tasks and outputs stay unchanged.

Each corrected case gets a separate directory, source/hash comparison and fresh readiness evidence. Do not edit the frozen original case directories. Model, effort, profiles, guidance, SDK/dependency inputs, engine and extractor stay fixed.

## Execution and scoring

Finish the original timed schedule before correction builds or extra tests. Then run only the previously unexposed TypeScript cells through an amended manifest, in their original within-case order: native repeat 1, code-kb repeat 1, code-kb repeat 2, native repeat 2. Retain every original setup failure and new attempt. Do not rerun a completed model trial to improve its result.

Reconstruct and regrade the four original MiniJinja and four C# patches with the versioned checker/grader, without model calls. Keep original strict and corrected grades separate. Independently verify added public tests in fresh public replays, with protected canonical inputs and unchanged source snapshots.

Source review assesses the frozen task requirements and actual implementation. Workspace style preferences absent from the external task/profile are not hidden acceptance conditions. Passing behavior alone still does not establish source acceptance.

Report actual dispatches, setup failures, original and corrected acceptance, paired task/index time, tokens, tool adoption and native fallback. Disclose the amended TypeScript execution order. Missing metrics and billing remain unavailable. Do not change global production routing from this pilot.
