# Retrieval value and default routing

**Status:** Proposed backlog, saved at the owner's request after the September 23
tooling discussion. No implementation or benchmark execution has started.

**Goal:** Keep the existing Tree-sitter/SQLite retrieval foundation. Keep code-kb
the default route for code-structure questions, and remove the blanket bans on
native search so that an agent can check or complete a code-kb answer. Measure
whether code-kb improves complete tasks before expanding its scope.

**Behavior contract:** code-kb is the first route for code-structure questions:
symbol definitions, a file's interface, callers and callees, and the tests a change
affects. Native search and file reads are the first route for literal text
(strings, errors, comments, config values) and for a small file the agent reads
or edits as a whole. Native tools are also the check after a code-kb answer that
is empty, capped, marked heuristic, or in conflict with other evidence. The
guidance names these cases explicitly. It does not say "use whatever is smallest",
because an agent's habit then wins and code-kb goes unused. Indexed references and
predicted tests remain useful but potentially incomplete. Report compression
estimates separately from measured task savings.

**Architecture:** Preserve the two-crate retrieval architecture, pinned
`julie-extract`, per-project SQLite index, and existing CLI/MCP tools. No rewrite,
new retrieval service, vector runtime, registry, editing system, or test runner.
The main risk is drawing conclusions from smaller output while missing evidence
needed for correct changes.

## Current evidence and constraints

- At inspection, `main` was `e9132c7` with unrelated source, instruction, test,
  release, and memory changes. Recheck the current state before execution;
  reconcile ownership before touching overlapping files. Do not overwrite or
  reset that work.
- The live index in this checkout contained 31 `docs/plans` Markdown files and
  no `.memories` file rows. This establishes inclusion, not harmful ranking.
- Token-savings telemetry compares returned output with an estimated full-file
  baseline. It does not observe what a native-tools agent would actually read.
- Exact symbol selection and result-limit work already exists in
  `2026-09-22-read-tool-precision.md`. Verify current behavior and reuse it;
  do not repeat completed work or restore that historical plan's old root policy.
- Preserve the current required `project_root` contract, freshness and path
  checks, Windows support, and honest truncation/heuristic notices.
- Pause speculative feature expansion while this comparison runs. Correctness,
  reliability fixes, and already-authorized work remain eligible.

## Tasks and acceptance checks

### 1. Keep code-kb the default for structure, and remove the blanket bans

**Files:** `hooks/code-kb-routing-block.md`,
`crates/code-kb-cli/src/routing-block.md`, `skills/code-kb/SKILL.md`,
`.claude-plugin/skills/code-kb/SKILL.md`, `CLAUDE.md`, `AGENTS.md`, `README.md`,
and the affected routing/asset checks under `tests/plugin/`.

- [ ] Remove the prohibitions: "DO NOT run `grep`, `rg`, `find`, `cat`, or
  `view_file`" and "DO NOT read an entire file". Remove the unmeasured "Saves
  80-90% context tokens" claim from the header.
- [ ] Keep direct, imperative wording for the code-kb cases. The guidance says "use",
  not "you may use":
  - A symbol by name: `lookup_symbol`, then `get_symbol_body` or `get_symbol_context`.
  - The interface of a file that is not small: `file_skeleton` before any full read.
  - Callers before a change to a function or type: `find_references`.
  - The tests to run for a change: `blast_radius`.
  - A concept with an unknown name: `search_symbols`.
- [ ] Name the native cases just as explicitly:
  - literal text
  - a small file read or edited as a whole
  - surrounding module state after the skeleton shows where to look
  - an index that is missing or still building
  - a code-kb answer that is empty, capped, marked heuristic, or in conflict with
    other evidence

  In the last case, check with native tools after the code-kb call, not instead
  of it.
- [ ] Do not add wording that weakens the code-kb cases as a whole, such as "optional",
  "if helpful", or "choose the smallest source". The failure this plan must avoid is
  an agent that reads the guidance as permission to skip code-kb.
- [ ] Retain guidance about exact symbols, explicit project roots, freshness,
  missing coverage, and heuristic reference results.
- [ ] Keep both routing copies, both skill copies, and the two project instruction
  files synchronized. Coordinate with Razorback's routing-policy TODO so its
  bootstrap does not override the revised guidance in either direction.
- [ ] Acceptance, guidance text: a plugin test checks that each code-kb case and
  each native case appears, and that the removed prohibitions and the savings claim
  are gone.
- [ ] Acceptance, behavior: in the Task 3 transcripts, on steps that match a code-kb
  case, the share of steps that call code-kb first does not fall below the share
  under the current guidance. A drop is a Task 1 defect to fix before Task 4. It is
  not evidence that code-kb has little value.
- [ ] Acceptance, fallback: a task that names a literal error uses `rg` at once. A task
  on a missing index finds a working native path. Existing retrieval tools remain
  callable, and their correctness contracts are preserved.

### 2. Separate compression estimates from actual task value

**Inspect:** `crates/code-kb-core/src/telemetry.rs`,
`crates/code-kb-core/src/formatters.rs`, `scripts/benchmark_quality.py`,
`scripts/test_benchmark_quality.py`, `README.md`, and both code-kb skill copies.

