# Accuracy base: every reference row accounted for (design)

Date: 2026-10-17. Status: approved direction (owner, 2026-10-17: "Ok let's do it"). Phase 1 of the
accuracy pivot. Evidence: `~/.code-kb/reports/2026-10-17-direction-research/` and
`docs/futures/2026-10-10-viability-findings.md`.

## Problem

code-kb answers "who uses X" with rows that are almost always true, but the lists are short and the
answer does not say so.

- On 12 symbols in flask, express, and gson, code-kb found 135 of 593 true call sites (23%).
  91% of its rows were true.
- `code-kb refs safe_tempdir` says "108 found". rg finds 209 call sites. The index holds the 113
  missing calls as pending rows with namespace `["crate"]` or `["code_kb_core"]`, and
  `pending_target_predicate` rejects them without a notice.
- 21 of 49 sampled "(0 found)" answers in session logs had real call sites.
- The `facts` index level drops call and variable-reference identifiers. So decorator uses
  (`setupmethod`, 0 of 44), functions passed as values (`.map(escape_like)`), and JS calls on an
  untyped receiver (`app.render(...)`, 1 of 313 express sites) are invisible.

An agent cannot trust "N found", so it greps anyway. The index call then adds a turn and saves
nothing.

## Goal

A callers answer that accounts for every row the index holds with the target's name, labels each
row by its evidence, and reconciles with a word-boundary text count. "17 text hits, 17 explained"
lets the agent skip the grep.

## Non-goals in phase 1

- No new tool, and no change to `blast_radius`, `get_symbol_context`, or callees output.
- No override or implementation listing, argument counts, or post-edit check. These are phase 2.
- No compiler index at query time. Compiler indexes (SCIP) are used offline, as ground truth.

## Labels

Each site gets exactly one label and a short basis text.

| Label | Meaning | Basis examples |
|---|---|---|
| proven | Index evidence ties the site to this definition. | same file, qualified path, receiver type, class chain, import, enclosing scope, same package (Go, Java), builds the class |
| likely | Only the name ties them, and nothing points elsewhere. | name only (1 definition), nearest of k definitions, receiver not typed |
| unresolved | The name matches, and the index cannot choose among k definitions. | `get` has 15 definitions |
| excluded | Evidence ties the site to another definition. | resolves to `Other.get` (path:line), receiver `x` is imported from `requests`, receiver `x` has type `Y`, other language |

Text lines are counted apart from sites: a line that holds the word but no indexed row is
"text only" (docs, comments, strings, or code the extractor did not record).

## Algorithm (callers of a selected target)

1. **Target group.** The selected symbol plus its overloads (same name, owner, and file;
   `get_symbol_overloads`).
2. **Resolver rows.** Run the existing `find_direct_references` path (with the constructor-class
   step) with an internal limit of 5,000. Classify each row's basis from its source and fields:
   - relationship row: same file (julie resolved it in the file);
   - pending row: namespace present → qualified path; receiver `self`/`this`/`cls`/`super`/`Self`
     → class chain; receiver equals the owner's name → qualified path; any other receiver →
     receiver type; no receiver and the target is in the caller's file → same file; the target is
     in an enclosing scope or the caller's own class → enclosing scope / class chain; the caller's
     file has an import row for the name → import; Go or Java and same folder → same package;
     otherwise → name only (likely), or nearest of k definitions (likely) when other definitions
     share the name;
   - identifier row: `member_access` (supported) → receiver type; `member_access (candidate)` and
     `handler (candidate)` → likely (or unresolved when k > 1); `type_usage` → same file, import,
     or same package when true, otherwise likely;
   - grouped member-access row (receiver names the type) → qualified path.
3. **Name sites.** Every row with the target's name: `pending_relationships` by
   `target_terminal_name`, `identifiers` by `name` (all kinds the level keeps), and
   `relationships` to another symbol with the name. Drop import rows and identifier rows at a
   definition of a symbol with that name (the definition is not a use). A site is `(path, line)`;
   rows on one line merge.
4. **Accounting.** For each name site that step 2 did not return:
   - relationship row to another definition D → excluded (resolves to D);
   - another definition D with that name (not in the group; at most 12, nearest first) returns the
     site from its own resolver run with a proven basis → excluded (resolves to D);
   - receiver bound to an import whose source is not an indexed file → excluded (imported from S);
   - receiver with a type fact naming a type that is not the target's owner → excluded;
   - language outside the target's family (`identifier_language_matches`) → excluded (other
     language);
   - only the target group defines the name → likely (name only);
   - otherwise → unresolved (k definitions; up to 3 named).
   Every name site lands in exactly one label. Nothing is dropped.
5. **Exact lines.** A pending row stores the statement's first line in JS, TS, Java, C#, Rust and
   others. When a call identifier with the same name and caller sits inside the row's span, the
   site uses the identifier's line.
6. **Text check.** Scan the project for the name as a whole word (ASCII letters, digits, `_`, `$`
   are word characters), with `.gitignore`, `.julieignore`, and code-kb's excluded folder names
   applied, nested repositories skipped, binary files and files over 2 MB skipped, and a 3 s
   budget. Each hit line is explained by a site (any label) whose span covers it, a definition of
   a symbol with the name, or an import row; the rest are text only. For a constructor target the
   text check is skipped with a note, because its call sites do not spell its name.

