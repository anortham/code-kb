# code-kb evaluation, September 26, 2026

code-kb has a useful, well-defined core. Its strongest case is helping an agent
locate and inspect a small part of a large codebase. The evidence supports large
reductions in individual retrieval responses compared with full-file reads. It
does not establish an 80-90% reduction in total task tokens, billed usage, or
context compactions.

Keep the Rust, julie-extract, SQLite, and CLI/MCP foundation. Prioritize task
measurement, bounded answers, and retrieval reliability before adding a vector
runtime or a broader agent framework.

This evaluation inspected code-kb at `54f2c08852ec81fe063cb833bc8d3a87805af2ef`
on `main`, initially clean, with no other worktrees. It also inspected clean
main checkouts of io `620f5f4`, Phoebe `4073d30`, Miller `3ed3920b`, and Julie
`4c801c1e`. Existing experiments below are historical evidence on their stated
snapshots. Fresh probes used the local `code-kb 2.2.2` executable, SHA-256
`e27a9ac693034d8c54e8155c1dbb0efe332cdf58e2660d33c30c48add8c1b043`.
The executable version alone does not identify a source commit.

The [measurement data](2026-09-26-evaluation-data.json) records the fresh probes,
binary identity, telemetry cutoff, and existing benchmark results. This is an
evaluation and recommendation list; the proposed product changes are not
implemented by this report.

## Current state

- The product boundary is coherent: ten MCP tools, matching CLI commands,
  per-project SQLite indexes, explicit project roots, and native agent editing.
  Exact symbol IDs, qualified names, freshness checks, safe extractor upgrades,
  and Windows support address practical sources of wrong answers.
- Verification extends beyond unit tests. The 2.2.2 release records a comparison
  on 15 real projects plus extractor fixtures, and Claude/Codex dogfood sessions
  that found and fixed incorrect answers. Those release results predate today's
  owner/member search expansion. See [release evidence](../release-notes/v2.2.2.md).
- Search handles identifiers well. Concept queries use lexical matches in names,
  signatures, documentation, and owner context. Discovery remains limited by
  which candidates enter the search. Today's expansion produced no top-1 gains in its
  66-query experiment, its confidence intervals include no improvement, and the
  report records 12 of 36 evaluation targets absent from the candidate pool.
  The change already exists; it should not be proposed as a new adoption from
  Phoebe. See [the search experiment](2026-09-26-builtin-search-comparison.md).
- References and impact prediction contain useful inference, with known
  ambiguous receivers and missed runtime paths. The release notes explicitly
  record both false matches and missed possible tests. Their large-project
  measurements also include 18-second reference and 10.7-second impact calls.
  These are release observations on hermes-agent, not fresh measurements here.
  The README's unconditional sub-5ms description is too broad.
- Maintainability is concentrated in `queries.rs`, now 8,726 lines. Its size alone
  does not justify a rewrite. Separate reference resolution, search ranking, and
  impact traversal when changing those responsibilities, retaining their current
  behavior tests.

## Five improvements, in priority order

1. **Measure complete tasks and correct the savings claims.** Reuse the existing
   retrieval scripts and the Miller comparison protocol. Compare competent native
   retrieval, code-kb under current guidance, and code-kb with explicit native
   verification/fallback. Use identical repository snapshots, prompts, model,
   effort, and budgets in independent fresh sessions. Include exact symbols,
   literals, unfamiliar concepts, ambiguous callers, and edits requiring module
   context. Repeat and randomize or alternate arm order. Score correctness and
   wrong actions before comparing usage; report all-task outcomes and efficiency
   on tasks both arms solve. Capture actual model input/output/cache usage, tool
   responses, setup overhead, follow-up reads, wall time, and compactions where
   the client exposes them. Separate cold indexing from warm use. Use
   product-neutral acceptance criteria, not requirements for a code-kb-issued ID.
   Keep code-kb first for structure; test native verification for empty, capped,
   heuristic, conflicting, or incomplete answers, while measuring adoption.
   **Acceptance:** a repeated comparison on held-out tasks with correctness,
   usage distributions, and explicit unavailable measurements. Until then,
   describe existing savings as full-file output compression and correct the
   unsupported README, skill, and routing claims.

2. **Make relationship inference easier to audit and faster on difficult cases.**
   Existing `candidate`, `possible`, and traversal-cap notices are a good start.
   Preserve the reason a relationship matched through resolved identity,
   inferred type, receiver/name matching, fixture propagation, or runtime
   prediction. Expose that reason where it changes confidence in a result.
   Use the known receiver ambiguities and missed tests as concrete expected
   edges and non-edges. Diagnose the recorded common-name latency cases with
   query plans before adding more heuristics. Extract shared resolution rules
   from the large query module as this work requires it.
   **Acceptance:** measured precision and recall plus p95 on fixed difficult
   cases, with no output/latency improvement achieved by silently dropping edges.

