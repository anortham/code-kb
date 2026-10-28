# Release readiness review after v2.9.1

Date: 2026-10-28. Source: `main` at `7c46520`, 44 commits after `v2.9.1`, not pushed.

## Result

The project is not ready for a release. The local gate passes, but four high
findings are regressions against v2.9.1, and the release process steps have not run.

## Local gate at 7c46520

- `cmp AGENTS.md CLAUDE.md`, `cargo fmt --check`, and Clippy with `-D warnings` pass.
- `cargo test --workspace --locked` passes 1,910 tests. `node --test tests/plugin/*.test.cjs` passes 20.
- `python3 -B -m unittest discover -s scripts -p 'test_*.py'` passes 301 tests.
- `cargo package --workspace --no-verify` succeeds.

## Process blockers

1. The 44 commits are not on `origin/main`, so CI has not run on them (preflight step 8).
2. The reach gate, corpus, and dogfood reports in `target/` are from v2.9.1 (steps 9-11).
3. The versions still say 2.9.1. JSON output gained fields, so the next version is a minor release.
4. `[patch.crates-io]` points `notify-debouncer-full` at `vendor/`. A crates.io publish drops
   the patch: the packaged `code-kb-core` manifest names plain `notify-debouncer-full = "0.7"`.
   `cargo install code-kb-cli` would ship without the removal-event fix of `89991f0`.
   crates.io has no stable release with the fix (newest: `0.8.0-rc.2`).

## Findings

Each finding passed one skeptic who tried to refute it against the source at HEAD.
The lead reproduced finding 1 live (below). Findings 2, 3, and 4 have scratch repros by the reviewers.

### 1. [high] A directory that is deleted and created again before the reconcile walk keeps its old registration and is never watched again

- Location: `crates/code-kb-core/src/watcher.rs:618` (watcher)
- Fix: Make the registration notice a new folder at an old path: on a Remove(Folder) or DELETE_SELF event, delete that path and the paths under it from `registrations.paths` before the reconcile; or store the folder's file ID with each registration and call `debouncer.watch` again when the ID changes. Add a test that deletes and creates a folder again inside one debounce window.

- Lead repro: an MCP session on a scratch repo ran `rm -rf src/foo && mkdir src/foo` and wrote a file. HEAD indexed that file, but later edits and new files in `src/foo` never reached the index. The v2.9.0 binary missed the first file but indexed later edits.

### 2. [high] Qualified nested-type calls (Outer.Inner.build()) dropped from proven 'qualified path' to unresolved in Python, Java, and C#

- Location: `crates/code-kb-core/src/queries.rs:4492` (queries)
- Fix: Use the chain status only when the first segment binds to a value. When no binding is found (owner NULL for that reason, not because of an evidence limit), fall back to `{predicate}` in queries.rs:4490 and skip the chain WHEN branch in reference_report.rs:5350. Add Python, Java, and C# regression tests for `Outer.Inner.build()`.

### 3. [high] New required-parameter path turns proven broken method calls into 'unresolved', so check_edit and the Stop hook no longer report them (regression vs v2.9.1)

- Location: `crates/code-kb-core/src/edit_check.rs:2009` (edit_check)
- Fix: In calls(), when required_arguments_on_line returns None (owner unknown), do not mark the site unresolved if verdict already rejects the count. Otherwise keep the label from the callers report, and use the unresolved fallback only when the owner check returns Some(false) on conflicting evidence.

### 4. [high] One unreadable subdirectory makes every MCP and CLI read fail, and the `code-kb scan` remedy does not clear it

- Location: `crates/code-kb-cli/src/mcp/server.rs:122` (mcp_cli)
- Fix: Treat a PermissionDenied walker error in reconcile and in watcher registration as a skipped directory that only logs a warning, or name `.code-kb-ignore` as the remedy in the error instead of `code-kb scan`.

### 5. [medium] New Elixir arity rule makes check_edit report every piped call (x |> f(a)) as a proven stale call

- Location: `crates/code-kb-core/src/reference_report/arity.rs:79` (queries)
- Fix: For Elixir, if `|>` comes right before the call (on the same line, or at the start or end of the line next to it), add 1 to the argument count or treat the count as unknown. Add a real-extraction regression test in edit_check_arguments_test.rs and note the pipe rule in docs/argument-counts.md.

### 6. [medium] New Ruby arity rule counts a &block parameter as a required positional argument

- Location: `crates/code-kb-core/src/reference_report/arity.rs:801` (queries)
- Fix: For Ruby, mark a segment that starts with `&` as not positional (or drop it in `segments.retain`), and treat a required keyword `name:` the same way. Add a regression case with a real `&block` method and a call that passes a block.

### 7. [low] Dropping a WatcherHandle now joins the worker, so a root switch blocks the MCP call until the old root's scan finishes

- Location: `crates/code-kb-core/src/watcher.rs:150` (watcher)
- Fix: In WatcherHandle::drop, set `shutdown` and let the worker thread run to its end without a join on the call thread (or join it on a background thread), so a root switch never waits on the old root's scan.

### 8. [low] Windows now opens one ReadDirectoryChangesW handle and 16 KB buffer for each directory instead of one root watch

- Location: `crates/code-kb-core/src/watcher.rs:623` (watcher)
- Fix: On Windows, keep one recursive root watch and drop events for ignored paths in the callback. Otherwise, measure live serve RSS on aspnetcore under Windows and record the per-directory cost under invariant 2.

### 9. [low] On Linux each directory removal or rename re-registers every directory, and each registration scans the debouncer's root list (quadratic)

