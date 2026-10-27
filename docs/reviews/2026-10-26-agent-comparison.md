# Controlled agent comparison, 2026-10-26

The confirmed consumer defects are repaired: unrelated languages are filtered from call edges, types bind through exact scope/import evidence, typed receiver fields are followed, and broader test advice respects runner scope. The next milestone should measure completed edits on untouched, larger repositories across language families.

The same agents solved these small cases with native tools at lower observed cost on several tasks. This does not establish a default for large repositories or completed code edits. Keep the seven tools and all 42 supported language IDs in scope. Production routing is unchanged.

## Measured setup

The measured engine is the frozen configuration at `8e07b094486c0f8c5d9ceca76ef79a882196cb7f`, before the follow-up fixes. Eight reserved tasks covered seven programming languages and JSON. Each task ran in a fresh fixture-only Git checkout under native tools, current code-kb guidance, and revised guidance, three times each: 72 runs. Model `gpt-6.1-sol`, high effort, Codex CLI 0.164.0, engine/extractor bytes, answer contract, task set, and original grader stayed fixed throughout. The [protocol](../plans/2026-10-26-controlled-agent-comparison.md) records balanced ordering, filesystem isolation, calibration, redaction, and the 360-second deadline.

All 72 runs returned answers. No scored run needed an infrastructure retry. Three calibration calls plus one retained approval-configuration retry preceded the set. The retry was repaired with a run-scoped seven-tool allowlist; global configuration stayed unchanged. No external-model policy was declared; the synthetic fixture was sent to OpenAI. Billing was unavailable.

## Answer correctness and grading

All 72 answers identify the requested source facts correctly. The strict reviewed acceptance is 63/72, or 21/24 for each arm. Every completed action was reviewed: 689 actions total. These counts need the following distinctions.

- The raw mechanical grader passed 36/72. JavaScript and TypeScript answers use their actual literal test titles; the original oracle used nonexistent generated names. Rust answers use valid package/target command forms that the original matcher rejected. Source review accepted those 27 answers. Both Rust command variants ran the intended single passing test in isolated fixture copies.
- All nine Go answers identify `InvitationService.Send` and explicitly reject `MailService.Send`. They also list that excluded alternative under `symbols[]`, which the frozen strict rule rejects. The output contract did not clearly distinguish selected declarations from negative evidence. Preserve the rejection, but do not call it a wrong receiver choice or use it as a tool-value verdict.
- Two current-guidance literal runs invoked MCP before a native content search, contrary to the literal-first rule. Their answers are correct; their routing comparisons are inconclusive.

Current regression data fixes the two test labels, declares verified Rust command alternatives, and explicitly keeps excluded alternatives out of the selected-symbol list. The [original oracle](../../evals/review-2026-10-26/results/frozen-oracles.json), [original manifest](../../evals/review-2026-10-26/results/frozen-experiment.json), original answers, raw scores, and review overrides remain retained. No paid trial was rerun or replaced. These used cases are now regression/tuning data.

## Observed time and tokens

Each time below is the median of three warm task observations, in seconds. The fixture is deliberately small. All three observations, cold-index-inclusive times, token categories, calls, adoption classifications, and availability are in [summary.json](../../evals/review-2026-10-26/results/summary.json) and [observations.json](../../evals/review-2026-10-26/results/observations.json).

| Task | Native | Current guidance | Revised guidance | Strict acceptance per arm |
| --- | ---: | ---: | ---: | --- |
| Python route | 28.2 | 34.1 | 32.1 | 3/3 |
| CommonJS export | 24.6 | 31.2 | 32.9 | 3/3 |
| TypeScript callers | 26.2 | 42.4 | 40.0 | 3/3 |
| Go receiver | 27.2 | 55.2 | 42.4 | 0/3 |
| Java controller | 24.2 | 31.4 | 37.3 | 3/3 |
| C# optional argument | 33.1 | 44.6 | 37.7 | 3/3 |
| Rust integration test | 31.7 | 40.0 | 38.7 | 3/3 |
| JSON literals | 18.3 | 37.0 | 20.1 | 3/3 |

