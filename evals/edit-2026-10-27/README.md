# Completed-edit comparison

Six pinned public repositories have source-reviewed reference repairs, passing upstream regression checks, an intended seed failure, and passing fresh patch replay. The cases cover Python, Rust, Go, TypeScript, Java and C#. Code-kb's product contract remains all 42 pinned Julie language IDs.

The original 24-slot schedule is complete: 20 model trials finished, and four TypeScript slots stopped during setup before model exposure. Three non-task calibration calls are also retained: the first found a missing Codex companion executable, and both arms passed after that runtime fix. The original 72-run navigation pilot is preserved separately.

[Protocol corrections](protocol-v2.md) document generated outputs, test-summary counts, the MiniJinja help checker and the existing NodaTime purity check. Corrected grades remain separate from the original grades. The four previously unexposed TypeScript cells have now completed under the amended protocol. All four pass behavioral grading, independent source review and added-test verification. Final acceptance is 10/12 native and 12/12 code-kb; the report retains the time and token tradeoffs. The original 20 model trials were not rerun.

- [Comparison report](results/report.md) records accepted patches, observed tokens, time and the limits of the study.
- [Protocol and case schema](protocol.md) define isolation, patch capture, behavioral checks, source review and reproducibility controls.
- [Corpus](corpus.json) records exact source pins and prior-corpus exclusions.
- [Frozen campaign](manifest.json) binds the 24 scored cells, model, timeout, executable, extractor, profiles, guidance, evaluator, readiness archive and prepared tool/dependency hashes.
- [Amended campaign](manifest-v2.json) binds the base protocol and corrections separately, with verified readiness and source-equivalent seeds. Only TypeScript slots 7, 8, 19 and 20 are selected for supplemental dispatch.
- [Frozen inputs](frozen-inputs.json) records tool/dependency hashes and durable seed snapshots. Task-owned prepared inputs live under `/home/murphy/.cache/code-kb-completed-edit/2026-10-27-fe7ede0`; installed toolchain roots are recorded individually. The finalized engine and extractor are retained in that cache under `tools/intelligence/`.
- [Readiness evidence](results/readiness/artifact-map.json) maps 899 verified raw archive members. Each case directory under `results/readiness/` contains its reviewed readiness summary and source-review receipt.
- [Curation evidence](results/curation/artifact-map.json) preserves earlier setup failures and repairs. The raw archive remains unchanged when later source review adds evidence.
- [Frozen study runtime](results/controls/runtime/identity.json) records its Linux/Windows verification, six isolated MCP controls and live watch/RSS probes; its [archive map](results/controls/runtime/artifact-map.json) verifies retained logs and diagnostic drivers.
- [Debouncer runtime](results/controls/runtime-backport/identity.json) records the later removal backport, Linux/Windows verification, live memory/handle probes and confirmed VM shutdown. The study binaries remain frozen.
- [Subsequent Claude review](../../docs/reviews/2026-10-27-claude-branch-review.md) records the queued walker-error correction and fresh Linux/Windows verification.
- [Current evaluator verification](results/controls/spending-removal/identity.json) records removal of financial approval/cap logic and validates the updated technical manifest; historical verification records describe their original revisions.
- [Calibration evidence](results/controls/calibration/identity.json) preserves all three calls, successful private-profile/network denial probes, fresh patch replay, and the public-source outbound audit.
- [Dispatch verification](results/controls/dispatch/identity.json) binds the reviewed launcher, 281 Python tests, real offline preindex, native engine isolation, and the final 32-control manifest.
- [Runner verification](results/controls/evaluation/identity.json) binds the implementation and test results. Preparation and grading expose no provider credentials and disable networking during execution.

Run the local CLI from the repository root:

```sh
python3 scripts/edit_evaluation.py --help
python3 scripts/edit_evaluation.py validate-case --case-dir evals/edit-2026-10-27/cases/hcl
```

`readiness` accepts the case directory, pinned local source checkout, output directory, and explicit tool/dependency bindings. Supplied prepared dependencies skip online preparation. `grade-bundle` reconstructs independent public and hidden trees, verifies the patch identity, and runs the declared checks. Source review evaluates the request and added tests; matching the reference filename or exact patch is not an acceptance rule.

Manifest validation requires an explicit absolute artifact binding for every hashed control; it checks the active evaluator sources as well as recorded bytes. Host-local bindings remain outside agent mounts. A changed binary, case, profile or dependency invalidates the freeze.

The runner needs Linux with bubblewrap. Code-kb's Windows runtime is verified separately. The comparison uses 24 scored runs with frozen inputs and successful readiness checks. Model evaluation is covered by the approved work and needs no separate spending approval or receipt.
