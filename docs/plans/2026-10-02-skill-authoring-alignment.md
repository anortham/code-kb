# Skill Authoring Alignment

Date: 2026-10-02. Status: done on branch `docs/skill-authoring`. See "Outcome" at the end.

Sources:

- Skill authoring best practices: https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices
- Claude Code skills reference: https://code.claude.com/docs/en/skills

Scope: `skills/code-kb`, `skills/telemetry`, `skills/report-issue`, and the copy in
`.claude-plugin/skills/`.

## What already meets the guidance

- Every `name` is lowercase with hyphens, under 64 characters, and has no reserved word.
- Every `description` is under 1,024 characters (215, 290, and 218) and has no XML tag.
- Every body is under 500 lines (105, 33, and 33).
- No skill links to a second file, so no reference is nested.
- `report-issue` has a clear step list and a review loop: show the file, edit it, show it again until approved.
- `telemetry` and `report-issue` end with a check list (`It's working if`).
- All paths use forward slashes.

## Findings

### 1. The `code-kb` description says when, not what (high)

The guide says a description states what the skill does and when to use it, with the key
use case first. The current text starts with `Use when` and never says what code-kb is.
It also claims `checking tool telemetry and token savings`, which the `telemetry` skill
also claims. Two skills then compete for one request.

Proposed text:

```yaml
description: Navigates code through the code-kb MCP server - symbol lookup, concept search, file skeletons, symbol bodies, callers and callees, and test impact - with far fewer tokens than grep or full-file reads. Use when exploring unfamiliar code, finding a symbol or its signature, preparing to edit a function, or checking what a change affects.
```

### 2. The `code-kb` body repeats text that is already in context (high)

The guide says to add only what Claude does not already know, and to test each paragraph
against its token cost. In a Claude Code session the agent already has:

- the routing block from the `SessionStart` hook (`hooks/code-kb-routing-block.md`, 318 words),
- the MCP server instructions,
- every tool schema, with parameter descriptions.

The skill (about 1,500 words) repeats most of that. Cuts:

- `Telemetry & Diagnostics`: the `telemetry` and `report-issue` skills own this. Replace it with one line that names them.
- `Parameter Aliases & Safe Defaults`: the section itself says to use canonical names. The alias list only gives the agent more options to pick wrong ones. Keep only the defaults that change a call: `direction` defaults to callers, `include_external` defaults to false.
- `Core Invariants`: `Zero Heap Footprint` and `Self-Cleaning Workspaces` describe the server internals. The agent does not act on them. Keep `Disambiguation`.
- `Quick Reference`: the table repeats the four phases. Keep the table or the phases, not both. The table is denser and keeps the CLI twins, so keep the table.

Target: under 50 lines and under 700 words.

### 3. The `code-kb` body has figures that go stale (medium)

The guide says to leave out information that becomes wrong over time.

- `retained memory is about 25 MB`: changes with each release. Remove it (see finding 2).
- `save 80–90% of token consumption`: the telemetry gives a measured figure. Say "far fewer tokens" or point at `/telemetry`.

### 4. MCP tool names are not qualified (medium)

The guide says to name an MCP tool with its server, because a bare name can fail when
several servers are present. The skill names `lookup_symbol` and the other tools bare.
The full name depends on the install:

- Claude Code plugin: `mcp__plugin_code-kb_code-kb__lookup_symbol`
- Claude Code manual registration: `mcp__code-kb__lookup_symbol`
- Codex and other harnesses: their own prefixes.

Proposed fix: one sentence near the top: "The tools below come from the `code-kb` MCP
server; the host may show them with a prefix such as `mcp__code-kb__` or
`mcp__plugin_code-kb_code-kb__`." This keeps the bare names short in the body and
removes the ambiguity once.

### 5. `telemetry` is a generic name (low)

The guide lists generic names (`data`, `files`, `tools`) as names to avoid. In the plugin
the command is `/code-kb:telemetry`, which is clear. On a machine with symlinked skills
(this dev machine), the bare `/telemetry` has no product in its name and can collide with
another skill. Option: rename to `code-kb-telemetry`. This changes the slash command, so it
needs the user's decision.

### 6. `telemetry` and `report-issue` do not use the Claude Code argument fields (low)