3. **Bound skeleton and context output, with a way to continue.** A skeleton of
   `queries.rs` returned 14,541 reference tokens in the fresh probe. The skeleton
   schema accepts only a file path, and its formatter renders every symbol.
   Context limits supporting rows but includes the entire target body. Existing
   search/reference result limits and truncation notices are useful and should
   be retained. Add an output budget or bounded view with explicit omitted counts
   and continuation to these unbounded paths. Preserve an explicit complete-body
   operation, so a budget never silently returns partial code as a full body.
   Borrow Miller's budget discipline without adding its full task-planning layer.
   **Acceptance:** a large-file fixture respects the budget, reports omissions,
   and permits retrieval of every omitted symbol or body segment.

4. **Make answer quality a repeatable gate.** Extend the existing search corpus,
   adversarial tests, and release dogfood assets rather than building another
   evaluation framework. Add fresh, held-out cases for candidate omissions,
   wrong-target references, missing tests, ambiguous names, and concept queries
   across languages. Score candidate recall separately from ranking; score both
   expected and forbidden relationship matches. A corpus comparison that finds
   changed output and a timing script cannot detect every confidently wrong or
   incomplete answer. Maintain separate tuning and held-out data, refresh stale
   labels against pinned snapshots, and preserve sealed acceptance data.
   **Acceptance:** retrieval changes show correctness and cost on the same fixed
   cases, including misses and negative examples. These cheap deterministic
   checks complement the agent-task experiment in item 1.

5. **Test a separate file/content retrieval path before more ranking tweaks.**
   Phoebe's most useful distinct result is ranking whole files alongside
   definitions. Miller demonstrates fresh, bounded source chunks in SQLite FTS.
   A narrow implementation could admit files using implementation text, comments,
   and documentation when symbol names and docstrings miss the query. First test
   those real misses against targeted native search. Keep file retrieval distinct
   from exact symbol ranking: Phoebe's file fusion loses symbol top-1 hits. Preserve
   AST extraction and current freshness checks.
   **Acceptance:** gains on an untouched, file-oriented task set over both current
   code-kb and native retrieval, with indexing, disk, memory, latency, and output
   costs reported. Do not ship it on a development-set ranking gain alone.

Item 1 largely executes the existing
[retrieval-value backlog](../plans/2026-09-23-retrieval-value-and-optional-routing.md).
It does not need another planning system. Paid agent replays require a budget;
this evaluation did not run them.

## What the token measurements establish

The fresh sample used `tiktoken 0.14.0`, encoding `cl100k_base`, to count response
text and source text. This is a reproducible reference encoding, not a claim
about the exact tokenizer of the current agent model. Four skeletons and four
bodies were selected from different-sized files; this is a diagnostic sample,
not a representative task benchmark.

| Retrieval | Full source file | code-kb response | Reduction against full file |
|---|---:|---:|---:|
| `slicer.rs` skeleton, 191 lines | 1,613 | 623 | 61.4% |
| `telemetry.rs` skeleton, 2,214 lines | 18,481 | 2,594 | 86.0% |
| `queries.rs` skeleton, 8,726 lines | 81,114 | 14,541 | 82.1% |
| `server.rs` skeleton, 1,750 lines | 14,928 | 1,485 | 90.1% |
| `resolve_telemetry_dir` body | 18,481 | 219 | 98.8% |
| `handle_call_tool` body | 14,928 | 855 | 94.3% |

The corresponding exact source excerpts are 197 and 835 tokens for the last two
rows. Adding five lines on either side makes them 286 and 889 tokens. Across all
four body samples, code-kb is 2.4-11.2% larger than the exact excerpt and 3.8-32.3%
smaller than the excerpt with that padding. Those comparisons assume the source
location is already known. They exclude the cost and mistakes of finding it, and
therefore are not native-agent results. They show why a full-file baseline cannot
establish code-kb's advantage over competent targeted retrieval.

The two context probes returned 321 and 261 tokens and include evidence beyond
the body, so an identical source excerpt is not an equivalent answer. Combining
useful evidence and locating it are parts of code-kb's value that byte compression
alone misses.

