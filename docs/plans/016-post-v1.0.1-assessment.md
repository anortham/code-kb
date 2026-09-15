# 016: Post-v1.0.1 assessment vs julie and miller (2026-09-15)

Review of main @ adb865c, clean tree. Evidence: live code-kb tool calls, telemetry (30 d),
server logs, SQLite row counts, `cargo test --workspace` (all suites pass), `cargo fmt --check`.

## Verdict

The cut is right. The gain is in the runtime, not in the tool count.

| | code-kb | julie | miller |
|---|---|---|---|
| Source lines (no tests) | 11.4k Rust | 134k Rust | 395k C# |
| Runtime | 1 process per session, SQLite, pinned extractor | machine service, registry, Tantivy, ONNX sidecar, dashboard | .NET server, hydrated graph, embeddings, dashboard, CT daemon |
| Tools | 11 | 10 | ~12 |
| Workspace param | none | required | required |
| Edit | 1 turn | 2 turns | 2 turns |

Telemetry, last 30 d: 1539 calls, 98.8% success, 1 to 6 ms median.

## Findings and what was done

1. **Routing text banned grep with no fallback.** code-kb indexes names, signatures and
   docstrings, not file text. Fixed: routing block and SKILL.md now say to use `rg` for
   literal text, error strings, comments and config values.
2. **`search_symbols` ranked docs over code.** 4147 Rust symbols vs 1400 markdown, yaml,
   toml, json and css symbols in this repo. Fixed: FTS results order definitions before imports, code
   languages before markup and config languages, then by BM25 score.
3. **Ergonomics gaps seen in telemetry errors.** Fixed: `lookup_symbol` accepts
   `symbol_name` and `symbol`; `file_skeleton` on a directory returns that directory's
   outline. The qualified-name field-vs-method ambiguity was already fixed in 1d1627f.
4. **No extractor version guard.** `artifact_metadata.binary_version` was never read.
   Fixed: `ensure_index_matches_pin` removes and rebuilds the index once when the recorded
   extractor version differs from the pin. Runs at server start, on rebind, and before CLI
   reads. The extractor refuses writes into an older-schema artifact, so the old file is
   removed first.
5. **Accretion in telemetry.** Removed `migrate_legacy_workspace_telemetry` and its tests
   (a pre-1.0 per-workspace format). "Tokens saved" is now labeled as an estimate for read
   tools only; it counts 0 for `find_references`, which is 47% of calls.
6. **The call graph is name-matched, not resolved.** 542 resolved relationships vs 8599
   pending name-only calls. Tool description, README and SKILL.md now say so and tell the
   agent to qualify overloaded names.
7. **Integration tests spawned servers against the real repo.** Three no-root spawns now set
   `current_dir` to the temp repo.
8. **`code_kb_stats` unadvertised alias** contradicted invariant 8. Removed with its test.
9. **Stale checkboxes** in plans 010 and the 2026-09-14 release plan were ticked.

## Left alone

- Test files named by review campaign (`adversarial_m1/m2/m3_test.rs`, 11.8k lines). Rename
  by subject over time.
- The 1:1 session model spawns one server per subagent. Each start walks and hashes the
  whole tree. Cheap at 85 files, unmeasured at 1300. Time a cold `code-kb serve` on
  `~/source/miller` before any design change. If it is under a second, leave it.
