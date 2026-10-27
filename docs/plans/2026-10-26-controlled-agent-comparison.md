# Controlled agent retrieval comparison

**Goal:** Run the eight reserved navigation/test-selection tasks with competent native tools, current code-kb guidance, and revised code-kb guidance, then fix confirmed defects and report measured outcomes.

**Spec:** [Retrieval value plan](2026-09-23-retrieval-value-and-optional-routing.md), [project review](../reviews/2026-10-26-project-direction.md), and the owner's approval: "ok what's next? you're approved for whatever you need".

**Architecture:** Reuse the verified engine and synthetic fixture, Codex JSONL output, existing event-parser/telemetry conventions, and the existing CSV schema. Add a small runner/summarizer, not an evaluation service. This pilot measures navigation and test selection, not completed patches or all-language grammar parity. All 42 languages remain in scope; this task sample covers seven programming languages and JSON.

**Architecture Quality:** Read-only Astra review completed; its six required corrections are incorporated below before implementation and paid runs. Main risks: answer leakage, inherited host guidance/MCP configuration, grading answer-shaped guesses, and confusing token counts with billing or task success.

## Authority and bounds

- Worktree: `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, starting commit `708ed6c1a2812f58fc24cbd217b5b02c9c0b784f`.
- Local implementation, commits, and paid evaluation are authorized by the owner's approval above. No further approval round is needed for this protocol.
- Publication, push, release, and global configuration changes are outside this slice.
- The original experimental design has 72 scored runs and three non-task calibration calls, with a 360-second per-call timeout. Track infrastructure retries separately; no financial retry ceiling is imposed. A retry never replaces a task failure; retain every attempt. Diagnose repeated infrastructure failures before further dispatch.
- Use the user's configured model/effort, frozen as `gpt-6.1-sol` / `high`, with installed `codex-cli 0.164.0`. Keep them constant across arms. Do not switch providers/models to rescue outcomes.
- OpenAI is the selected provider. No external-model policy is declared in the project instructions; report `no external-model policy declared — synthetic fixture sent to openai`. Read the policy before every dispatch. Run the security-review outbound redactor on the constructed payload and fixture bundle; a redaction failure stops dispatch.
- Send only reviewed synthetic fixture files and the task prompt. Never copy the repository, oracle, reviews, memories, user instructions, or other task answers into a trial root.

## Frozen comparison

Tasks and oracle start at `evals/review-2026-10-26/{tasks,oracles}.json`. Commit the task-set fingerprint, scoring, arm guidance, and decision rule before the first scored run. Calibration uses separate synthetic inputs and no reserved task.

- Native: terminal tools only; no code-kb server or command.
- Current: the routing/skill policy at `708ed6c`, with native tools and the candidate code-kb MCP server.
- Revised: code-kb first for symbol/structure/caller/impact questions and non-small file interfaces; native tools first for literals and small files, and for checking incomplete/heuristic answers after code-kb. Tool inventory, engine binary, and exact captured MCP descriptions match the current arm. Both arms receive an explicit higher-priority instruction that the arm routing policy overrides generic read-order advice in tool descriptions. Calibration must demonstrate revised native-first reading of a small file.
- Repeat 1: native, current, revised. Repeat 2: revised, native, current. Repeat 3: current, revised, native. Every arm occupies every position once. This corrects the older plan's unbalanced order.
- Use one model session per task/arm/repeat. No shared conversations, generated memories, plugins, hooks, skills, web tools, or subagents. Ignore user configuration/rules while retaining existing authentication. Set project-document loading to zero and supply the frozen guidance explicitly.
- Each trial gets a fresh fixture-only Git checkout at the same seed commit, without a remote. Keep prompts, outputs, telemetry, and oracle outside it. Use a required Linux bubblewrap namespace to hide owner repositories/oracles, prior transcripts, and host guidance. Mount a fresh Codex profile at its normal filesystem path without changing HOME/CODEX_HOME; bind existing authentication read-only, never copy or log its contents. Use a read-only tool permission profile that denies the private Codex profile. Fail preflight/calibration if the oracle, host guidance, private canary, or unintended tools are accessible/injected. Record the actual isolation rather than assuming CLI flags prove it.
- Before any paid call, verify the extractor hash against scripts/julie-pins.json and retain successful verify-extraction.sh output. Pre-index the two MCP arms and record initial index latency separately. Report both warm task wall time and cold-index-inclusive time. Native gets no index. Record binary/extractor hashes and fixture fingerprints.
- Capture raw JSONL, final structured answer, stderr, wall time, exit status, and code-kb telemetry per trial. Extract actual input/output/cached tokens when emitted. Missing metrics stay unavailable, never zero. Money is unavailable unless the harness actually supplies billed cost.
- Do not tune the candidate, prompts, oracle, or scoring during the 72-run set. Infrastructure repairs are recorded separately. Keep successful and unsuccessful attempts. Retry only a documented provider/transport/startup infrastructure failure that yielded no completed answer. Model timeouts, bad answers, routing violations, and ordinary tool errors are outcomes, not retryable infrastructure. The first completed/non-infrastructure attempt supplies acceptance; add every attempt's available tokens/time to total cost and retain unavailable-attempt costs explicitly. No retry erases an outcome.

## Scoring and decision rule

Every arm gets the same structured output contract asking for requested definitions/tests/literals, file/line evidence, a source-chain explanation, and a focused command when evidenced or an explicit unavailable status with reason. Each answer must identify the requested source definitions, the correct focused tests, and relevant literal values/locations. Grade against the hidden source oracle, not output length or tool usage. Reject the Go decoy, nonexistent tests, invented runnable commands, and unsupported caller certainty. Allow equivalent fixture-relative Rust commands after relocation. Python's first decorator or definition line is accepted as the declaration location. Review the source-chain explanation and C# optional-argument requirement separately from mechanical field matching.

Record per-run fact checks and a reviewed acceptance result. Verify every accepted answer and every borderline rejection against source; automatic name matching alone cannot establish success. Incorrect extra tests/owners fail, as do missed requested facts. A mechanical grader produces provisional checks, never final acceptance. Record tool adoption on structural steps, retries, follow-up reads, unsupported claims, and metric availability. Treat each task's three repeats as a small pilot, not independent evidence for broad statistical claims.

- Consistent success means 3/3 accepted repeats. Compare per-task medians for wall time (warm and cold-inclusive), total input/output tokens, uncached input, and cached tokens separately. Use a 10% practical tolerance, show all three observations, and do not treat the pilot as a significance test. Token and latency evidence remain usable when billing is unavailable; never convert tokens to dollars. Keep code-kb-first under review when an adopted arm gets 3/3 with no measured dimension worse beyond tolerance. Conflicting dimensions or missing token/latency evidence are inconclusive.
- A structural step is a command/tool action to locate a named definition, understand a non-small file interface, locate callers, or select affected tests; literal searches and revised-policy small-file reads are separate categories. Review each completed action in the transcript. Adoption is matching structural actions that call code-kb first divided by matching structural actions; retain numerator/denominator and unavailable/ambiguous classifications. Compare revised versus current adoption on their common structural categories. A lower revised share is a guidance defect, not a negative tool verdict. Consider native-first only for this sampled task type when both code-kb arms adopt the tools, both have fewer accepted repeats than native, or both are worse beyond tolerance on every usable cost dimension without a success advantage. Do not change global routing from tiny synthetic fixtures. Low adoption is a guidance/isolation defect, not a tool-value verdict.
- Mixed repeats, unavailable token/latency evidence, or routing violations are inconclusive. Never delete a tool, exclude a language, or claim billed savings from this pilot.
- After the frozen set, reproduce confirmed product defects with focused real-extractor tests and repair them. Used tasks become regression/tuning evidence. Do not rerun a paid campaign merely to replace a disappointing result.

## Verification strategy

**Security scope:** outbound policy check/redaction and fixture-only staging. No dependencies added; no dependency-audit scope declared for this local runner slice.

**Worker:** parser/grader tests with replayed JSONL and negative answers; staging/config/scheduling tests with controlled local subprocesses. Test first for new behavior.

**Affected change:** Python tests for the new runner/summarizer, routing contract tests, mirrored-file comparisons, and a separate calibration proving native has no MCP and MCP arms have the seven expected tools.

**Branch:** applicable Python/plugin tests, guidance asset checks, `git diff --check`, `check_edit`, and release build when embedded routing assets change. Reuse the previously passing runtime suites unless runtime changes warrant affected regressions. Check source/worktree state before commits and completion.

## Parallel execution contract

Commit mode: `parallel-lead-commit`. Workers do not stage, commit, checkpoint, or launch model sessions.

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
| --- | --- | --- | --- | --- |
| Metrics/grading | A | `scripts/comparison_metrics.py`, `scripts/test_comparison_metrics.py` | No | None - safe parallel batch once the record contract is reviewed. |
| Campaign execution | A | `scripts/comparison_campaign.py`, `scripts/test_comparison_campaign.py` | Yes | Reviewed metrics and lead isolation helper; no provider calls by worker. |
| Runner/guidance/results | Lead | `scripts/agent_comparison.py`, `scripts/test_agent_comparison.py`, routing/skill mirrors, guidance docs, protocol, review fixture metadata, results/memory | Yes | Consume the reviewed metric interface; freeze inputs before scored dispatch. |

## Tasks

### 1. Freeze controls and prove isolation

**Files:** Protocol, `evals/review-2026-10-26/`, routing/skill mirrors, instruction/docs mirrors, narrow contract tests.
**Interfaces:** Same seven MCP tools and existing CLI; explicit arm instructions.
**Contract inputs:** Verified engine, eight reserved prompts/oracle, installed CLI help and current official documentation.
**File ownership:** Lead-owned protocol/guidance/assets/tests.
**Serialization required:** Yes.
**Dependency reason:** Controls precede calibration and scored calls.

**What to build:** Reviewed/frozen scoring and guidance, a fixture-only seed, and a verified isolated invocation. Preserve the original guidance as an experimental control.
**Approach:** Apply the existing routing plan without changing engine behavior. Keep all mirrored files synchronized. Use local CLI `--ignore-user-config`, `--ignore-rules`, `--ephemeral`, and JSON output; verify effective isolation with calibration.

- [x] Astra's concrete concerns are resolved and recorded.
- [x] Task/oracle/guidance fingerprints and decision rule are committed before scored calls.
- [x] Calibration demonstrates isolated instructions, native-only availability, and correct MCP tool availability without exposing the oracle.
- [x] Guidance/asset checks and mirrored-file checks pass.

### 2. Record and grade real sessions

**Files:** New runner/summarizer modules and tests, existing CSV schema and experiment assets.
**Interfaces:** Codex JSONL and the existing task/oracle formats; no new agent product tool.
**Contract inputs:** Frozen session/scoring protocol above.
**File ownership:** Metrics worker owns only its two files; campaign worker owns its runner and tests; lead owns isolation helpers and assets.
**Serialization required:** Yes for scored execution.
**Dependency reason:** Scored dispatch follows calibration and input freeze.

**What to build:** Bounded 72-run execution, complete durable evidence, factual grading and human/source review, and an outcome table.
**Approach:** Reuse existing parser/telemetry patterns, extend only for actual observed events, and test missing usage, invalid output, answer leakage, wrong owners/tests, and native-arm tool contamination.

- [x] Tests demonstrate correct counts, missing-metric behavior, negative grading, isolated staging, and balanced scheduling.
- [x] Every scheduled trial has a recorded outcome or an explicit infrastructure failure; no silent drop/replacement.
- [x] Every accepted answer is reviewed against source; instructions/adoption are assessed independently from answer correctness.
- [x] Outcome table distinguishes task success, tokens, latency, startup, unavailable billing, and inference limits.

### 3. Close confirmed defects and evidence

**Files:** Narrow affected product/regression files if failures prove defects; review, plans, guidance, memory.
**Interfaces:** Existing product contracts.
**Contract inputs:** Completed frozen outcomes; no speculative feature expansion.
**File ownership:** Lead assigns any confirmed independent implementation scope explicitly.
**Serialization required:** Yes.
**Dependency reason:** Findings must come from completed/retained evidence.

**What to build:** Verified fixes for actionable confirmed defects and a source-backed next-project decision.
**Approach:** Reproduce before fixing, preserve unfavorable experiment results, and label used cases as regression evidence.

- [x] Confirmed in-scope defects are fixed and narrowly verified; genuine upstream dependencies remain explicit.
- [x] Report and active brief reflect the measured direction and current authorization.
- [x] Local commits include docs/memory; task state is reconciled and no publication/global configuration changes occur.

## Verified external surface

Installed CLI help confirms `--ignore-user-config`, `--ignore-rules`, `--ephemeral`, `--json`, `--output-schema`, and per-invocation `-c` overrides. The selected model/effort match the existing user configuration; no global setting is changed. Official documentation supports JSONL events and structured answers ([non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode)), and developer instructions, disabled memories, named read-only permission profiles with deny paths, and project-document limits ([configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference)). Registered CLI features include disabling plugins/apps/hooks/memories/multi-agent/host skill discovery; calibration verifies the effective result rather than treating flags as proof.

## Execution rulings before dispatch

- The revised policy is an isolated experimental asset first; production guidance stays at the verified control until its adoption/behavior checks pass. Two routing copies and AGENTS/CLAUDE are mirrored; skills/code-kb/SKILL.md is a single shared plugin skill, so the older plan's second skill path was stale.
- A real bubblewrap test proves oracle/host-instruction masking and a fixture working directory. An actual `codex sandbox` invocation proves the generated read-only permission profile denies a private-profile canary. No model was called by those checks. The CLI splits dotted override paths naively; a quoted filesystem path must therefore be passed in an inline TOML table, which the integration test verifies.
- The extraction preflight passed:9 expected symbols,7 static calls,5 test calls, plus the CommonJS value binding; known framework gaps remain explicit. The downloaded Julie3.9.1 archive matched the pinned SHA and its binary matched `.tools/julie-extract` byte-for-byte. Raw hashes and extraction output are retained in target/review-comparison/2026-10-26/preflight.
- The immutable task/oracle/schema/guidance/tool-description manifest is evals/review-2026-10-26/experiment.json. Calibration is complete; no scored model calls have run yet.

## Calibration result

Frozen-control commit9762900 precedes all model calls. Native, current, and revised non-task calibrations completed. Native used2terminal actions/0MCP; revised read the2-line source directly and used0MCP. The first current run encountered the CLI's default MCP approval requirement. A tested per-invocation allowlist now approves only the seven code-kb tools inside the fixture namespace, without changing global settings or sandboxing. One shared infrastructure retry completed file_skeleton and get_symbol_body successfully. All valid controls reported the private canary unreadable; an independent real Codex sandbox test proves denied reads. The original blocked attempt is retained, not overwritten or scored. This is3initial calibrations plus1 of8allowed infrastructure retries;72scored trials remain. Calibration artifacts/validation.json are under target/review-comparison/2026-10-26/calibration.

The run-scoped tool-approval configuration follows the official [MCP configuration examples](https://learn.chatgpt.com/docs/extend/mcp). Successful model completion alone is insufficient for calibration: the current control required completed MCP results, and the revised control required the first source read to be native.

The final pre-scoring source audit corrected the C# `Invite` oracle from line4 (a brace) to line5 (the declaration). All other symbol/literal coordinates matched source. This correction precedes the final manifest freeze and all scored dispatch.

## Confirmed follow-up scope during the frozen run

Implementation waits until all 72 scored runs finish; the candidate binary and frozen inputs remain unchanged.

| Finding | Owner and files | Acceptance |
| --- | --- | --- |
| Broader suite advice loses target scope and selects a foreign runner | Lead: `formatters.rs`, CLI recovery regression; language-arguments worker: `test_commands.rs` only | Recover open-end/file-seed paths; choose compatible runner families; preserve package boundaries; refuse unknown or conflicting scopes; keep manual fallback. Cover every existing runner family, including script/JVM/.NET variants, and Windows wrapper behavior. |
| JavaScript oracle uses a synthetic test identifier | Lead: used-case metadata and result evidence | Preserve the frozen raw mechanical results; source review accepts the actual declared test label. Correct future regression data after the set, retain the original manifest and freeze commit, and do not rerun paid tasks to replace results. |

Read-only Astra review confirmed the runtime boundary above. A workspace marker alone does not establish that arbitrary Rust paths belong to a member. An explicit root-only query may still discover one unambiguous configured suite; a pathless symbol report must not silently become such a query. A command near one path must not imply coverage of mixed impact scopes.

Two further Go/TypeScript transcripts demonstrate graph-walk language leakage and loss of typed field-receiver chains. Source facts already contain the Go root and field types; this is consumer behavior, not an extractor dependency. The TypeScript oracle also uses a generated name instead of the declared literal test label; handle it identically to JavaScript.

Runtime follow-up order after the frozen set:

1. Language-members worker owns `queries.rs` and the call-compatibility functions in `reference_report.rs`, plus a new core `cross_language_graph_test.rs`. Share the existing call families (JS/JVM/.NET/C), preserve narrower identifier semantics, and gate every relationship/pending/constructor branch and depth. No competing shared-file edits.
2. After that worker hands off, language-conformance worker owns `find_related_types` and minimal binding helpers in `queries.rs`, plus a new `related_type_context_test.rs`. A separate reviewer-only context probe selected the unrelated Rust `InvitationService` for imported TypeScript parameters because global name/path-length ordering suppressed the correct class. Resolve using language and exact import/lexical owner evidence, preserve supported interoperation, and omit ambiguity. Keep these unscored probes out of campaign metrics.
3. After type-context ownership hands off, language-constructions worker owns those shared files for bounded, exact-owner typed receiver-chain proof and a new core `nested_receiver_test.rs`. Use one proof for positive and negative decisions; ambiguous or incomplete chains stay unproven. Cover Go/Java/C#, shadowing, same-field decoys, duplicate types, inheritance, callers, and impact.
4. Language-arguments worker can independently update `test_commands.rs`; lead independently updates formatter scope and CLI regression coverage. Workers do not format files owned by another worker.
5. Lead reviews each slice, integrates, runs affected scopes and the final branch gate, records the original frozen results, and updates the used-case oracle/metadata without replacing paid outcomes.

Astra reviewed these boundaries. Language compatibility permits further call matching; it never proves an edge. Indexed, bounded type/owner queries must preserve the memory invariant. No general type-inference engine is in scope.

The Java scored transcript also exposes unrelated Rust/TypeScript callee signatures in context. Include the demonstrated dependency query in the shared call-compatibility fix; related-type binding is a separate query and remains the type-context owner’s scope.

Task2 evidence is complete in [the baseline report](../reviews/2026-10-26-agent-comparison.md). The original72outcomes and all689actions are retained; future regression-data corrections do not replace measured grades. Runner/scope fixes are committed in `eb163ea`, graph/context guards in `a55711c`, type binding in `2033f69`, and receiver chains in `e99aa82`. Integration evidence below closes Task3.

Call-language graph/context repair passes three focused regressions and lead review after one fix round. Pending name-only type relations use the existing narrow identifier rule; explicit resolved non-call relations retain their behavior. The related-type and receiver-chain repairs are recorded below.

Related-type binding now passes 12 real-extractor regressions, four private unit checks, and the existing context compatibility test. Lead review closed imported-name provenance, qualified Java imports, and nearest-Go-module findings. The shared scalar returns a unique owner or NULL; indexed name/path/import queries, lexical/module depth bounds, and a 4096-fact context cap preserve bounded work. The subsequent receiver-chain repair uses this shared owner resolver.

## Final integration gate for consumer repairs

The follow-up changes shared query behavior, so the final gate includes the full locked Rust workspace suite on Linux and Windows NTFS, workspace Clippy with warnings denied, formatting, and locked release builds. Reuse the passing 218 Python and 20 plugin checks while their sources remain unchanged. Run the Windows plugin suite for platform evidence and shut the guest down afterward. Preserve prior declared platform skips.

Compare fresh MCP processes for the retained measured engine and final release on the same final worktree/index using the same 20-call workload. Record RSS and anonymous memory, three warm timings per operation, binary identities, and response sizes. Review the new release CLI check plus connected check_edit. These checks verify the runtime repair; they do not replace original pilot outcomes or claim post-fix agent savings.

## Required report-boundary correction

Read-only Astra architecture review found repeated receiver-proof SQL expansion remained a deadline risk after measured optimizations. Accept the bounded status-map approach: compute receiver owners and nullable target status separately for each existing judge chunk and target, then bind keyed statuses into the compact judge. Keep raw pending-row chain/language/line semantics, synthetic absent-target columns, existing claim/rival ordering, and shared graph proof. No repository graph cache, global statement cache, SQLite UDF, temporary writes, new dependency, budget increase, or general parser.

Architecture quality: affected modules are queries.rs and reference_report.rs; the caller-facing tools and core report interfaces remain unchanged. Complexity stays behind the private report/proof seam. Real-extractor reference/impact/edit tests verify behavior; count/size and measured prepare/step checks guard query expansion. This required correction is folded into the approved consumer repair because its deadline failures prevent correct completion. Risk is medium; the main acceptance risks are absent targets, nullable status, mixed chunks, and raw versus augmented JavaScript chains.

## Runtime repair closure

Receiver chains are committed in `e99aa82`. The final focused normal-parallel run passed 25 real-extractor cases in 8.73 seconds. Private quota, legacy-schema, and compact-statement checks passed. The report adapter uses separate bounded owner/base/status queries; its compact chain judge prepared in about 18 ms in the diagnostic fixture, compared with 0.7–1.06 seconds for the initial expanded statement. These are local query diagnostics, not post-fix agent outcomes.

The first final Linux gate exposed a telemetry-fixture race and a Clippy conditional warning. Commit `e85512e` moves parity-test telemetry outside the scanned root and combines the existing conditions. The failing parity test and three ancestry quota checks passed before the workspace gate was repeated. Freshness errors remain enforced.

Commit `ddebbec` corrects three stale query-test assumptions exposed by the workspace gate: language metadata is present in the minimal callee schema, same-language pending test reach has a coherent Rust fixture alongside a QML negative control, and the EXPLAIN assertion matches the complete symbols alias rather than a prefix of a bounded JSON virtual-table name. All three failing IDs passed individually before the final workspace rerun. No production rule was loosened.

The next complete gate exposed two further defects: unknown typed-chain claims bypassed the existing name-only late decision, and unconditional graph/callee proof expansion exceeded the existing receiver-query budget. Commit `16e124b` restores late labeling without losing positive import evidence. Commit `38add43` chooses query shape for complete caller/name scopes and every current/open-end impact frontier, including constructor-parent names. The original budget regression and affected graph/context checks pass. The real Go fixture now proves plain→typed→plain discovery across three hops. A trial merged OR join was rejected for seconds of execution time; indexed branches remain.

The retained pre-optimization probe pair records the discovered slowdown. Final paired results record the corrected release separately in the pilot report. Actual typed-positive graph proof remains more expensive to prepare; compact ordinary paths do not establish uniform latency for every chain.

The Linux gate at `38add43` passed 1,889 Rust tests and all 42 languages in12.52s. Its quiet paired probe recovered impact latency and stable retained/peak memory, but context still reparsed the owner query per fact (87.64ms versus20.06ms retained baseline). Commit `5f998c4` reuses one lazy statement within the bounded context operation;12type regressions and the fact-limit guard pass. The pre-owner-reuse pair is retained separately. Final verification follows that runtime commit.

The final Linux gate at `5f998c4` passes 1,889 Rust tests across 37 suites, all 42 language IDs, Clippy, formatting, and the locked release build. The current regression manifest validates engine `dab4bbdb3957`; the original frozen manifest remains unchanged. The final quiet probe pair and Linux log archive are retained under results. Windows NTFS verification at `f355247` passes 1,865 Rust tests across 37 suites, all 42 language IDs, Clippy, formatting, release, and 12 plugin tests with eight declared POSIX-fixture skips. Runtime source is unchanged from the Linux-verified slice.

Final Windows checks pass at the synced clean commit `f355247`; the binary starts and its hash is retained. The guest is confirmed shut off. A pre-existing September log in the guest clone was left untouched; tracked source is unchanged. The task worktree and original checkout are reconciled, and final closure contains documentation, memory, and verification artifacts only.
