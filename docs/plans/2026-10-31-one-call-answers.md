# One-call answers (code-kb 2.12.0, julie 3.11.0)

The user asked on 2026-10-31 for an agent's evaluation of the toolkit and a plan. This
plan records the evaluation and the work. The julie side is in
`~/source/julie-extractors/docs/plans/2026-10-31-annotations-and-write-sites.md` and
runs in a separate session in parallel.

## Evaluation

Evidence: the seven tool schemas from a live `tools/list` (about 2,000 tokens), live
calls on this repository at 1fc6a1a, the index tables and row counts, julie's schema v8
and data contracts, `docs/futures/2026-10-10-viability-findings.md`, the completed-edit
study in `evals/edit-2026-10-27/results/`, and plan 018.

### Small answers and many calls, or one complete answer

The cost model decides it:

- A turn re-sends the fixed prompt from cache, about 50k tokens and a few seconds. A
  tool answer is 1k to 8k tokens. Read and search output is 11-23% of peak context.
- In the edit study the code-kb arm made 18.8 MCP calls per trial, ran 181 shell
  commands against 165 native, used more tokens, and took 67 s longer. Completeness did
  not remove calls yet.

One complete answer under the 8K budget beats two small answers when the agent does
not have to make the second call anyway. The answers are complete per tool, not per
agent intention, so the chain stays long:

| Intention | Calls today | Target |
| --- | --- | --- |
| Orient in a folder or file | `file_skeleton` | 1 |
| Locate a name or concept | `lookup_symbol`, then `get_symbol_body` | 1 for a short unique match |
| Prepare an edit of one symbol | `lookup_symbol`, `get_symbol_context`, `find_references`, `blast_radius` | 1, with drill-downs |
| Check an edit | `check_edit` | 1 (already) |

The failure mode of a complete answer is answering what was not asked (35 proven sites
and 186 text-only lines when the question was "who calls this and which tests"). The
fix is a fixed shape, not a smaller answer: one summary line that always carries the
decision, then capped detail sections, then drill-down hints. `find_references` does
this with its `Sites:` line; every tool should.

### What works

- `file_skeleton` on a folder: one line per file with top symbols and a test count.
- The proven, likely, unresolved, excluded labels with a basis per site. No LSP says
  why it believes a site.
- `blast_radius` "Open Ends" and `check_edit`: no native substitute.

### What costs turns

- The routing text says to call `file_skeleton` before every Read. When the agent will
  edit the file, Claude Code requires a Read anyway. The study arm called
  `file_skeleton` 11 times and still read files.
- `lookup_symbol` then `get_symbol_body` is two turns for a known name.
- `get_symbol_context` lacks callers and run commands, so two more calls follow it.
  `blast_radius` was the most-called tool in the study (21 of 91 counted calls).

### Position against LSPs

