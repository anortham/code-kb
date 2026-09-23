---
name: telemetry
description: Use when the user runs /telemetry or asks how many tokens code-kb saved, how often its tools fail, or what the recent tool errors were. Arguments are optional; a time window (today, 7d, 30d, month, year, all) and the word "workspace" to scope to the project of the most recent code-kb call.
---

# code-kb Telemetry

Summarize code-kb tool usage, token savings, and recent errors from `~/.code-kb/telemetry.db`.

## Steps

1. Read the arguments. The window defaults to `all`. The word `workspace` sets `workspace_only=true`; otherwise report all workspaces. `telemetry_summary` takes no `project_root`: `workspace_only` scopes to the project of the most recent code-kb call.
2. Call `telemetry_summary(time_window=<window>, workspace_only=<bool>)`.
3. Reply in this shape:
   - First line: scope, window, total calls, success rate, tokens served, tokens saved with its coverage.
   - Then only what needs attention: tools under 95% success, errors that repeat, calls to tool names that do not exist.
   - Skip tools with nothing notable. Do not paste the whole table.
4. Report tokens saved with the coverage figure beside it. The header reads `Est. Tokens Saved: ~N (baseline known for K of M calls)`, and each tool row reads `~N (K/M)`. Never quote the number without the coverage.
5. End with the terminal twin so the user can verify:

```
code-kb stats --since <window> [--workspace]
```

## What "saved" means

The saved figure is the size, in estimated tokens, of the files the answer points into, minus the tokens served. A skeleton, body, or context read is measured against its own file. A lookup, search, references, blast-radius, or facts answer is measured against the distinct files its rows name, at most 20 files. A call with no file to point at, such as an outline or a telemetry summary, records no baseline and is not counted as saved. Calls recorded before this measurement existed keep no baseline either, so old windows show a lower coverage.

## It's working if

- The reply fits on one screen and names the window it covers.
- The saved number carries its coverage figure.
- Every flagged item points at one tool or one repeated error.
