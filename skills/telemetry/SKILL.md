---
name: telemetry
description: Use when the user runs /telemetry or asks how many tokens code-kb saved, how often its tools fail, or what the recent tool errors were. Arguments are optional; a time window (today, 7d, 30d, month, year, all) and the word "workspace" to scope to the current repository.
---

# code-kb Telemetry

Summarize code-kb tool usage, token savings, and recent errors from `~/.code-kb/telemetry.db`.

## Steps

1. Read the arguments. The window defaults to `all`. The word `workspace` sets `workspace_only=true`; otherwise report all workspaces.
2. Call `telemetry_summary(time_window=<window>, workspace_only=<bool>)`.
3. Reply in this shape:
   - First line: scope, window, total calls, success rate, tokens served, tokens saved.
   - Then only what needs attention: tools under 95% success, errors that repeat, calls to tool names that do not exist.
   - Skip tools with nothing notable. Do not paste the whole table.
4. End with the terminal twin so the user can verify:

```
code-kb stats --since <window> [--workspace]
```

## It's working if

- The reply fits on one screen and names the window it covers.
- Every flagged item points at one tool or one repeated error.