Totals below cover each arm’s 24 task runs. Input tokens include cached input; cached input is shown separately and must not be added again. Output is the CLI-emitted category, not an estimate from answer length. These are observed counts, not billed dollars.

| Arm | Input tokens | Cached input | Uncached input | Output tokens | Task seconds | MCP calls |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| native | 1,196,663 | 903,424 | 293,239 | 17,589 | 650.17 | 0 |
| current | 2,340,030 | 1,826,304 | 513,726 | 27,838 | 951.06 | 273 |
| revised | 2,197,102 | 1,759,360 | 437,742 | 24,929 | 879.87 | 154 |

Revised guidance made 154 MCP calls versus 273 for current guidance, about 44% fewer, with identical acceptance. Captured task time fell about 7.5% and input tokens about 6.1% relative to current guidance; both are within the protocol’s 10% practical tolerance. Some individual dimensions and tasks differ more. Do not turn these totals into a significance claim.

Both code-kb arms followed code-kb-first on all reviewed common structural actions: current 198/198, revised 104/104. Small-file reads, literal searches, setup, and interface-only steps were classified separately. Lower call count did not reflect lower structural adoption. Extra MCP use on the literal task remains visible.

Current/revised query telemetry contains 273/154 samples, with p95 25 ms in both arms. Median pre-index time was 0.064/0.032 seconds. Index-inclusive task times are recorded separately; fixture copying, policy checks, and review overhead are outside task timers. Calibration usage is retained separately and excluded from comparable task totals.

Under the frozen decision rule, C#, Java, CommonJS, and Rust favor native-first for these sampled small-file task types: both adopted code-kb arms have the same success and exceed native medians by more than 10% in every recorded time/token dimension. Python and TypeScript have conflicting dimensions, including lower uncached input in a code-kb arm. Go’s strict output ambiguity and the literal routing violations make those judgments inconclusive. Cache behavior, full guidance/tool context, and tiny files limit attribution. This compares complete configurations, not the marginal cost of one SQLite query. It does not justify a global routing change.

## Confirmed consumer defects

These observations are independent of the final answers, which often recovered through source reads.

| Defect | Evidence and repair scope |
| --- | --- |
| Foreign-language impact edges | Go `Send` returns a C# controller/test; TypeScript `send` returns Java/Rust callers and tests. Share call compatibility across every relationship, pending-call, constructor, and recursion branch. Preserve intentional interoperation and narrower identifier rules. |
| Foreign-language context dependencies/tests | Java and C# scored contexts include unrelated languages. Apply the same call-evidence rules to the demonstrated dependency and related-test queries. |
| Lost typed receiver chain | Go `h.service.Send` is excluded as though `h` itself were the service. The index already has the root type and exact owner field type. Resolve the complete bounded chain and use that proof consistently; ambiguity stays unproven. |
| Unrelated related type | Separate reviewer-only TypeScript context probes choose Rust `InvitationService` by global name and shortest path despite the TS import. Bind types through language, lexical owner, and indexed import evidence. These probes are not trial metrics. |
| Unsupported broader test scope | An empty Python walk recommends the root Rust workspace; a multi-package report recommends only its first package. Retain open-end/file scope and require a compatible command for every reported scope, otherwise show the manual fallback. |

The runner/formatter fix has 68 selector checks and six new scope regressions passing. Graph/context language guards pass three focused regressions. Related-type binding passes 12 real-extractor regressions, four private unit checks, and the existing context compatibility check. The typed-chain repair is committed as `e99aa82`, with 25 real-extractor regressions passing. It preserves absent targets and uncertainty, shares proof with impact queries, and bounds per-chunk evidence. Final integration verification is recorded below. All measured cost numbers remain the original pre-fix observations.

## Direction after this pilot

Keep source-backed accuracy ahead of adding features. The next evidence set should use untouched, larger repositories and completed edits with tests, where indexed navigation can plausibly repay its context and interaction cost. Include several language families, same-name decoys, and runtime entry points. The current sample cannot establish all-language agent performance or post-fix task savings. The existing real-extractor conformance suite remains the 42-language coverage gate.

