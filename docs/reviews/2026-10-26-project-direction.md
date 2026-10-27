# code-kb project direction review

Reviewed on 2026-10-26, starting from `main` at `d1ab7d9` (v2.9.1), with
julie-extract 3.9.1. The initial checkout was clean and had one worktree.

**Recommendation:** keep the smaller tool surface and the accuracy direction.
Make the next milestone about agents completing correct edits with less rework
across every language supported by the pinned julie-extractors.
The current product is a useful navigation and evidence engine; its reference,
test-selection, and edit-check answers still require judgment about coverage.
Smaller responses and honest warnings are valuable, but neither establishes
that an agent finishes a task more reliably or cheaply.

**The removals make the interface more coherent.** Their effect on completed
agent tasks still needs a controlled comparison.

| Change | Assessment | Remaining tradeoff |
| --- | --- | --- |
| `codebase_outline` folded into folder `file_skeleton` | Keep it. Repository and file orientation form one natural progression. | The CLI retains additional outline controls. |
| `search_symbols` folded into `lookup_symbol` | Keep one discovery entry point. Known names and unfamiliar concepts need less tool selection. | Ranked search runs only without an exact name; CLI `search --explain` remains useful for diagnosis. |
| `find_structural_facts` removed from MCP; CLI `facts` retained | A defensible simplification, but the most significant capability loss for an agent restricted to MCP. | Route, SQL, model, and configuration queries need terminal access. Include these tasks in the next comparison before deciding whether to expose facts again. |
| `telemetry_summary` removed from MCP; CLI `stats` retained | Keep it outside the main coding surface. | Operational inspection remains available through the CLI and skill. |
| Earlier removal of `edit_file` and `replace_symbol_body` | Keep editing in the host's native tools. The product can specialize in understanding and checking changes. | code-kb no longer supplies its own atomic-edit, syntax-validation, or body-hash protections. Native editing and normal verification must supply those checks. |

The seven remaining tools cover orient, locate, inspect, trace, assess impact,
and check an edit. Body and context are useful separate choices: reading one
implementation need not also return dependencies and tests. References and
multi-step impact answer different questions. Seven is a good current result,
not a permanent limit on future capabilities.

**The architecture has a sound foundation.** The two-crate boundary, shared
operations for CLI and MCP, required project root on each call, per-project
SQLite indexes, explicit symbol IDs, and pinned extractor all serve real agent
needs. The references-level index also avoids collecting extractor data the
product does not consume. Preserve those decisions.

Complexity is concentrated in language resolution and change analysis:
`reference_report`, its language-specific helpers, `edit_check`, and `queries`.
References, edit checks, and impact already share reference evidence. Preserve
that shared policy and test the same source sites through all three interfaces.
Moving large functions into more files would not by itself improve correctness.
When repeated source parsing compensates for missing AST facts, prefer adding
the reusable fact in julie-extractors over another independent parser here.

**The initial review found uneven language coverage.** The owner's requirement is support
for every language julie-extractors supports. Indexing and basic navigation use
the shared extracted records, but advanced analysis adds language-specific rules.
Extracting a language does not establish that every applicable check works for
it. The starting implementation had these concrete coverage limits:

| Check | Starting language/type gate | Evidence |
| --- | --- | --- |
| Accepted argument ranges | Python, Java, C#, Go, Rust, Kotlin, TypeScript/TSX | `rule_language` in [arity.rs](../../crates/code-kb-core/src/reference_report/arity.rs) returns no rule for other languages. Applicability depends on the language's call semantics. |
| Named-parameter rename detection | Python, Kotlin, C#, Swift | `NAMED_ARGUMENT_LANGUAGES` in [signatures.rs](../../crates/code-kb-core/src/edit_check/signatures.rs) omits other languages with named arguments, including PHP, Scala, Ruby, and Dart. |
| Field changes and affected constructions | Rust structs, Go structs, Python dataclasses, Java records | `owner_of` in [fields.rs](../../crates/code-kb-core/src/edit_check/fields.rs) rejects other owners, including C# records, Kotlin data classes, and Swift structs. |

These are baseline examples, not a complete language-by-capability inventory. The
follow-up inventory is [language support](../language-support.md).
Language-specific rules are necessary where semantics differ. Missing equivalent
capabilities in other applicable languages are coverage gaps. The initial
Python-first recommendation over-weighted the Flask dogfood examples; the next
work must address the cross-language requirement explicitly.

**Three concrete defects were reproduced during this review.**

