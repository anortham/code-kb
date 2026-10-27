# Completed-edit readiness

Six pinned public repositories have source-reviewed reference repairs, passing upstream regression checks, an intended seed failure, and passing fresh patch replay. The cases cover Python, Rust, Go, TypeScript, Java and C#. Code-kb's product contract remains all 42 pinned Julie language IDs.

These artifacts are local readiness controls. No new provider call or scored coding-agent trial has run. The original 72-run navigation pilot is preserved separately.

- [Protocol and case schema](protocol.md) define isolation, patch capture, behavioral checks, source review and the budget boundary.
- [Corpus](corpus.json) records exact source pins and prior-corpus exclusions.
- [Frozen inputs](frozen-inputs.json) records tool/dependency hashes and durable seed snapshots. Task-owned prepared inputs live under `/home/murphy/.cache/code-kb-completed-edit/2026-10-27-fe7ede0`; installed toolchain roots are recorded individually.
- [Readiness evidence](results/readiness/artifact-map.json) maps 899 verified raw archive members. Each case directory under `results/readiness/` contains its reviewed readiness summary and source-review receipt.
- [Curation evidence](results/curation/artifact-map.json) preserves earlier setup failures and repairs. The raw archive remains unchanged when later source review adds evidence.
- [Runner verification](results/controls/evaluation/identity.json) binds the implementation and test results. Preparation and grading expose no provider credentials and disable networking during execution.

Run the local CLI from the repository root:

```sh
python3 scripts/edit_evaluation.py --help
python3 scripts/edit_evaluation.py validate-case --case-dir evals/edit-2026-10-27/cases/hcl
```

`readiness` accepts the case directory, pinned local source checkout, output directory, and explicit tool/dependency bindings. Supplied prepared dependencies skip online preparation. `grade-bundle` reconstructs independent public and hidden trees, verifies the patch identity, and runs the declared checks. Source review evaluates the request and added tests; matching the reference filename or exact patch is not an acceptance rule.

The runner needs Linux with bubblewrap. Code-kb's Windows runtime is verified separately. A paid comparison is bounded to 24 scored calls and at most 28 total calls, and requires a frozen executable/control manifest and its explicit budget receipt before dispatch.