The [raw archive](../../evals/review-2026-10-26/results/raw-trials.tar.gz), [artifact hashes](../../evals/review-2026-10-26/results/artifacts.json), and per-run reviews preserve the evidence. No publication, push, release, or global configuration change is part of this slice.

## Runtime verification after the pilot

Runtime commit `5f998c4` passes the full locked Linux workspace: **1,889 Rust tests across 37 suites**, including all 42 real-extractor language fixtures. The conformance suite took 13.11 seconds. Workspace Clippy with warnings denied, formatting, and the locked release build pass. The unchanged Python and plugin sources retain their passing 218 and 20 checks. The [verification record](../../evals/review-2026-10-26/results/post-fix-verification.json) and [Linux logs](../../evals/review-2026-10-26/results/post-fix-linux-verification.tar.gz) retain evidence. Windows NTFS verification at `f355247` also passes: **1,865 Rust tests across 37 suites**, all 42 language IDs, Clippy, formatting, and the locked release build. The plugin suite passes 12 tests with eight declared POSIX-fixture skips. The Windows executable starts successfully; its version and SHA-256 are retained in the [Windows logs](../../evals/review-2026-10-26/results/post-fix-windows-verification.tar.gz). Runtime sources match the Linux-verified slice; the intervening commit records guidance and evidence.

The freshly built CLI check and connected edit check were reviewed. The final CLI report has 11 intended shape changes, no proven or likely unchanged use outside the diff, and no cut sections. Its macro/value limitations remain; compilation and regressions provide the stronger verification.

A normal-parallel gate exposed a telemetry-fixture journal race and stale query-fixture assumptions; both were corrected without weakening freshness, language, or indexed-query requirements. It also exposed unknown-chain test labeling and repeated SQL compilation. The final repair probes each complete caller/name scope and each current/open-end impact frontier, including constructor-parent names. A real Go fixture preserves plain→typed→plain discovery across three hops. Unknown evidence retains the full proof. Indexed graph branches remain after an OR-based merge made execution take seconds. Related-type owner SQL is prepared once within each bounded context call.

Fresh MCP processes then ran the same 20 calls against the same final worktree/index, with the Windows guest off. Each row below shows the median of three warm observations after one initial call; this is a local diagnostic, not another agent campaign. The pair records identical source commit, diff hash, and status.

| Operation | Retained measured engine (ms) | Final engine (ms) |
| --- | ---: | ---: |
| Skeleton | 8.51 | 8.32 |
| Lookup | 64.25 | 64.42 |
| Context | 20.09 | 59.14 |
| Callers | 38.40 | 33.12 |
| Impact | 27.30 | 32.02 |

Retained RSS was 25.75/25.59 MiB, anonymous memory 15.76/15.49 MiB, and peak RSS 133.02/132.69 MiB. The [before](../../evals/review-2026-10-26/results/post-fix-rss-before.json) and [after](../../evals/review-2026-10-26/results/post-fix-rss-after.json) files retain all timings, response sizes, source identities, and binary hashes. The measured engine is `fbde44e013a80`; the final engine is `dab4bbdb3957`.

Context retains about 39 ms additional cost in this sample. The intermediate regression was much larger: about 808 ms context and 1,372 ms impact; the pre-optimization pair is retained separately. A second pair preserves the 88 ms context result before local owner-statement reuse. Actual typed-positive graph proof still prepares roughly 321 KB of SQL (about 642 ms in the debug diagnostic fixture). Compact ordinary paths do not establish uniform latency, large-repository behavior, or post-fix agent savings. Original pilot outcomes and costs remain unchanged.

The Windows clone has no tracked source changes. Its pre-existing `test.log`, last modified September 29, was left untouched. The guest was shut down and `win-test status` confirmed `shut off`. The final closure changes documentation, memory, and evidence only. No paid trial, push, publication, release, or global configuration change followed the retained pilot.
