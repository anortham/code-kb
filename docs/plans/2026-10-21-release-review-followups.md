# Release review follow-ups

**Goal:** Correct confirmed defects from code-kb v2.7.0..v2.8.0 and julie-extractors v3.8.0..v3.9.0.
**Architecture:** Julie owns extracted facts; code-kb owns query-time resolution and hook telemetry. Keep both boundaries and existing CLI/MCP interfaces.
**Tech Stack:** Rust, SQLite, Python.
**Spec:** [Agent guidelines](../../AGENTS.md), [release behavior](../release-notes/v2.8.0.md), and the owner's 2026-10-21 review request. Project instructions include fixing confirmed review defects.
**Architecture Quality:** Preserve module boundaries; inaccurate extraction or shortened type names must never justify dropping a real caller.
**Authority:** Existing request authorizes local fixes and commits. Push, PR, deployment and releases remain unauthorized. External reviewer choice: none.

## Global constraints

- Work in /home/murphy/source/code-kb on review/v2.8.0-followups and /home/murphy/source/julie-extractors on review/v3.9.0-followups. Both initially clean; no other worktrees exist.
- No dependency, artifact-schema, release-version or pin changes. Leave development binaries outside tracked files.
- Follow code-kb navigation rules with explicit project_root. Get context and callers before edits, blast radius before tests, check_edit after changes.
- Tests first for behavior. No narration comments. Do not revert others' work. Workers neither commit nor checkpoint; lead owns integration.
- Review campaign is ordinary, fresh-session, two rounds maximum, zero external CLI calls. Discovery is frozen. Confirmation covers fixes only.

## Verification strategy