The `telemetry` description spends a sentence on its arguments. Claude Code now has
`argument-hint` (shown in autocomplete) and `$ARGUMENTS` substitution.

```yaml
argument-hint: "[today|7d|30d|month|year|all] [workspace]"
```

Then step 1 reads the arguments from `$ARGUMENTS`, and the description loses the argument
sentence. `report-issue` can take `argument-hint: "[what went wrong]"`.

Risk: the same `skills/` directory ships to Codex (`.codex-plugin/plugin.json`). Claude
Code accepts these fields. Verify that Codex ignores unknown frontmatter keys before the
change ships. Do not use them for claude.ai or API uploads: those reject keys outside the
Agent Skills spec.

### 7. `.claude-plugin/skills/` is a dead copy (low)

`marketplace.json` sets the plugin source to `./`, so Claude Code reads skills from the
root `skills/` folder. Claude Code does not read a `skills/` folder inside `.claude-plugin/`.
The copy costs a sync test (`test_skills_md_sync_contract` in
`crates/code-kb-cli/tests/cli_test.rs`) and a preflight step
(`scripts/release-preflight.sh`, step 1). Proposed fix: delete the copy, the test loop,
and the preflight loop. First confirm that no other harness manifest points into
`.claude-plugin/skills/`.

### 8. No skill has an evaluation (medium)

The guide says to build at least three evaluations per skill before more documentation,
and to test with each model you plan to use. No evaluation exists for any of the three
skills. Proposed evaluations:

- `code-kb`: (a) "where is `ensure_fts_index` defined" loads the skill and calls `lookup_symbol`, not `rg`; (b) "find the error text `Indexing ... started`" uses `rg`, not `search_symbols`; (c) "what calls `symbol_not_found_parts`" uses `find_references` with `project_root`.
- `telemetry`: (a) "how many tokens did code-kb save this week" loads `telemetry`, not `code-kb`; (b) the reply quotes the coverage figure; (c) `/telemetry 7d workspace` passes `workspace_only=true`.
- `report-issue`: (a) the skill shows the full bundle before it submits; (b) a refusal to approve stops the submit; (c) no `gh` auth gives the new-issue link, not `github_issue_url`.

`claude plugin eval` can run these as a plugin eval suite. Check its format before the
suite is written.

## Order of work

1. Findings 1, 2, 3, and 4 in one change to `skills/code-kb/SKILL.md` (and its copy, until finding 7 lands).
2. Finding 7, after the manifest check.
3. Finding 6, after the Codex check.
4. Finding 8.
5. Finding 5 only if the user approves the rename.

`test_guidance_docs_use_canonical_parameter_names` reads `skills/code-kb/SKILL.md`; run it
after step 1.

## Outcome

The user approved all findings and the rename on 2026-10-02.

- Findings 1 to 4: `skills/code-kb/SKILL.md` has the new description, a server-name line, one table with CLI twins, and the rules that change a call. About 1,500 words became about 550.
- Finding 5: `skills/telemetry` is now `skills/code-kb-telemetry`. In the plugin the command is `/code-kb:code-kb-telemetry`; the bare `/code-kb-telemetry` also works. README names it.
- Finding 6: `code-kb-telemetry` and `report-issue` have `argument-hint`, and their descriptions now say what the skill does first. The bodies do not use `$ARGUMENTS`: Claude Code appends `ARGUMENTS: <input>` when no placeholder takes it, and other harnesses would show the literal text. Codex ignores unknown frontmatter keys: `SkillFrontmatter` in `codex-rs/skills/src/parser.rs` has no `deny_unknown_fields`.
- Finding 7: `.claude-plugin/skills/` is deleted. No manifest pointed at it. `test_skills_md_sync_contract` became `test_skills_follow_agent_skill_frontmatter_rules`, which checks the name, the description, and the body length of each skill. The preflight script no longer compares the copies.
- Finding 8: `evals/` has three cases per skill, with mocks for the `code-kb` MCP server and its real `tools/list` in `evals/mocks/code-kb/_tools.json`. `scripts/plugin-eval.sh` runs them: `claude plugin eval` refuses a plugin folder with more than 20,000 entries, so the script runs the suite on a copy of the tracked files.
