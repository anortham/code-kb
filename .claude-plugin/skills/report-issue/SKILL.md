---
name: report-issue
description: Use when the user runs /report-issue, says code-kb misbehaved, or asks to file a bug against code-kb. Collects a diagnostic bundle (versions, index facts, recent tool errors, log tail) and opens a GitHub issue with it.
---

# Report a code-kb Issue

File a GitHub issue against `anortham/code-kb` with a diagnostic bundle attached. Never send the bundle anywhere else. Nothing leaves the machine before the user approves the exact text in step 4.

## Steps

1. Ask the user one question with three parts: what went wrong, what they expected, and the exact tool call or CLI command with its output. Skip the parts they already gave. Keep the title under 80 characters.
2. Build the bundle in the terminal, from the repository where the problem happened:

```
code-kb bug-report --title "<title>" --description "<what went wrong, expected, reproduction>" --json > <scratch>/bug-report.json
```

   The bundle holds the OS, both binary versions, index facts, the last 10 tool errors, and the last 40 log lines. Home directories are masked as `~`. Nothing else is masked: tokens, hostnames, or paths outside the home directory in an error or log line stay as they are.
3. Write the `markdown_body` field to `<scratch>/bug-report.md`. That file is the only text that gets submitted.
4. Show the user the whole file, including the error table and the log lines. Ask what must be removed. Wait for the answer. Edit the file, then show it again until the user says it is approved. Do not shorten this step.
5. Submit the approved file:
   - If `gh auth status` succeeds, run
     `gh issue create --repo anortham/code-kb --title "<title>" --body-file <scratch>/bug-report.md`
     and give the user the issue link.
   - Otherwise give the user the title, the path of the approved file, and `https://github.com/anortham/code-kb/issues/new`. Tell them to paste the file as the body. Do not use the `github_issue_url` field from the bundle: it was built before the edits in step 4.

## It's working if

- The submitted body is the approved file, byte for byte.
- The issue body starts with `### Environment` and names both binary versions.
- No absolute home path, token, or private hostname appears in the body.
- The user has one link: the created issue or the new-issue page to paste into.