- [ ] Review rendered telemetry and benchmark descriptions. Label existing
  full-file comparisons as estimates, retain baseline-coverage reporting, and
  remove any wording implying they measure billed savings or successful-task cost.
  Reuse accurate existing wording; do not rename interfaces without a real need.
- [ ] Gather actual input/output/cache tokens, tool calls, latency, retries,
  follow-up reads, and acceptance results from agent-run logs for the comparison.
  Record unavailable measurements as unavailable rather than inferring them from
  omitted bytes. Separate initial index creation from warm query use.
- [ ] Acceptance: a reader can distinguish compression, query quality, and
  end-to-end results. A tiny response that misses the needed code cannot be
  counted as a successful retrieval merely because it saved output space.

### 3. Test code-kb against competent native retrieval

**Starting assets:** `scripts/benchmark_quality.py` and its tests for existing
query checks. Reuse captured agent transcripts for task outcomes; no new
benchmark platform is required. Razorback's TODO owns the shared task selection
and comparison protocol; this task owns the retrieval-specific evidence.

- [ ] Run after the code-kb release that pins julie-extract 3.6.0. Earlier builds
  miss callers through untyped locals (for example `let x = f()?; x.method()`), so a
  comparison on them measures a gap that is already fixed.
- [ ] Compare three arms on the same repository revisions and task inputs:
  - native tools only
  - native tools plus code-kb under the current guidance
  - native tools plus code-kb under the Task 1 guidance

  The second arm is the baseline for the code-kb adoption check in Task 1. Keep
  model, effort, harness, workflow instructions, and memory configuration constant.
  Use isolated run configuration; do not change the user's global agent setup to
  run a trial.
- [ ] Repeat each task at least three times per arm, and alternate the arm order
  between repeats. Agent runs vary from run to run, and a fixed order adds drift: in
  the 2026-09-24 latency check, a fixed order alone showed a false 5-15% slowdown.
  Report the spread, not only the mean.
- [ ] For each run, record per step whether the step matches a code-kb case from
  Task 1 and whether the agent called code-kb for it first.
- [ ] Include exact-name lookup, a literal error/config query, an unfamiliar
  concept, an indirect or ambiguous caller, and a change needing surrounding
  module state. Predefine the relevant source locations and acceptance behavior
  without exposing the solution to the agent.
- [ ] Check whether each route finds the necessary source, callers, and tests;
  record omissions, misleading matches, subsequent full-file reads, and repairs.
  On selected queries, record whether historical plans appear and displace useful
  current source. Preserve Markdown discovery for documentation/skill repositories.
- [ ] Inspect each result against current source and existing tests. Reference
  and blast-radius predictions must not be treated as proof that omitted paths
  or tests are irrelevant. Use the acceptance checks to judge the change.
- [ ] Acceptance: results show task success and measured cost by query/task type,
  including cold-start costs and failure cases. Comparisons use realistic targeted
  reads, not an assumed baseline that reads every referenced file in full.

### 4. Decide the next scope from results

- [ ] Write the decision rule before the first Task 3 run, and commit it with the task
  list. For each task type, it states what result keeps code-kb as the default route,
  what result moves that task type to native-first, and what result is inconclusive.
  A task type moves to native-first only when the code-kb arms used code-kb on its
  matching steps and still did worse on task success or measured cost. Low code-kb use
  under weak guidance never moves a task type. It sends the guidance back to Task 1.
- [ ] Record which existing tools help, which need output/coverage fixes, and
  which show no demonstrated benefit for the sampled tasks. Mixed evidence stays
  inconclusive; it does not justify a universal claim from a small pilot.
- [ ] Propose plan/source separation only if returned historical material causes
  real confusion. Preserve access to relevant documentation and history.
- [ ] Evaluate a language-server or embedding supplement only for a demonstrated
  semantic or vocabulary gap. A concrete failed task and an improvement over the
  existing native-tool fallback are required before adding maintenance scope.
- [ ] Acceptance: retain useful capabilities, identify narrowly justified changes,
  and leave unsupported expansion parked. No automatic deletion of tools or
  historical documents follows from low usage alone.

## Verification and execution boundaries

These tasks are sequential: establish the guidance and measurement definitions,
run controlled comparisons, then decide scope. One implementer is sufficient;
no parallel worker/report machinery is required by this backlog.

- Guidance/asset changes: `node --test tests/plugin/*.test.cjs` and `git diff --check`.
- Verify each mirrored pair with `cmp`: the two routing files, two skill files,
  and `CLAUDE.md`/`AGENTS.md`.
- If existing benchmark Python changes:
  `python3 -m unittest discover -s scripts -p 'test_benchmark_quality.py'`.
- If Rust behavior changes, first add a focused public-behavior regression and
  run its package/test target. At integration use the repository's current Rust
  test, lint, formatting, and relevant Windows gates; do not run them for a
  documentation-only update.
- Guidance tests prove the stated routing contract. Existing query tests prove
  retrieval behavior. Neither substitutes for the measured agent-task comparison.
- Use existing transcripts first. Obtain an explicit budget before paid model
  replays. Report missing outcome evidence plainly if replays are not funded.
- Security scope: no new dependencies or outbound source dispatch is required by
  the guidance changes. Future model replays must follow the repository's provider
  policy and use a reviewed, sanitized task bundle.
- Deliver the results as one small table beside the experiment notes. No push,
  release, global configuration change, or speculative feature implementation is
  authorized by saving this backlog.
