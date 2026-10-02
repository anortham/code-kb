---
max_turns: 15
allowed_tools: [Read, Glob, Grep, Skill, Bash, Write]
tags: [report-issue]
---

This folder is my Flask checkout. File a bug against code-kb. `lookup_symbol(query="Flask")` returned `No symbol matched` in my Flask checkout, but `src/flask/app.py` defines `class Flask`. I expected one class row. Title: lookup misses Flask class.
