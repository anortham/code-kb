# Retrieval and test-reach regression cases

These eight tasks were used in a 72-run controlled agent pilot. They are now regression and tuning cases, not fresh evaluation evidence. The sample covers seven programming languages and JSON; the separate extractor conformance suite covers all 42 pinned language IDs.

The [protocol](../../docs/plans/2026-10-26-controlled-agent-comparison.md) records the controls and limits. [Results](results/observations.json) retain every original outcome, answer, source review, and action classification. [The compressed archive](results/raw-trials.tar.gz) contains raw trial events, calibration attempts, prompts, telemetry, and preflight evidence. [Artifact hashes](results/artifacts.json) bind that archive to the original manifest and freeze commit `8e07b094486c0f8c5d9ceca76ef79a882196cb7f`.

Current oracle data corrects the JavaScript and TypeScript test labels and declares two verified Rust command alternatives. The original [oracle](results/frozen-oracles.json), [task metadata](results/frozen-tasks.json), [manifest](results/frozen-experiment.json), and raw mechanical grades remain unchanged in the result evidence. No paid task was rerun to replace a result. Future instructions explicitly distinguish selected definitions from excluded alternatives; the original Go answers expose that ambiguity.

Give a regression agent one task prompt and the `fixture/` tree. Keep the oracle outside its project. Judge actual source definitions, caller chains, focused tests, literal values, and command evidence. A short answer or an indexed test name alone does not establish task success. Use untouched cases and a new frozen protocol for another comparison.

Run real extraction before using the fixture. `verify-extraction.sh` uses the pinned `julie-extract` at the `references` level and checks recorded definitions, static calls, and test calls in a temporary directory. The Python framework route and CommonJS handler binding have no extracted end-to-end edge from their test entry points. JavaScript has a `variable_ref` to the handler, which does not prove framework dispatch. These missing facts remain separate from consumer defects recorded by the pilot.

The CSV template defines measurement columns; actual per-trial data lives in the result evidence. Missing metrics remain unavailable. Billing was not supplied by the CLI, so the pilot reports observed token categories and time rather than dollar savings.
