# Attempt 004 hidden help assertion diagnostic

The immutable hidden test `evals/edit-2026-10-27/cases/minijinja/checks/edit_eval_autoescape.rs` normalizes `--long-help`, isolates the `--autoescape` section, verifies `.j2`, `.jinja`, and `.jinja2`, then requires one exact substring from this allowlist:

- `ignoring case`
- `case-insensitive`
- `case insensitive`
- `regardless of case`
- `without regard to case`

The captured grade output says: `Suffixes are matched without regard to ASCII letter case, so report.html.jinja2 uses HTML escaping and report.JSON.JINJA uses JSON escaping.` The wrapper names and examples are present. The exact phrase allowlist misses this wording because `ASCII letter` appears between `regard to` and `case`.

The request asks for matching `without regard to ASCII letter case`; the candidate help states that behavior. Its added test `test_autoescape_long_help_describes_wrapped_suffixes` checks the same wording and passed in the captured in-trial run. The source review therefore treats the hidden help failure as a wording false negative, preserves the failed strict grade, and makes no case or test changes.

Evidence: attempt `004-minijinja-code-kb-r1` grade logs `grade-006-stdout_log.log` and `grade-007-stderr_log.log`; source review logs `candidate-vs-seed.diff`, `long-help-test-body.stdout.log`, and `apply-to-env-body.stdout.log`.