- Location: `crates/code-kb-core/src/watcher.rs:692` (watcher)
- Fix: In the vendored debouncer, change `roots` to a set or a BTreeMap keyed by path, so the `add_root` duplicate check is O(log n) or O(1); use a range removal for `remove_root`.

### 10. [low] Python self.attr.method() calls lost 'receiver type' proof and exclusions (proven to likely, excluded to unresolved)

- Location: `crates/code-kb-core/src/queries.rs:4372` (queries)
- Fix: In typed_receiver_chain_scope_ctes/owner_id_sql, when segment 0 is `self`, `this`, or `cls` and has no type fact, start the chain at the caller's enclosing class (lexical owner of s_from). Add a Python test with a `self.attr.method()` chain that must be proven, plus one with a decoy.

### 11. [low] docs/reach-sections.md not updated for the new Constructions rules

- Location: `crates/code-kb-core/src/reference_report/sections.rs:1773` (queries)
- Fix: Update the Constructions section of docs/reach-sections.md for the new rules: the excluded type use that resolves to the target, the C/C++/Zig/F#/Elixir literal rules on `uses` rows, the source-checked constructor calls for C#, Kotlin, Scala, Swift, and Ruby, and the new comment and string skips. Also fix the "Excluded sites never count" line in the doc comment on `constructions`.

### 12. [low] Broader Gradle command for a subproject ignores the root wrapper and emits `cd app && gradle test`

- Location: `crates/code-kb-core/src/test_commands.rs:133` (edit_check)
- Fix: In the Gradle branch of `scoped_command` (and of `root_command`), find the nearest `gradlew` and `gradlew.bat` at or above the build folder, as `java()` does with `nearest`. Build the path with `wrapper_path`, for example `cd app && ../gradlew test`. Add a test with a root `gradlew` and `app/build.gradle`.

### 13. [low] npm's placeholder test script counts as a configured runner for the broader suite

- Location: `crates/code-kb-core/src/test_commands.rs:347` (edit_check)
- Fix: Make has_test_script return false for npm's default placeholder (a script that contains "no test specified" and ends in `exit 1`), and add a regression case for the `npm init -y` package.json.

### 14. [low] CLI `refs --direction callees --json` returns cross-language callees that the text output and MCP drop

- Location: `crates/code-kb-cli/src/main.rs:753` (mcp_cli)
- Fix: In the `Command::Refs` arm of main.rs, send callees with `--json` to `find_callees_page_scoped` / `find_callees_page_for_symbol` as well, with `limit`/`offset`. Or add the `call_languages_compatible_sql` and `pending_relationship_language_filter` predicates to the callee branch of `find_direct_references`.

### 15. [low] CLI zero-limit notice now prints with no trailing newline

- Location: `crates/code-kb-cli/src/main.rs:710` (mcp_cli)
- Fix: Use println! at main.rs:710/798/843/880 and change the parity test in report_recovery_test.rs to compare cli_text.trim_end() with the MCP text, or add '\n' to both ZERO_LIMIT_NOTICE constants.

### 16. [low] MCP callee page hint tells agents to use the CLI flag `--offset`

- Location: `crates/code-kb-core/src/formatters.rs:1048` (mcp_cli)
- Fix: In format_references_window, replace the flag-specific wording with neutral text such as `pass offset N to see more`, or let the caller give a parameter-name argument.

### 17. [low] MCP `offset` refuses a string integer that `limit` accepts

- Location: `crates/code-kb-cli/src/mcp/server.rs:149` (mcp_cli)
- Fix: In result_offset, parse a numeric string the same way result_limit does, then add a unit test for {"offset":"50"}.

### 18. [low] Release evidence fingerprint does not cover the patched vendor/notify-debouncer-full crate

- Location: `scripts/release-evidence-inputs.json:33` (release_hygiene)
- Fix: Add "vendor/**/*" (or "vendor/notify-debouncer-full/**/*") to the globs in scripts/release-evidence-inputs.json, and add a test_release_evidence.py case where a change to a vendor file changes the fingerprint.

### 19. [low] A dogfood fix that touched Cargo.toml or Cargo.lock becomes 'stale' after the version-bump commit, and record-fix cannot repair it

- Location: `scripts/release_evidence.py:902` (release_hygiene)
- Fix: In latest_input_commits, give a path to a commit only when that commit changes the path's normalized hash. Hash the content before and after each commit with normalized_input_bytes. Then a package-version-only commit no longer counts as the latest change.

### 20. [low] RELEASING.md says documentation-only changes need no new evidence, but two Markdown docs are fingerprint inputs

- Location: `docs/RELEASING.md:149` (release_hygiene)
- Fix: Remove `evals/review-2026-10-26/README.md` from `release-evidence-inputs.json`. In RELEASING.md, state that `scripts/release-dogfood-prompt.md` is an evidence input and the one exception to the documentation rule.

### 21. [low] README find_references row still names the old seven-language argument-count list

- Location: `README.md:467` (release_hygiene)
- Fix: In the README.md:467 `find_references` row, replace the seven-language list with a pointer to docs/argument-counts.md, or with the full 21-language list from that doc.

## Refuted

- A parked prepare that finishes after a root switch keeps a live watcher for the inactive root until the next switch
- Removed-member reads run one 3 s reference report per removed field inside one candidate, outside the 20 s check budget
- Fingerprint omits routing-block.md, which include_str! compiles into the binary
- CLAUDE.md/AGENTS.md release-workflow summary still says an ancestor reach PASS is enough
- Merged task worktree is still registered

## Other state

- The merged worktree `/home/murphy/.codex/worktrees/1e6f/code-kb` (`fix/cross-language-review`) is clean and fully merged into `main`.