## Output

Text (MCP and CLI):

```
References to method `Config.from_mapping` (src/flask/config.py:304)
Sites: 8 · proven 1 · likely 7 · unresolved 0 · excluded 0
Text: `from_mapping` is on 17 lines: 8 sites, 1 definition, 0 imports, 8 text only.

Proven (1):
- `Config.from_file` [src/flask/config.py:302] (kind: calls; same file)
Likely (7): name only; `from_mapping` has 1 definition
- `test_config_from_mapping` [tests/test_config.py:112] (kind: calls)
…
Text only (8): docs, comments, strings, or code the index did not record
  docs/tutorial/factory.rst: 46, 56, 85
  …
Not visible to this index: calls through getattr() or names built at runtime.
```

- Counts are always complete. `limit` (default 20, max 200) caps the listed site rows, in label
  order. Rows past the cap fold into one line per label with per-file counts. Text-only lines fold
  per file.
- CLI `--json` for callers prints a report object: `target`, `counts`, `sites` (each a
  `ReferenceSite` plus `label`, `basis`, `end_line`), `text_only` (`path`, `lines`), `text_scan`
  (`files_scanned`, `complete`, `elapsed_ms`), `cannot_see`. Callees keep the array.
- A text scan that ran out of budget says so and names the scanned share.

## Index level

code-kb builds new indexes at the `full` level instead of `facts`. The `facts` level computes call
and variable-reference identifiers and then drops them; `full` keeps them, plus decorator
annotations, comment and string regions, and parameter counts (phase 2 uses these).

Measured on aspnetcore (17,334 files): `facts` 32.6 s, 2.0 GB, extractor peak 2.8 GB; `full` 53.5 s,
3.7 GB, extractor peak 4.7 GB. The growth is call identifiers (507k) and variable references (889k).
A trimmed level would save under 10%, so it is not built now. A later julie level that drops
references to local variables is the follow-up if size matters.

The version guard already rebuilds an index recorded at another level, so existing `facts` indexes
rebuild once.

## Resolver fix: Rust path-qualified calls

`pending_target_predicate` rejects namespaces `crate`, `self`, `super`, and crate names. New rule for
a top-level Rust target:

- `crate::f`, `self::f`, `super::f`: the target is in the same crate as the caller (same package
  folder and both under `src/`, or the same file);
- `<crate_name>::f`: the target's package folder name, with `-` read as `_`, equals the crate name
  and the target is under `src/`;
- a module segment matches a file stem or a folder name (`/value.` or `/value/`), with `-` read as
  `_`.

## Ground-truth gate

New offline gate, `scripts/reach_gate.py`, with cases in `evals/reach/`.

- Corpus: flask (Python), express (JS), zod (TS), gson (Java), cobra (Go), Newtonsoft.Json (C#),
  code-kb at 765d940 (Rust). Each repo is pinned by URL and commit and materialized in
  `~/.cache/code-kb-reach/`.
- Truth: SCIP reference occurrences from scip-python, scip-typescript, scip-java, scip-go,
  scip-dotnet, and rust-analyzer, minus import and `use` lines, with overloads unioned.
  Express methods assigned as properties have no SCIP symbol, so their truth is hand-checked.
  Where SCIP and code-kb disagree, an adjudication (true or false, with a note) is stored in the
  case file. `scripts/reach_truth.py` rebuilds truth; the gate needs only code-kb and the cases.
- Symbols: 30 per repo, sampled with a fixed seed and stratified by shape (common name, overridden,
  decorator, many callers, functions passed as values), split into dev and holdout halves. Fixes
  are tuned on dev only.
- Scores per language and split: proven precision; proven+likely recall; accounted share (truth
  lines that appear in any label or as text only). A truth line inside a site's span counts.
- Pass marks: proven precision at least 0.98 in every language and accounted share at least 0.98.
  Recall target: proven+likely at least 0.90 overall (reported per language; a hard mark once the
  baseline is known).
- `release-preflight.sh` requires a fresh PASS report, like the corpus check.

## Testing

- Unit and fixture tests for each basis, each accounting rule, the Rust path rule, the text scan
  (word boundaries, ignore rules, nested repos, budget), and the report formatter.
- The existing reference tests keep their meaning: rows they expect stay listed (now labeled).
- Gate baseline (v2.6.0) and candidate reports, side by side.
- Latency: refs p50/p95 on aspnetcore and hermes-agent before and after. Memory: live `serve` RSS
  before and after the level change.

## Risks

- Common names flood likely and unresolved rows (`get`, 385 calls). Counts stay complete; rows
  fold per file.
- The per-definition exclusion runs cost time on names with many definitions. Cap at 12 and a
  time budget; past the cap, sites stay unresolved with a note.
- A larger index rebuild on upgrade: one time, about 1.6x scan time.
- The text scan on large repos: parallel walk, 3 s budget, stated when cut.

## Phase 2 (after this phase's gate report)

The reach answer (overrides and implementations, constructions, match sites, registrations, direct
tests with commands), call-site argument counts in julie-extractors, the post-edit check pushed by
a hook, and tool removals (`blast_radius` word-guess tests, `search_symbols`,
`find_structural_facts`, `codebase_outline`, `telemetry_summary` from MCP).