Claude Code ships LSP tools. Where a server exists, it is exact. code-kb's space is
impact and tests, cross-language edges (QML to C++, Razor to C#, JS to TS), 42
languages with no server install, mixed repositories, and harnesses with no LSP
(Codex sends 59% of navigation to code-kb, Claude 3-8%). The README and the skill
should lead with "evidence-labelled impact across languages", not "definitions and
references".

### Julie data not yet used

Row counts on this repository's index at the `references` level:

| Data | Rows | Read today | Fill |
| --- | --- | --- | --- |
| `symbols.visibility` | 1,293 public / 4,334 private functions | ranking | public-surface outline |
| `structural_facts` | 151,003 | CLI `facts` only | routes and config keys in `lookup_symbol` |
| `type_facts` on functions | 1,724 | receiver proof | return types in context |
| `language_capability_gaps` | 21 | never | honest coverage line |
| `symbol_annotations` | 0 at this level | never | sibling conventions (julie 3.11.0) |
| `identifiers.variable_ref` | 93,880 | callers report | read/write split (julie 3.11.0) |
| `literals`, `complexity_metrics`, `source_regions`, `type_arguments` | 65,039 / 0 / 0 / 0 | CLI `facts` / never | leave |

The sibling-conventions idea comes from the study: both native failures were a
missing `[Pure]` attribute on a new NodaTime method; the code-kb arm passed because the
body showed the attribute.

Deferred for good, measured in the viability spikes: semantic search, a lexical
full-text index, edit tools, dashboards.

## Tasks

Base: `main` at 1fc6a1a or later. One worktree per task under `.worktrees/<task>`,
branch `feat/<task>`. Tasks 1-3 change answers that the dogfood round and the corpus
check compare, so each one records its expected output change in its PR text.

### 1. `edit-prep`: one-call `get_symbol_context`

Owner files: `ops.rs` (`get_context_slice_op`), `models.rs` (`ContextSlice`),
`formatters.rs` (`format_context_slice`), `queries.rs` where a shared query is needed.

- Return types: print `Returns: <type>` from `type_facts` on the target when the
  signature does not spell it, and on each callee line after the signature.
- Add to `ContextSlice`: the callers summary of the target (the `Sites:` counts, the
  first `proven` and `likely` sites up to a cap, grouped like `find_references`), the
  tests with run commands (the `Tests that call or use it` section of the callers
  report, which `test_commands.rs` already computes), and the open ends of a depth-2
  walk. Read them through `reference_report` and the walk, from the one index snapshot
  the context already takes.
- New section order: Target, Signature and return type, body, `### Callers` (summary
  line first), `### Tests to run`, `### Dependencies`, `### Types`, `### Open Ends`.
  `### Related Tests` folds into `### Tests to run`.
- Budget: `fit_context_slices_to_budget` shares the 8K budget; the body yields first,
  then callers detail, never the summary lines. A drill-down hint line names
  `find_references` and `blast_radius` with the exact arguments.
- Time: the callers report in context keeps the 3 s limit; an unchecked count prints
  as in `find_references`.
- CLI `context` prints the same text; `--json` adds `callers`, `tests`, `open_ends`.
- Tests: a context for a target with cross-file callers lists the summary and the run
  commands; a target with no caller says so in one line; the budget cut keeps the
  summary lines; language conformance fixtures gain the section for every language
  with calls.

### 2. `routing-text`: skeleton to orient, Read to edit

Owner files: `crates/code-kb-cli/src/routing-block.md`, `hooks/code-kb-routing-block.md`,
`skills/code-kb/SKILL.md`, the server instructions string in `mcp/server.rs`, the hook
tests, README.

- Replace "Before you Read a source file, call `file_skeleton`" with three lines:
  `file_skeleton` on a folder or a file you do not know yet; `get_symbol_context` for
  the symbol you will change (it carries body, callers, tests, and types); Read the
  file you will edit, since the editor needs it. Keep the `rg` for literal text line.
- Keep the block under its current token count.

### 3. `lookup-body`: body with a unique short lookup

Owner files: `ops.rs` (`lookup_symbols_op`), `formatters.rs`.

- When a lookup names exactly one definition (one exact-name row of a definition kind,
  imports folded) and its body fits in a quarter of `OUTPUT_BUDGET_CHARS`, the answer
  appends the body under the row, as `get_symbol_body` prints it. Overloads, members
  listings, and misses are unchanged. CLI `lookup --json` adds `body`.
- Tests: one definition under the cap prints the body; one over the cap prints the
  row and the `get_symbol_body` hint; two definitions print rows only.

### 4. `small-fills`: three data fills, one release

The fourth fill, return types in `get_symbol_context`, moved to task 1, which owns that
answer, so tasks 1 and 4 can run in parallel.

Owner files: `queries.rs`, `formatters.rs`, `ops.rs`, `mcp/server.rs` (one new optional
parameter), `docs/language-support.md`.

- Public-surface outline: `file_skeleton` takes `public_only` (CLI `--public-only`).
  For a folder it keeps rows whose `visibility` is `public` (or `pub(crate)` where the
  language has it, documented per language) and counts the hidden rows per file; for a
  file it hides private rows the same way. Languages with no visibility fact (Python
  by name convention, JavaScript by export) use the rule `docs/language-support.md`
  records for them.
- Routes and config keys in `lookup_symbol`: a fifth recall branch over
  `structural_facts` for pattern families `route` and `config` (the `CATEGORY_ALIASES`
  families), matched on `json_extract(metadata_json, '$.normalized_route_template')`,
  `'$.key_path'`, and `'$.key'`. A row prints as `route \`GET /users/{id}\`
  (path:line) -> handler` or `config \`mcp_servers.code-kb.command\` (path:line)`. The
  branch runs only when a query holds `/`, `.`, `{`, or `:`; it adds no row to a
  plain-word query, so search ranking tests stay as they are.
- Coverage lines: when a `find_references` or `blast_radius` target is in a language
  with an `exception` gap for `identifiers` or `pending_relationships`
  (`language_capability_gaps`), one line says the index records no sites for that
  language, before the empty sections.

### 5. `julie-3.11` (after the julie release)

Owner files: `scripts/julie-pins.json`, `sync.rs`, `queries.rs`, `formatters.rs`,
`reference_report.rs`, `docs/language-support.md`, CLAUDE.md/AGENTS.md.

- Pin 3.11.0 with checksums. The version guard rebuilds every index, since the level
  content changes.
- Sibling conventions in `get_symbol_context`: a `### Conventions` section that lists
  the annotations (`symbol_annotations.annotation`) that at least two thirds of the
  target's siblings (same parent, same kind) carry, and whether the target carries
  each. Nothing prints for a target with fewer than two siblings or no shared marker.
- Read/write split: `find_references` for a field, property, variable, or constant
  groups sites by `identifiers.metadata_json.access` (`write`, `read_write`, `read`,
  then unclassified), with a count per group in the `Sites:` line. `check_edit` lists
  writes first for a removed or retyped field. Languages without the fact keep the
  single list.

## Measurement

Run after tasks 1-3 land in one branch, before the release:

1. The six study cases in `evals/edit-2026-10-27/` with the same harness, two repeats,
   code-kb arm only against the frozen 2.11.x traces. Pass mark: at least 20% fewer
   MCP calls per trial, acceptance 12/12, and total input tokens not above the 2.11.x
   arm. A fail sends task 1 back to design, not to the release.
2. Reach gate alone, corpus check, dogfood round, as `docs/RELEASING.md` says.

## Release order

1. Tasks 1-4 to `main` through PRs; measurement; release code-kb 2.12.0.
2. julie 3.11.0 release (separate session, its own plan).
3. Task 5; release code-kb 2.13.0.

## Effort

For an agent: tasks 1-3 one session each, task 4 one session, the measurement one
session, task 5 one session after the julie release. Human time: the measurement
verdict, the pin bump approval, and the three release approvals.
