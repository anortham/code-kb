# Toolkit efficiency confirmation protocol

Status: frozen at 2026-11-04T01:42:06.047350+00:00, before production edits and confirmation exposure.
One candidate is selected in `finding.md`; its retention remains conditional.

The implementation request authorizes local evaluation and commits. It prohibits
pushes and releases. The binding design is
`docs/plans/2026-11-03-toolkit-efficiency-plan.md`.

## Sample and controls

Use six new tasks, two repeats per arm, spanning configuration, framework
entrypoints, and Python, JavaScript, Rust, Go, Java, and C#. The full pinned
repositories, seeds, requests, protected inputs, reference repairs, offline
dependencies, hidden checks, and readiness records are frozen in the manifest.
Task authors may inspect source and run readiness before freezing. No task may
be replaced or tuned after confirmation exposure.

"Untouched" means a new task request, seeded defect or requested feature, and
acceptance checks, not an unseen repository or absence from model training.
This follows this plan's task requirement and supersedes the 2026-10-27
protocol's repository exclusion list and size eligibility rule. Existing
repository exposure and sample limits must be disclosed. Historical six task
directories and results remain immutable development evidence.

Use the existing completed-edit runner and its two-arm manifest format.
`protocol` remains the unchanged historical protocol required by its validator;
`toolkit_protocol` binds this document and supplies the new sample, comparisons,
thresholds, and stop rule. All historical sandbox, capture, protection, source
audit, and grading boundaries remain in force.

Freeze Codex CLI 0.164.0, model `gpt-6.1-sol`, high effort, 900 seconds per
attempt, and identical task/dependency/profile inputs within each comparison.
Record executable hashes, versions, source commits, and guidance hashes.
The local runtime must come from a fresh `cargo build --release`; never use
the PATH code-kb binary. The model alias cannot freeze provider weights or
backend revision. The CLI supplies no supported sampling seed here. Provider
cache state and host scheduling are uncontrolled and must not be represented
as fixed controls. Billing is unavailable.

Run one slot at a time on this host. Repeat one runs native, current, then
candidate for each task in corpus order; repeat two runs candidate, current,
then native for each task in corpus order. This counterbalances each pair's
relative order. Use two ordinary manifests with the runner's fixed 24-slot
native/code-kb schedule. The first runs all 24 native/current slots. The second
runs only its 12 code-kb slots using the candidate runtime. Each native slot
is reused once through its immutable first-manifest reference, never
dispatched twice. `schedule.json` binds the 36 ordered dispatches and 12
matched sets without adding a runner arm. No scored trial runs concurrently
with another scored trial or build/readiness/branch-gate work on this host.
The coordinating session may inspect progress and report status; it must not
tune the candidate, cases, or grading rules after exposure.

## Readiness and acceptance

Before confirmation, verify the pinned public baseline, intended seed failure,
and passing complete reference repair in the existing offline grader. Retain
raw readiness records and independent source-review derivatives. Hidden tests
and repairs never enter the agent's mounted inputs. Each seed has one local
Git commit, no remote, and no code-kb index. Native cannot access code-kb;
current and candidate get their own hashed executable/extractor directories.

Final acceptance requires a completed model attempt, replayable patch,
unchanged protected inputs, passing canonical public and hidden checks, and
independent source review against the frozen request. Source review checks
all requested behavior, affected callers, valid inputs and edge cases,
meaningful new tests, and any unsupported safety or coverage claims. Run added
tests on a fresh public replay with trusted configuration and offline inputs.
Agents may choose appropriate new test filenames and classes; reference test
names are examples, never hidden acceptance conditions.
Preserve behavioral grades, source rejections, and incomplete attempts
separately. A passing hidden test alone is insufficient.

Candidate quality requires accepted patch counts at least as high as current
code-kb overall and for every individual task, and zero protected-input
violations. A quality comparison with native also requires accepted counts at
least as high overall and for every task. Counts describe this small sample;
they do not establish performance over all 42 languages.

## Measurements and fixed gates

Preserve `comparison_metrics.summarize_trace` missing values. Take input,
cached input, output, command counts, and MCP counts from each retained trace
receipt. Uncached input is input minus cached input only when both exist and
the values are valid. Report cached input and output separately. Never infer
tokens from characters, dollars from tokens, or missing durations from logs.

Use the runner's index-inclusive agent duration, including cold indexing for
code-kb. Native indexing is not applicable, not a missing measurement.
"Cold" means a missing project index; OS and provider caches are not flushed.
Report agent and index durations separately as well. Preparation, offline dependency
setup, grading, and independent source review are outside this duration;
disclose them rather than adding them selectively to one arm.

For a complete comparison, sum each token metric over its 12 trials and
compare summed index-inclusive agent time over those same trials. A retained
candidate requires both total and uncached input at most 95% of current
code-kb, time at most 105%, and the quality gate. Whole-task savings against
native require the same thresholds against native and its quality gate.
Report these two gates separately. With no candidate, candidate retention is
not applicable; report current-versus-native using the native savings gate.
Five percent is a practical threshold, not statistical significance.

List all 12 matched sets, with each runtime/manifest/slot identity, acceptance,
protected inputs, source review, total/cached/uncached input, output, shell/MCP
calls, index-inclusive time, and unavailable fields. Missing acceptance,
required tokens, or index-inclusive time in any required slot makes its full
comparison inconclusive. Partial totals may be shown only with their explicit
denominator and cannot pass the full gate. Fewer calls or smaller answers
cannot substitute for these gates.

## Stop rule and evidence

One candidate at most and one fixed confirmation sample. Never rerun a
completed model attempt, repair its patch, tune against results, drop a failed
slot, or extend the sample. Preserve every attempted slot unchanged. A setup
failure before exposure may be repaired, with its original evidence retained;
run only the unexposed slot in a separately identified recovery output. Any
attempt for which dispatch may have occurred is exposed until evidence proves
otherwise. Do not retry provider failures after exposure under this protocol.

An ineffective candidate is removed from the retained product diff without
destroying its source, build identity, test evidence, or attempt archive. A
negative or inconclusive result makes no corresponding savings claim. If
evidence requires changing tools, schemas, references, index freshness,
language scope, Stop behavior, or sandbox boundaries, stop and revise the
design with the owner. Cross-tool smoke and Razorback changes remain outside
this repository's ownership.

Freeze this document, trace finding, corpus, cases, readiness derivatives,
runtime identities, profiles, controls, schedule, and manifests before the
first scored dispatch. Store raw attempts under ignored
`target/toolkit-efficiency/` and commit redacted result records plus a hashed
archive inventory under `results/`. Never rewrite historical evidence.

K1 freezes the sample, current-runtime manifest, comparisons, thresholds,
source seam, and ordered schedule before K2 edits production code. K2 adds
the candidate executable hash and its ordinary manifest after building it;
that build-dependent identity cannot exist earlier. It must copy the frozen
case/readiness/schedule and all non-candidate controls exactly. No K1 task or
acceptance input changes during that final runtime binding.
