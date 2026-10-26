# code-kb project direction review

Reviewed on 2026-10-26, starting from `main` at `d1ab7d9` (v2.9.1), with
julie-extract 3.9.1. The initial checkout was clean and had one worktree.

**Recommendation:** keep the smaller tool surface and the accuracy direction.
Make the next milestone about agents completing correct edits with less rework.
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

1. **Establish a fresh outcome baseline alongside the next feature slice.**
   Reuse the [existing comparison plan](../plans/2026-09-23-retrieval-value-and-optional-routing.md),
   updating its obsolete tool names and versions. Compare native tools with
   native tools plus the current code-kb under the same model, harness, tasks,
   and acceptance checks. Include concept discovery, a multi-file signature
   edit, an ambiguous caller, a literal/configuration task, and a framework
   test path. Reserve new tasks before tuning and repeat runs. Measure accepted
   changes, missed required edits, repair rounds, wall time including cold
   indexing, and observed tokens. Record tool adoption separately: enforced
   calls are not proof of value. Paid runs require a separate budget approval.

2. **Trial a `check_edit` extension where real edits escape it.** My preferred
   first feature slice is removed class/instance attributes, followed by
   functions passed as values if the task baseline supports that priority. The
   [v2.9.1 dogfood](2026-10-22-v2.9.1-dogfood.md) records concrete examples:
   `Request.routing_exception`, `App.subdomain_matching`,
   `Flask.request_class`, and `_async_wrapper=self.ensure_sync`. These are
   candidate capability additions to the existing shape-check contract, not
   evidence that every body change should produce a warning. Acceptance should
   include newly caught required edits, unchanged valid edits, false blocks,
   and latency on fresh cases. Keep blocking limited to evidence strong enough
   to justify it.

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
   ancestor PASS does not establish that later semantic changes were checked. Allow docs/version-only
   changes without requiring an expensive rerun.

**Defer:** more general MCP tools, native editing/test execution inside code-kb,
an embeddings runtime, compiler-scale resolution, and a broad architecture
rewrite. Each needs a demonstrated failed workflow and a measurable improvement
over the existing fallback. Stronger edit checking with clear coverage and
recovery is the proposed next product slice; fresh agent-task results should
determine whether it earns that investment ahead of retrieval improvements.

The review covers recent changes, tool contracts and plugin guidance, index
lifecycle, shared query architecture, evaluation/release evidence, and local
Linux verification. The repository's CI matrix also covers macOS and Windows;
those platforms were not rerun locally for this review.

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

**Final verification:** 1,663 Rust tests, 149 Python harness tests, and 20 Node
plugin tests passed. The release build, workspace Clippy with warnings denied,
format check, whitespace check, and AGENTS/CLAUDE synchronization check passed.
`check_edit` identified the two expected shape changes: new reconciliation error
variants and the watcher's private failure state. The error uses were reviewed
and compiled; no missed required edit was found. Its unresolved sites and known
coverage omissions were not treated as proof of safety. No paid agent comparison,
push, release, or Windows/macOS test run was performed.