- A failed incremental refresh could leave MCP and CLI serving successful stale
  answers. After indexing `old_target`, changing it to `new_target`, and forcing
  the extractor update to fail, lookup still returned the removed function.
  References also reported the stale target successfully. MCP disclosed no
  error; CLI logged a warning but exited successfully. The fault crosses
  [reconciliation](../../crates/code-kb-core/src/sync.rs),
  [watcher handling](../../crates/code-kb-core/src/watcher.rs), and
  [MCP preparation](../../crates/code-kb-cli/src/mcp/server.rs).
- `lookup Widgt --kind function` suggested `class Widget` when both normal
  lookup result lists were empty. That miss path omitted the kind filter in
  [lookup_symbols_op](../../crates/code-kb-core/src/ops.rs).
- The [quality benchmark](../../scripts/benchmark_quality.py) expected the old
  lookup JSON array. Four of six checks became `N/A` against v2.9.1 because the
  current result is an object with `names` and `ranked`, and the script swallowed
  the parsing failure. Two live-project graders also contained obsolete
  expectations. The Python evaluation tests were absent from CI.

**What the evidence establishes.**

The fresh retrieval run used the unchanged v2.9.1 release binary and the existing
11-task corpus: one warmup and three measured passes, 14 tool calls per pass.
Its recorded query latency was 3 ms median and 33 ms at the 95th percentile,
with 11,904 estimated response tokens per pass and the one expected wrong-path
error. This is a scripted retrieval and telemetry check. It neither assesses
answer correctness nor measures completed agent work; its savings baseline is
full-file reads, not observed native-tool behavior.

The recorded [planted-bug replay](../bug-replay-baseline.md) is more relevant to
test selection. In the October 22 reach build, symbol-seeded answers on its
holdout half missed 85% of failing tests in Cobra, 57% in Express, and 52% in
Flask. These are missed failing-test counts, not the percentage of bugs an agent
would fail to detect. A single selected failing test can still expose a bug.
The new warnings removed silent misses at default depth on that corpus, but
two fixes used holdout cases, so those results are now tuning evidence.

The [edit replay](../replay-baseline.md) likewise explicitly spent its holdout
on fixes. Its older results are useful diagnoses, not a new measurement of
v2.9.1. The 8-of-88 session-event catch result also includes events that are not
shape changes; it cannot be presented as an edit-check success rate. Existing
live agent checks and release dogfood uncovered real problems, but they are
not a controlled comparison against competent native retrieval.

**Next work, in priority order.**

1. **Establish language coverage and a fresh outcome baseline.** Inventory every
   language in the pinned extractor against each applicable tool capability,
   distinguishing implemented, limited, missing, and not applicable. Add
   conformance cases for every supported language; identify whether each gap
   needs an extractor fact or code-kb behavior. Choose fresh agent tasks across
   language families rather than treating Flask results as representative.
   Reuse the [existing comparison plan](../plans/2026-09-23-retrieval-value-and-optional-routing.md),
   updating its obsolete tool names and versions. Compare native tools with
   native tools plus the current code-kb under the same model, harness, tasks,
   and acceptance checks. Include concept discovery, a multi-file signature
   edit, an ambiguous caller, a literal/configuration task, and a framework
   test path. Reserve new tasks before tuning and repeat runs. Measure accepted
   changes, missed required edits, repair rounds, wall time including cold
   indexing, and observed tokens. Record tool adoption separately: enforced
   calls are not proof of value. Paid runs require a separate budget approval.

2. **Close applicable edit-check gaps across languages.** Removed members and
   functions passed as values are candidate capabilities to investigate using
   the coverage inventory. Implement shared detection over Julie facts where
   possible, with language rules and tests for differing semantics. The
   [v2.9.1 dogfood](2026-10-22-v2.9.1-dogfood.md) records concrete examples:
   `Request.routing_exception`, `App.subdomain_matching`,
   `Flask.request_class`, and `_async_wrapper=self.ensure_sync`. Keep those as
   Python regression cases, alongside equivalent cases in other applicable
   languages. Select the implementation order from the verified gaps and fresh
   tasks. Acceptance should include newly caught required edits, unchanged valid
   edits, false blocks, and latency. Keep blocking limited to evidence strong
   enough to justify it.

