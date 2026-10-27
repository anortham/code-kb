# MiniJinja checker correction design

The corrected case copy is still byte-for-byte identical to `cases/minijinja`. The predeclared adjudication classifies one strict-check failure as a wording false negative: the actual help says “Suffixes are matched without regard to ASCII letter case,” while the frozen phrase list accepts “without regard to case.” Keep the original checker, case, attempts, and grades unchanged.

## Test-first correction after the lead's go

Extend the corrected copy's existing help-check test with a small predicate that inspects only the `--autoescape` help section, requires `.j2`, `.jinja`, and `.jinja2` as separate suffix tokens, and requires one explicit case-insensitivity phrase. The current substring check for `.jinja` also matches `.jinja2`; require a non-alphanumeric token boundary after each suffix so that one cannot stand in for another. Add examples before broadening the phrase list:

- Accept the original failing help text: all three suffixes and “without regard to ASCII letter case.”
- Accept a concise equivalent: all three suffixes and “case-insensitive.”
- Reject all three suffixes with no explanation of case behavior.
- Reject case-insensitive wording when `.j2` is absent.
- Reject case-insensitive wording when `.jinja` is absent, even if `.jinja2` is present.
- Reject case-insensitive wording when `.jinja2` is absent.
- Reject the wording when it appears outside the `--autoescape` section.

Then add the specific normalized phrase `without regard to ascii letter case` to the accepted language. Keep the suffix checks and section boundary. Do not accept generic mentions of “case” or wording that says matching is case-sensitive.

Do not run these correction tests, builds, or readiness commands until the lead says go. Afterward, bind the updated corrected case and its fresh readiness evidence through the lead-owned v2 manifest; keep the original strict results as a separate historical record.