Telemetry computes returned tokens as bytes divided by four. Its baseline is the
full size of the named file, or up to 20 distinct files named by a result. Savings
are clamped at zero and added per call, hiding negative differences when an
answer exceeds its full-file baseline. It does not subtract tool registration,
routing instructions, extra model turns, retries, or later reads, and it can
credit the same file again on another call. It does not observe what an agent
without code-kb would have read. See
[the accounting](/home/murphy/source/code-kb/crates/code-kb-cli/src/mcp/server.rs:569) and
[file baseline calculation](/home/murphy/source/code-kb/crates/code-kb-core/src/queries.rs:371).

Before this evaluation's calls, version 2.2.2 had 90 recorded calls, roughly
70,539 served tokens, and 4,373,600 estimated tokens saved against that baseline,
known for 83 of 90 calls. There was one recorded error. These calls mix workloads,
including dogfood; success means the tool did not report an error, not that an
agent produced a correct answer. The current telemetry heading appropriately
says "upper bound". The README and routing/skill text still overstate what this
measures. Historical versions also used different accounting, so their totals
should not be pooled into an efficiency claim.

There is overhead too. The ten raw `tools/list` definitions serialize to 2,275
reference tokens and the routing block to 617. Clients expose, defer, duplicate,
or cache these differently, so their sum is not a measured per-session bill or
context cost. Cached input, context occupancy, and compaction counts also need
separate measurement. The bytes/4 estimate is not uniformly accurate either: the
855-token `handle_call_tool` response is recorded as about 1,200 tokens.

The existing `scripts/retrieval_score.py --json` ran on its pinned corpus at
`8e1b388`, using a private telemetry database. Twelve scripted tasks make 15
calls per pass; three measured passes produced p50 2 ms and p95 19 ms. It reported
11,823 estimated response tokens per pass and the expected missing-file error.
The script explicitly does not score answer quality. It is a useful tool-cost
regression check and is not the missing agent comparison.

My judgment is that code-kb saves substantial context when it prevents a large
file read or a broad search dump. That makes the observed reduction in compaction
frequency plausible. The benefit can shrink or reverse when native retrieval
already has the right location, when a file is small, or when tool chaining leads
to a full read anyway. No reliable overall percentage, billing saving, or
compaction reduction follows from the evidence inspected here.

## What to take from the other projects

| Project | Evidence | Recommendation |
|---|---|---|
| Phoebe | Optional file fusion has MRR 0.658 versus code-kb 0.563 on an 82-query Miller development set. It also loses 5-7 symbol top-1 hits. | Test separate file retrieval. Keep AST extraction; Phoebe's regex implementation required fixes for 33 defects. |
| io | On 32 valid code-kb queries, code-kb had 22 symbol top-1 hits, io semantic 19, and io hybrid 17. io indexed code-kb in 12m46s with GNU time reporting 454 MB maximum RSS; this is not a summed process-tree peak. The sidecar used roughly 400 MB. | Keep embeddings out of the default product for now. This implementation's cost is not justified by these results; it does not rule out every embedding approach. |
| Miller | Its paired agent study found more completed tasks with Miller but more tool-output tokens on tasks both arms solved. It has explicit context budgets and revision-checked source-text chunks. | Reuse evaluation discipline, budgets, and freshness rules. Do not port the entire tool suite or assume richer retrieval lowers total tokens. |
| Julie | A 23-case expected-file scorecard reports semantic MRR 0.848 versus lexical 0.351. Its synthetic token-saving test uses assumed field lengths. | Keep a small retrieval-quality scorecard. These results do not establish embedding value or task savings for code-kb. |

MRR measures how highly the first accepted answer ranks; higher is better.
File ranking, symbol ranking, and successful task completion are different
outcomes. The numbers above should not be combined into one league table.

Sources: [Phoebe results](/home/murphy/source/phoebe/docs/results.md:125),
[io results](/home/murphy/source/io/docs/results.md:31),
[Miller calibration](/home/murphy/source/miller/docs/findings/2026-08-25-miller-vs-bare-agent-v1.22.1-calibration.md:49),
[Miller source freshness](/home/murphy/source/miller/src/Miller.Indexing/ContentCorpusContextReader.cs:14),
[Julie scorecard](/home/murphy/source/julie/docs/eval/semantic-value/results/2026-09-11T14-17-58Z.md:3).

Miller's often-quoted 11/15 versus 5/15 completed-task result includes five tasks
requiring product-issued symbol identities that the bare arm could not satisfy.
Its tool-output efficiency medians of 4,468 versus 1,062 cover only tasks both
arms solved. Its own repeated controls also showed substantial variation.
Borrow its controls and honest failure reporting, while removing that acceptance
bias from a code-kb experiment. Phoebe used development data, some io comparison
queries had already tuned Phoebe, and Julie's sample is curated and small.
None of these projects establishes code-kb's actual compaction savings.