**Project source of truth:** Both AGENTS.md files and workspace Cargo manifests.
**Worker red/green scope:** Task-specific focused tests below.
**Worker ceiling:** Assigned test file or narrow language test tier; no full suites.
**Worker gate invariant:** Regressions fail before the fix and pass after it; unknown evidence retains sites.
**Lead affected-change scope:** Combined changed Rust integration tests, replay tests, extractor C# tests, direct CLI reproductions.
**Branch gate:** code-kb cargo fmt --check; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace; node --test tests/plugin/*.test.cjs; Python replay tests. Julie formatting, focused C# tests and project default test tier.
**Security scope:** none declared. No dependencies or new outbound behavior.
**Replay/metric evidence:** Targeted reproductions are hard gates. Existing large-corpus release scores are historical, not rerun or claimed fresh.
**Escalation triggers:** A fix changes a public contract or extraction schema, or broad verification finds a regression caused by this diff.
**Assigned verification failure:** Investigate within owned scope; report unresolved blockers with logs.
**Verification ledger:** .razorback/sdd/release-review/progress.md plus worker reports; record commands, results and commits. Do not repeat passing unchanged scopes.

## Parallel execution contract

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| 1 Java imports | A | code-kb reference_report.rs and tests/reference_report_test.rs | No | None - safe parallel batch. |
| 2 C# metadata | A | Julie csharp/argument_types.rs and tests/csharp_argument_types.rs | No | None - safe parallel batch. |
| 3 Hook attribution | A | code-kb telemetry.rs, edit_check/session.rs, CLI hooks.rs, CLI tests/edit_hook_test.rs | No | None - safe parallel batch. |
| 4 Lead integration | A | code-kb edit_check.rs, edit_check/binding.rs, tests/edit_check_test.rs, scripts/edit_replay.py, scripts/test_edit_replay.py; documentation and memory in both repos | No | None - safe parallel batch. |

### Task 1: Judge all Java static imports

**Files:** crates/code-kb-core/src/reference_report.rs; crates/code-kb-core/tests/reference_report_test.rs in code-kb.
**Interfaces:** Accounting::java_static_import consumes indexed imports and emits an optional site verdict. Preserve report schemas.
**Contract inputs:** /tmp/code-kb-java-import-review-e_s_f7hp reproduces two static imports of check from p.Util and q.Other. check(true) can reach Util, check("hello") Other. Current first-import selection excludes or proves both by import order.
**File ownership:** code-kb reference_report.rs and tests/reference_report_test.rs.
**Serialization required:** No.
**Dependency reason:** None - safe parallel batch.
**What to build:** Inspect all static imports. A rival reachable owner prevents an unjustified proof or exclusion. A unique static import retains existing proof behavior, including removed symbols.
**Approach:** Test both import orders and explicit/wildcard imports with distinct owners. Prefer unresolved over invented overload resolution. Do not fix the JS regex heuristic: the lead reproduced it as safely likely with an explicit binding basis, not a false proof.
**Acceptance criteria:**
- [x] Both import orders preserve reachable target calls and do not falsely prove rival calls.
- [x] Single-owner static imports retain behavior; removed-member checks are included in the branch gate.
- [x] Red/green targeted tests pass; relevant reference-report group passes; report handed to lead, no commit.

### Task 2: Correct C# argument metadata

**Files:** crates/julie-extractors/src/csharp/argument_types.rs; crates/julie-extractors/tests/csharp_argument_types.rs in Julie.
**Interfaces:** enrich emits arg_types as an array of optional type strings; uncertain expressions remain null. Preserve schema and metadata shape.
**Contract inputs:** /tmp/code-kb-csharp-scope-review-keeir9lf demonstrates a local from a finished nested block overriding a same-named string field, emitting int and suppressing the true caller. integer_type currently uses suffix alone, so large unsuffixed and U/L-suffixed values get wrong types.
**File ownership:** Julie csharp/argument_types.rs and tests/csharp_argument_types.rs.
**Serialization required:** No.
**Dependency reason:** None - safe parallel batch.
**What to build:** Respect lexical visibility when selecting typed declarations. Infer integer literal types from value, radix and suffix under C# rules, with null for unsupported/invalid forms.
**Approach:** Use AST scope evidence without workspace resolution. Check ended blocks, nested active blocks, loop/catch scopes and signed literal boundary handling. Official type rules: https://learn.microsoft.com/en-us/dotnet/csharp/language-reference/builtin-types/integral-numeric-types ; unsuffixed values choose int,uint,long,ulong, U chooses uint/ulong, L chooses long/ulong. 2147483648 is uint and can convert to long; it is not int.
**Acceptance criteria:**
- [x] A completed block local cannot replace the field or outer variable visible to a call.
- [x] Active inner bindings are selected; unsupported bindings remain unknown rather than borrowing an outer type.
- [x] Decimal/hex/binary integer boundaries, separators and suffixes have correct types; unary minus boundary exceptions remain sound.
- [x] cargo test -p julie-extractors --test csharp_argument_types passes after red/green; report handed to lead, no commit.

### Task 3: Attribute hook outcomes to one session

**Files:** crates/code-kb-core/src/telemetry.rs; crates/code-kb-core/src/edit_check/session.rs; crates/code-kb-cli/src/hooks.rs; crates/code-kb-cli/tests/edit_hook_test.rs in code-kb.
**Interfaces:** record_hook_check and record_hook_followed are internal-to-product Rust entry points exported by the crate. Keep CLI and JSON output unchanged; additive local telemetry fields are allowed.
**Contract inputs:** Two sessions can block on the same root and diff hash. Current UPDATE matches every unanswered blocked row by root/hash, so one session's next Stop incorrectly resolves another session's row.
**File ownership:** code-kb telemetry.rs, edit_check/session.rs, CLI hooks.rs, CLI tests/edit_hook_test.rs.
**Serialization required:** No.
**Dependency reason:** None - safe parallel batch.
**What to build:** Tie follow-through to the exact blocked check/session. Preserve existing telemetry opt-outs, upgrade existing telemetry databases, and preserve one-block-per-diff behavior.
**Approach:** Add a check or session identity and ensure writes are ordered safely with session locking. Avoid tying one outcome to rows from another session that shares the hash. Check every caller if function parameters change.
**Acceptance criteria:**
- [x] Two sessions with identical root/hash receive independent outcomes; one follow-through leaves the other unanswered.
- [x] Existing followed/not-followed tests and legacy telemetry upgrade still pass.
- [x] Focused regression red/green and cargo test -p code-kb-cli --test edit_hook_test pass; report handed to lead, no commit.

### Task 4: Lead fixes and verification

**Files:** Lead ownership from parallel contract, including both repos' docs and memory.
**Interfaces:** Preserve full C# type identity in overload checks and do not accept a sibling overload by argument count alone; replay commands resume only identical binary contents. Correct v7-to-v8 recovery instructions.
**Contract inputs:** Current binding normalization strips custom namespaces. After Pick(A.Token) gains a parameter, Pick(B.Token) can wrongly absorb its unchanged A.Token caller. Replay --out currently reuses results after binary replacement with the same version. Julie --force still refuses schema 7 writes.
**File ownership:** code-kb edit_check.rs, edit_check/binding.rs, tests/edit_check_test.rs, scripts/edit_replay.py, scripts/test_edit_replay.py; documentation and memory in both repos.
**Serialization required:** No.
**Dependency reason:** None - safe parallel batch.
**What to build:** Fix namespace loss, replay identity, and schema-upgrade instructions. Review workers, run combined verification, checkpoint and commit each repository locally.
**Approach:** Normalize only proven framework aliases; preserve custom qualification. Fingerprint executable contents before replay resume, reject legacy unverified directories without deleting results. Document fresh v8 output rather than an impossible in-place --force upgrade.
**Acceptance criteria:**
- [x] Calls of custom same-short-name types remain listed after an overload changes. The regression also exposed arity-only pruning; typed overloads now require a matching type vector before an accepted count drops a site.
- [x] All replay runners reject replaced binaries with unchanged version; unchanged binaries can resume; legacy scores remain readable.
- [x] Migration docs give a valid fresh-artifact command.
- [x] Every accepted finding has source evidence and passing validation; final state is committed locally in both repositories, with no push/release.

## Completed verification

- code-kb workspace: 1,550 passing tests with the rebuilt optimized Julie; strict workspace Clippy, formatting, and 20 plugin tests pass.
- Julie default tier: 6,929 passing tests, seven existing ignored, 163 seconds; C# golden and strict extractor Clippy pass.
- Replay harness: 64 passing tests. Direct CLI reproductions and both check_edit calls pass.
- Worker reports were reviewed inline; source changes remain within the extraction/query boundary. Unknown scope/type evidence retains the caller.
- Windows and the large-corpus release gates were not rerun. No push or release occurred.

Julie task 2 and migration documentation committed as `f9a4fa1f` on `review/v3.9.0-followups`. Both checkouts retain their original paths; no extra worktrees were created.