3. **Make an incomplete answer give a usable next action.** Preserve uncertainty
   labels and open-end warnings. Supply an executable broader test command
   when project evidence identifies the runner and configuration; otherwise,
   name the broader scope and the missing command information. Make folded
   results retrievable from MCP. `N more in CLI --json` currently forces a second
   interface. Measure whether agents retrieve and use the extra rows before
   choosing pagination, section filters, or structured results. MCP supports
   [structured results and output schemas](https://modelcontextprotocol.io/specification/2026-07-28/server/tools#structured-content),
   but adding those fields alone would not resolve missing coverage or prove
   client compatibility. Clear recovery instructions and bounded output also
   follow [published agent-tool guidance](https://www.anthropic.com/research/writing-tools-for-agents).

4. **Improve retrieval and test reach from failed tasks, not broad heuristics.**
   Concept search is lexical over indexed names, signatures, and documentation;
   the release dogfood's account-registration query ranks helpers above the
   intended view. Build source-backed retrieval cases before changing ranking
   or adding body vocabulary. For test reach, address CommonJS export bindings
   and runtime/framework entry points only where doing so yields useful test
   choices. The existing depth experiment shows why a blanket deeper walk is
   unattractive: depth 5 selects about 70% of the Cobra suite and frequently
   reaches the result ceiling, while barely helping some Flask/Express cases.
   Score bugs detected, failing tests missed, and fraction of suite selected
   together. A correct full-suite recommendation can be the appropriate answer.

5. **Keep evaluation and release evidence trustworthy.** Preserve old corpora
   as regression sets and add new held-out projects/tasks. Caller truth currently
   covers nine projects across seven languages, while planted test-reach bugs
   cover three projects and release dogfood uses Flask. Tie future release
   reports to the relevant source or binary fingerprint; accepting an arbitrary
   ancestor PASS does not establish that later semantic changes were checked.
   Allow docs/version-only changes without requiring an expensive rerun.

**Defer:** more general MCP tools, native editing/test execution inside code-kb,
an embeddings runtime, compiler-scale resolution, and a broad architecture
rewrite. Each needs a demonstrated failed workflow and a measurable improvement
over the existing fallback. Stronger edit checking across applicable languages,
with clear coverage and recovery, is the proposed next product slice; fresh
agent-task results should determine whether it earns that investment ahead of
retrieval improvements.

The review covers recent changes, tool contracts and plugin guidance, index
lifecycle, shared query architecture, evaluation/release evidence, and local
Linux verification. The repository's CI matrix also covers macOS and Windows;
those platforms were not rerun locally for the initial review.

**Repairs included with this review.**

- Reconciliation now finishes the updates it can, then reports failures with
  bounded details and the root. MCP also propagates preparation failures and
  keeps watcher failures pending until recovery succeeds. An unrelated
  successful event cannot erase an earlier failure, and recovery cannot clear
  a newer concurrent failure. The stricter behavior intentionally returns an
  error until freshness can be established. The watcher retains one bounded
  error and a generation counter, not repository data. Unknown and removed
  tools are rejected before waiting for index preparation, so an unhealthy
  index cannot mask the tool-removal error.
- Lookup miss suggestions now use the same stored/display-kind predicate as
  the ranked fallback. Regressions cover the wrong-kind example and the
  `attribute` display alias.
- The benchmark parses current lookup results and rejects malformed output
  as a harness error. A real rerun against the same release binary scored all
  six targets at rank 1; previously four were silently unscored. This repairs
  the measurement, not the retrieval engine. The two stale graders were
  corrected, and 149 Python harness tests passed and were added to CI.
- Legacy integration fixtures now use scanned indexes when they exercise
  incremental updates, and the stdio fixture accounts for each file on disk.
  Their case, path, addition, deletion, and isolation assertions remain. The
  worktree test keeps its telemetry outside the scanned project so a temporary
  SQLite journal cannot race reconciliation. Documentation now points to the
  extractor pin and consistently lists the four CLI-only query commands.

The original stale-answer reproduction was rerun against the release build:
with the failing extractor wrapper, the CLI exits 1 with the project and cause;
after restoring the extractor, it exits 0 and finds `new_target`. The live MCP
smoke benchmark measured final RSS of 29.24 MB before and 29.77 MB after the
changes, with its changed-file probe visible in about 212 ms in both runs.
These are small local measurements, not large-repository performance claims.

**Initial review verification:** 1,663 Rust tests, 149 Python harness tests, and 20 Node
plugin tests passed. The release build, workspace Clippy with warnings denied,
format check, whitespace check, and AGENTS/CLAUDE synchronization check passed.
`check_edit` identified the two expected shape changes: new reconciliation error
variants and the watcher's private failure state. The error uses were reviewed
and compiled; no missed required edit was found. Its unresolved sites and known
coverage omissions were not treated as proof of safety. At that initial stage, no paid agent comparison,
push, release, or Windows/macOS test run was performed.

**Local cross-language follow-up.** The approved implementation keeps the seven
MCP tools and their CLI equivalents. Both guidance files now explicitly require
every pinned Julie language and distinguish missing facts from semantic N/A.
[Language support](../language-support.md) records all 42 IDs, real-extractor
fixtures, deeper semantic tests, and remaining limits.

- Named-argument and range checks now cover more language families, including
  PHP, Ruby, Scala, Dart, F#, VB.NET, R, and PowerShell where their semantics apply.
  PHP extra positional arguments, R/PowerShell prefixes, Swift external labels,
  C declarations, receiver arguments, defaults, and variadics have valid-call
  regressions. A total count cannot stand in for named-argument binding.
- Construction checks cover Kotlin data classes, Scala case classes, C# records,
  Swift memberwise initialization, C/C++ aggregates, Zig structs, F# typed
  records, Elixir structs, and Ruby keyword structs. Defaults, custom
  constructors, copy updates, and known foreign owners retain their own rules.
- Removed members include indexed class variables, properties, constants,
  instance attributes, and callable attributes. Changed callable values are
  advisory dependencies; their framework invocation is not proven. Typed
  foreign receivers, assignments, overload ambiguity, and same-line calls have
  precision regressions.
- Erlang/GDScript and one-line structural skeletons, embedded HTML calls, and CSS
  custom properties/functions now consume facts the initial fixtures exposed.
  CSS compatibility corrections preserve IDs, source ranges, and stored written
  signatures. They use bounded indexed queries and exact source evidence.
- Folded reports can be paged through MCP and CLI. Page sizes remain bounded;
  callers and edit-check offsets can pass 200, while impact traversal keeps its
  explicit 200-row discovery ceiling. Incomplete test reach supplies a broader
  configured command or names the missing runner evidence.
- Release evidence fingerprints semantic inputs and records binary identity
  separately from source HEAD. A later fix needs current-commit regression and
  exact-call evidence; dogfood PASS also requires the actual finalized session
  outputs. Docs/package-version-only continuations remain permitted.

This establishes broader tested behavior, not complete grammar coverage. Julie
still omits some CSS call identifiers and formal facts, can lose a declaration's
Unicode spelling, and does not provide a dialect-safe SQL argument contract.
Curried F# calls and dynamic binding remain conservative. The inventory records
these limits without calling them N/A or presenting an empty result as proof of
safety.

The eight task prompts and separate oracles under `evals/review-2026-10-26/` were subsequently used in the controlled pilot linked below. They are now regression/tuning cases. These earlier implementation checks alone did not establish task-success, time, or token-cost improvement.


**Cross-language Linux verification:** 1,818 Rust tests, 172 Python harness tests,
and 20 plugin tests passed. The release build, workspace Clippy with warnings
denied, formatting, whitespace, and guidance synchronization passed. The final
private helper and lint cleanup passed 391 focused edit tests. The
connected MCP and freshly built CLI post-edit reports were reviewed: 27 intended
shape changes, with no proven or likely stale site outside the diff. Unresolved
name matches and known omissions are not proof of safety.

Fresh MCP processes ran the same 20 calls on the same final worktree and index:
skeleton, lookup, context, callers, and impact, four calls each. Retained RSS
was 23.64 MiB before and 24.46 MiB after; anonymous memory was 13.86 and
14.50 MiB. The measured binaries were `af37420953d7` and `fbde44e013a80`.
These small local measurements do not establish large-repository performance
or an agent-task outcome.

**Cross-language Windows verification:** Source commit
`de67a398db6b1e92d3bb244d6f758fce789abd17` passed all 1,794 Rust tests across
34 suites on NTFS, using the checksum-verified Julie 3.9.1 Windows binary.
The plugin suite passed 12 tests and skipped eight platform-specific fixtures.
The initial run exposed fixture reliance on global Git identity and direct
execution of `npm` instead of Windows `npm.cmd`; both were repaired. The nested
runner fixture now executes on Windows too. Linux executes the exact emitted
POSIX command with `sh`; Windows derives the runner arguments and working
directory from the two supported fixture forms and executes them with
`cmd.exe`. Product output retains its declared POSIX shell syntax.
Windows Clippy with denied warnings, formatting, and the locked release build
also passed. The final evidence commit changes documentation and memory only;
production code remains the verified runtime slice. The built Windows executable
started successfully, and the VM was shut down after verification.


## Controlled comparison follow-up

The [72-run pilot](2026-10-26-agent-comparison.md) is complete. All runs returned answers without scored infrastructure retries. Each arm has the same strict reviewed result, 21/24. All 72 answers identify the requested source facts correctly; nine Go answers retain strict rejection because they list an explicitly excluded alternative in the symbol array. The report keeps that contract ambiguity, 27 mechanical false negatives, and two literal-routing violations visible.

Native tools cost less on several of these tiny fixtures. Revised guidance uses fewer MCP calls than current guidance without improving task success over native tools. This does not establish a global routing default or performance on completed edits. No tool was added or removed, and production routing remains unchanged.

The experiment exposed concrete consumer errors: cross-language impact/dependency/test edges, lost typed-field receiver chains, related types chosen from another language, and unsupported broader runner scope. Agents often recovered from source, so correct final answers do not validate those tool results. The broader runner/scope repair is locally committed as `eb163ea`, graph/context language guards as `a55711c`, and related-type binding as `2033f69`. Typed receiver chains are committed as `e99aa82`, with 25 real-extractor regressions passing. Final integration evidence is recorded in the pilot report. The [retained evidence](../../evals/review-2026-10-26/results/observations.json) preserves every original outcome. Final Linux, local query, and Windows status are recorded in the pilot report.

The completed consumer repair also preserves conservative name-only test labeling and selects compact query shapes per exact caller scope or impact frontier. Linux at `5f998c4` passes 1,889 Rust tests, all 42 languages, Clippy, formatting, and release. The pilot report records the measured query/memory tradeoffs and Windows NTFS verification; the original paid results are unchanged.

Windows NTFS at `f355247` passes 1,865 Rust tests across 37 suites, all 42 language IDs, Clippy, formatting, release, and 12 plugin tests with eight declared POSIX-fixture skips. This completes the approved consumer-repair verification; the next product milestone remains completed edits on untouched, larger repositories across language families.

## Completed-edit preparation

The next evaluation now has six pinned, larger repositories: SQLGlot (Python),
MiniJinja (Rust), HCL (Go), date-fns (TypeScript), Commons Lang (Java), and
Noda Time (C#). Each case requires changes in at least two production files.
These six cases sample the product's 42 supported language IDs; they do not
replace the full language contract.

The [completed-edit study](../../evals/edit-2026-10-27/README.md) preserves a
passing upstream baseline, an intended seeded failure, a passing reference
repair, and a fresh replay of each reference patch. Public and hidden checks
run in separate reconstructed trees. Source review includes requested new
tests and callers. A passing grader result remains pending acceptance until
source review passes. These are local evaluator controls, with zero scored
agent attempts and zero new provider calls.

Patch capture includes ignored and untracked source, deletions, modes and
symlinks independently of Git's index. Protected tests, build configuration,
locks and Git history cannot become part of an accepted patch. Preparation,
agent execution and grading have separate mounts and state. Graders receive
no provider credentials or network; real Codex permission probes verify the
agent shell's private-profile denial without calling a model. The original
72-run pilot and its frozen controls are unchanged.

Preparation exposed an operating-system resource defect: ignored build and
dependency folders still received recursive watches. The watcher now registers
only scanner-eligible directories and refreshes registrations with ignore-rule
and directory changes. Its bounded owner queue reconciles event overflow;
physical registration failures remain unhealthy until registration succeeds.
Startup reconciles changes made before registration. This uses the same scan
policy for every language and retains a directory registry, not a repository
file inventory. AGENTS.md and CLAUDE.md document the behavior together.

The next product decision should follow completed agent edits on this corpus:
compare accepted patches, missed callers, test results, time including cold
indexing, and observed tokens. The frozen comparison proposes 24 scored calls
and at most 28 total calls. Its separate budget gate remains pending; local
readiness does not establish an agent-success or cost improvement and does not
justify changing global tool routing.

The final Linux executable is `d50d77812c0b9aad04b38b31c63b71df7768d0c44273b6d97a7e4a15a37b2127`.
On the same local fixture with 1,000 ignored leaf directories, it uses three
operating-system watches versus 1,016 before the fix. Five samples after a
skeleton call finish at 14,760 KiB retained RSS versus 15,760 KiB before.
On this development worktree, the fixed server uses 212 watches and finishes
that probe at 19,852 KiB RSS. This is a small local resource check, not a
workload-wide latency or agent-performance result.
