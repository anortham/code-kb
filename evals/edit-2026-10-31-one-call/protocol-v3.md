# One-call answers: release measurement method

This note extends [the base protocol](../edit-2026-10-27/protocol.md) and
[its corrections](../edit-2026-10-27/protocol-v2.md). It measures the code-kb arm
only, for the release that adds one-call answers and julie-extract 3.11.0. The
plan is `docs/plans/2026-10-31-one-call-answers.md`, section "Measurement".

## What changes

- The engine. The code-kb binary is built from `feat/julie-3.11`, and the
  extractor is julie-extract 3.11.0. Both live in a new tools folder.
- The guidance. The code-kb guidance is built the same way as the frozen one:
  the routing block (`crates/code-kb-cli/src/routing-block.md`, which
  `code-kb hook SessionStart` prints), a newline, then the body of
  `skills/code-kb/SKILL.md` after its front matter. The profile embeds
  `common.md`, a newline, and that guidance as `developer_instructions`. All
  other profile lines are unchanged. The MCP server instructions and tool
  descriptions come from the new binary.

## What stays fixed

- Codex CLI 0.164.0 (the cached frozen executable), `gpt-6.1-sol`, high effort,
  and a 900-second deadline.
- The six corrected cases, the seeds, the dependency inputs, the sandbox and
  the grader.
- The native profile and guidance bytes. No native cell runs.

## Schedule

The manifest keeps the 24-cell schedule, because the validator requires it.
Only the 12 code-kb slots run: 2, 4, 6, 8, 10, 12, 13, 15, 17, 19, 21 and 23.
They run once each, in schedule order. No completed trial is rerun.

## Pass mark

All three must hold, against the frozen 2.11.x code-kb arm:

1. At most 16.47 MCP calls per trial (20% fewer than 247 over 12 trials).
   An MCP call is one `code_kb_calls` count in the trace metrics of the
   attempt record, the same field that gave the 247.
2. Acceptance 12/12: corrected behavioral grading plus independent source
   review, including the NodaTime purity test and the added tests on fresh
   public replays.
3. Reported input tokens, including cached input, at most 8,506,310.

A fail sends the design back. Nothing is tuned to pass.
