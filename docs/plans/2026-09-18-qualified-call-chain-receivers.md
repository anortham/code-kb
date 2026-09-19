# Qualified Call Chain Receivers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use razorback:subagent-driven-development whenever delegation is available and permitted, including for one task; serialize dependent tasks. Use razorback:executing-plans only when delegation is unavailable or the user/session explicitly selected single-agent execution.

**Goal:** `find_references`, callees, and `blast_radius` see every call written as a qualified chain (`Outer.Inner.method(...)`) in every language julie extracts, instead of dropping the receiver or the whole call.

**Architecture:** julie-extractors is the data producer: each language's relationship extractor builds an `UnresolvedTarget` (terminal name, receiver, namespace path) for a call it cannot resolve in-file, and the artifact stores it in `pending_relationships`. code-kb reads those rows in `find_references_internal` and filters them with `pending_target_predicate`, then falls back to `identifiers` rows matched by name. The fix lands first in julie (one shared chain-splitting helper, applied per language), then in code-kb (predicate accepts class ancestors as namespace, identifier fallback respects the recorded receiver), then the pin is bumped.

**Tech Stack:** Rust, tree-sitter, SQLite (rusqlite), `cargo xtask` test tiers in julie, `cargo test` in code-kb.

**Spec:** Diagnosis checkpoints `.memories/2026-09-18/223700_6709.md` (single-language root cause) and `.memories/2026-09-18/224150_c968.md` (cross-language survey) in this repo. There is no separate design doc; the survey table in the second checkpoint is the binding scope list.

**Architecture Quality:** Two module changes. (1) julie `base/relationship_resolution.rs` gains `UnresolvedTarget::from_chain` and `UnresolvedTarget::from_qualified_text`; every language builds chain targets through them. (2) code-kb `queries.rs` keeps `pending_target_predicate` as the single filter but teaches it that a namespace segment may name an ancestor symbol, and the identifier fallback gains one receiver check. Risk: julie's own in-file resolver (`resolve_call_target`, tier 3 receiver lookup) now sees a single-identifier receiver where it used to see a compound string; the contract and capability tiers must stay green, and any fixture change must be a receiver split only.

## Global Constraints

- Target shape for a qualified call `Q1.Q2 ... Qn.t(...)` with n >= 1: `terminal_name = t`, `receiver = Qn`, `namespace_path = [Q1 .. Qn-1]`, `display_name = "Q1.Q2 ... Qn.t"` (parts joined with `.`, regardless of the source separator). A two-part call `X.t()` keeps today's shape: receiver `X`, empty namespace.
- Split a receiver taken from source text only when every part is a plain identifier (ASCII letters, digits, `_`, `$`, `@`, leading `$` allowed). A receiver that contains a call, index, or other expression (`foo().bar`, `a[0].b`) keeps today's behavior unchanged.
- Language separators: `.` for all; additionally `::` for C++, PHP, Ruby, Rust-like syntax; `->` for C, C++, PHP; `$` for R.
- Do not change Scala, VB.NET, Rust, Elixir, F#, or Erlang relationship extractors. They already produce the target shape.
- No new crate dependencies in either repo.
- julie version becomes `3.1.1` in `crates/julie-extract-artifact/Cargo.toml`, `crates/julie-extractors/Cargo.toml`, `crates/julie-extract-cli/Cargo.toml`, with a release note at `docs/release-notes/v3.1.1.md` in the julie repo.
- code-kb pins the new extractor in `scripts/julie-pins.json` (version and the four sha256 values from the published archives) and updates the "currently `3.1.0`" text in `CLAUDE.md` and `AGENTS.md` in the same commit (the two files stay byte-identical).
- Test bodies contain no comments. No narration comments in changed code.
- julie work happens in the worktree `/home/murphy/source/julie-extractors/.worktrees/qualified-call-chain` on branch `qualified-call-chain`. Set `CARGO_TARGET_DIR=/home/murphy/source/julie-extractors/target` so dependency builds are shared with the main checkout.
- code-kb work happens in a worktree created with the native worktree tool from `/home/murphy/source/code-kb` when Task 7 starts.

---

## Verification Strategy

**Project source of truth:** julie: `docs/testing-strategy.md` and `docs/release.md`. code-kb: `docs/RELEASING.md`, `scripts/release-preflight.sh`, and `cargo test` per crate.

**Worker red/green scope:** julie: `cargo test -p julie-extractors <lang>::` with the new test name as filter (for example `cargo test -p julie-extractors tests::java::cross_file_pending::qualified_chain`). code-kb: `cargo test -p code-kb-core --test references_test <test_name>`.

**Worker ceiling:** julie: `cargo xtask test language <name>` for each language the task owns. code-kb: `cargo test -p code-kb-core`.

**Worker gate invariant:** the new test fails before the change and passes after; the owned language's existing suite stays green.

**Lead affected-change scope:** julie: `cargo xtask test changed <owned paths>` after each batch. code-kb: `cargo test -p code-kb-core && cargo test -p code-kb-cli`.

**Branch gate:** julie: `cargo fmt --check`, `cargo test -p xtask`, `cargo xtask test default`, `cargo xtask test contract`, `cargo xtask test capability`, `cargo xtask test golden`. code-kb: `cargo build --release`, `cargo test --workspace`, `node --test tests/plugin/*.test.cjs`.

**Security scope:** julie: `cargo deny check` at the branch gate. code-kb: `none declared`.

**Replay/metric evidence:** golden and capability fixture diffs are hard gates: a diff is accepted only when the changed row is a receiver split into `receiver` + `namespace_path` for a qualified chain, or a newly emitted pending row for a chain that previously emitted nothing. Any other diff is a regression.

**Escalation triggers:** a change under `crates/julie-extractors/src/base/` triggers `cargo xtask test contract` before the batch is accepted. Any failure in `cargo xtask test capability` stops the batch.

**Assigned verification failure:** Workers stop and report when assigned verification fails, unless this plan explicitly says to update that gate.

**Verification ledger:** Record invariant, command, scope label, commit SHA, result, and timestamp (plus hard-gate and report-only metrics). Reuse a passing entry for the same HEAD and scope instead of rerunning.

## Parallel Execution Contract

Commit mode: `serial-worker-commit` for Tasks 1, 6, 7, 8; `parallel-lead-commit` for Batch A (Tasks 2-5, one shared worktree).

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| Task 1: Shared chain helper + C# | None - serial | julie: `crates/julie-extractors/src/base/relationship_resolution.rs`, `crates/julie-extractors/src/csharp/relationships.rs`, `crates/julie-extractors/src/tests/csharp/cross_file_pending.rs` | Yes | Locks the `from_chain` / `from_qualified_text` contract every later julie task calls. |
| Task 2: Java, Kotlin, Go, PowerShell | Batch A | julie: `crates/julie-extractors/src/{java,kotlin,go,powershell}/relationships.rs`, `crates/julie-extractors/src/tests/{java,kotlin,go,powershell}/cross_file_pending.rs` | No | None - safe parallel batch. |
| Task 3: C++, Zig, JavaScript, TypeScript | Batch A | julie: `crates/julie-extractors/src/cpp/relationships.rs`, `crates/julie-extractors/src/zig/relationships.rs`, `crates/julie-extractors/src/javascript/mod.rs`, `crates/julie-extractors/src/javascript/relationships.rs`, `crates/julie-extractors/src/typescript/relationships.rs`, `crates/julie-extractors/src/tests/{cpp,zig,javascript,typescript}/cross_file_pending.rs` | No | None - safe parallel batch. |
| Task 4: Python, Swift, Dart, Lua, GDScript | Batch A | julie: `crates/julie-extractors/src/python/relationships.rs`, `crates/julie-extractors/src/swift/relationships.rs`, `crates/julie-extractors/src/dart/pending_calls.rs`, `crates/julie-extractors/src/lua/relationships.rs`, `crates/julie-extractors/src/lua/core.rs`, `crates/julie-extractors/src/gdscript/relationships.rs`, `crates/julie-extractors/src/tests/{python,swift,dart,lua,gdscript}/cross_file_pending.rs` | No | None - safe parallel batch. |
| Task 5: C, PHP, Ruby, R | Batch A | julie: `crates/julie-extractors/src/c/relationships.rs`, `crates/julie-extractors/src/php/call_relationships.rs`, `crates/julie-extractors/src/ruby/relationships.rs`, `crates/julie-extractors/src/r/relationships.rs`, `crates/julie-extractors/src/tests/{c,php,ruby,r}/cross_file_pending.rs` | No | None - safe parallel batch. |
| Task 6: julie branch gate, fixtures, 3.1.1 | None - serial | julie: `fixtures/**` rows that legitimately change, `crates/*/Cargo.toml`, `Cargo.lock`, `docs/release-notes/v3.1.1.md`, `docs/contracts/extracted-data-v3.md` | Yes | Needs Tasks 1-5 merged into the branch. |
| Task 7: code-kb namespace ancestors + identifier receiver | None - serial | code-kb: `crates/code-kb-core/src/queries.rs`, `crates/code-kb-core/tests/references_test.rs` | Yes | Tests scan with the Task 6 extractor build through `JULIE_EXTRACT_BIN`. |
| Task 8: code-kb pin bump and release prep | None - serial | code-kb: `scripts/julie-pins.json`, `CLAUDE.md`, `AGENTS.md`, `docs/release-notes/v1.1.1.md`, `crates/*/Cargo.toml`, `Cargo.lock` | Yes | Needs the julie 3.1.1 archives published (approval boundary). |

---

### Task 1: Shared chain helper and the C# fix

**Files:**
- Modify: `crates/julie-extractors/src/base/relationship_resolution.rs:15-51` (add two constructors next to `UnresolvedTarget::simple`)
- Modify: `crates/julie-extractors/src/csharp/relationships.rs:565-620` (`unresolved_call_target_at_depth`)
- Test: `crates/julie-extractors/src/tests/csharp/cross_file_pending.rs`
- Test: unit tests for the two constructors in `crates/julie-extractors/src/base/relationship_resolution.rs` (a `#[cfg(test)] mod` at the end of the file)

**Interfaces:**
- Consumes: `UnresolvedTarget { display_name, terminal_name, receiver, namespace_path, import_context }` and `UnresolvedTarget::simple`.
- Produces:
  - `impl UnresolvedTarget { pub fn from_chain(parts: Vec<String>) -> Self }`: one part gives `simple(part)`; two or more parts give the Global Constraints shape; an empty vector gives `simple("")`.
  - `impl UnresolvedTarget { pub fn from_qualified_text(text: &str, separators: &[&str]) -> Option<Self> }`: splits `text` on every listed separator, trims each part, returns `None` when any part is empty or is not a plain identifier per the Global Constraints, otherwise returns `from_chain(parts)`.

**Contract inputs:** Global Constraints target shape. The C# tree for `A.B.C(...)` is `invocation_expression(function: member_access_expression(expression: member_access_expression(expression: identifier A, name: identifier B), name: identifier C))`. The existing test file `crates/julie-extractors/src/tests/csharp/cross_file_relationships.rs:130-155` shows how to reach `results.structured_pending_relationships` and assert `target.receiver`.

**File ownership:** julie: `crates/julie-extractors/src/base/relationship_resolution.rs`, `crates/julie-extractors/src/csharp/relationships.rs`, `crates/julie-extractors/src/tests/csharp/cross_file_pending.rs`

**Serialization required:** Yes

**Dependency reason:** Locks the `from_chain` / `from_qualified_text` contract every later julie task calls.

**What to build:** The two constructors, with unit tests, and the C# chain fix that uses `from_chain`. `unresolved_call_target_at_depth` today collects only direct `identifier` children of one `member_access_expression`, so a nested chain yields one identifier and falls back to a bare name. Replace the flat scan with a recursive walk that descends into the `expression` child while it is a `member_access_expression` (respecting `should_visit_tree_depth` / `child_tree_depth` as the function already does), collects identifiers in source order, and calls `from_chain`. A `generic_name` child (`Foo<T>.Bar()`) contributes its identifier child. `this`/`base` receivers keep their current handling. Keep the receiver-type lookup (`self_receiver_type`) untouched.

**Approach:** Write the C# test first: extract `class Caller { void Run() { EmailSettingQueryRepository.EmailSettingPlaceholderGenerators.AnnualAttestationNotification("u", "n"); } }` and assert one structured pending target with `terminal_name == "AnnualAttestationNotification"`, `receiver == Some("EmailSettingPlaceholderGenerators")`, `namespace_path == ["EmailSettingQueryRepository"]`, `display_name == "EmailSettingQueryRepository.EmailSettingPlaceholderGenerators.AnnualAttestationNotification"`. Add a second assertion in the same test that a two-part call `Helper.Process()` in the same snippet still has receiver `Helper` and an empty namespace. Unit tests for `from_qualified_text`: `"Outer.Inner"` with `["."]` gives receiver `Inner`, namespace `["Outer"]`; `"a::b->c"` with `["::", "->"]` gives three parts; `"foo().bar"` gives `None`; `"$outer.inner"` gives two parts with `$outer` kept.

**Acceptance criteria:**
- [x] `cargo test -p julie-extractors base::relationship_resolution` passes with the new constructor tests.
- [x] `cargo test -p julie-extractors tests::csharp::cross_file_pending` passes with the new chain test, and the test fails on the unmodified extractor.
- [x] `cargo xtask test language csharp` passes (unit tier; golden fixture regeneration deferred to Task 6).
- [ ] Worker-scope verification passes and the change is committed by the worker on `qualified-call-chain`.

### Task 2: Java, Kotlin, Go, PowerShell chains

**Files:**
- Modify: `crates/julie-extractors/src/java/relationships.rs:417-449` (`unresolved_call_target`)
- Modify: `crates/julie-extractors/src/kotlin/relationships.rs:441-490` (`unresolved_call_target`)
- Modify: `crates/julie-extractors/src/go/relationships.rs:160-200` (the `selector_expression` arm that builds `UnresolvedTarget { receiver: Some(receiver) ... }`)
- Modify: `crates/julie-extractors/src/powershell/relationships.rs:150-180` (the member-call site that builds `UnresolvedTarget::simple(method_name)`)
- Test: `crates/julie-extractors/src/tests/{java,kotlin,go,powershell}/cross_file_pending.rs`

**Interfaces:**
- Consumes: `UnresolvedTarget::from_chain(Vec<String>)` and `UnresolvedTarget::from_qualified_text(&str, &[&str])` from Task 1.
- Produces: pending targets in the Global Constraints shape for these four languages.

**Contract inputs:** Survey results with julie 3.1.0: Java `Outer.Inner.chain()` gives terminal only; Kotlin same; Go `outer.Inner.Chain()` gives terminal only; PowerShell `[Outer]::Inner.Chain()`, `[Two]::Part()`, and `$outer.inner.Chain2()` all give terminal only. Two-part Java/Kotlin/Go calls already give receiver + empty namespace and must keep doing so.

**File ownership:** julie: `crates/julie-extractors/src/{java,kotlin,go,powershell}/relationships.rs`, `crates/julie-extractors/src/tests/{java,kotlin,go,powershell}/cross_file_pending.rs`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**What to build:** Java and Kotlin: replace the one-level child scan with a recursive collection over nested `method_invocation`/`field_access`/`scoped_identifier` (Java) and `navigation_expression` (Kotlin) nodes, in source order, then `from_chain`. Go: the `selector_expression` `operand` may itself be a `selector_expression`; collect the identifier chain recursively and use `from_chain`. PowerShell: for `member_access`/`invocation_member` expressions collect the chain of type literal (`[Outer]` contributes `Outer`), member names, and variables (`$outer` contributes `$outer`), then `from_chain`; a bare command stays `simple`.

**Approach:** One test per language in the owned `cross_file_pending.rs` named `qualified_chain_call_keeps_receiver_and_namespace`, asserting terminal, receiver, namespace, and display name for a three-part call and receiver + empty namespace for a two-part call in the same snippet. Read the existing tests in that file for the parser and extraction helpers the language uses. PowerShell asserts `[Outer]::Inner.Chain()` gives receiver `Inner`, namespace `["Outer"]` and `[Two]::Part()` gives receiver `Two`.

**Acceptance criteria:**
- [x] Each new test fails on the unmodified extractor and passes after the change.
- [x] `cargo xtask test language java`, `kotlin`, `go`, `powershell` all pass.
- [x] Worker-scope verification passes and the change is committed by the worker on `qualified-call-chain`.

### Task 3: C++, Zig, JavaScript, TypeScript chains

**Files:**
- Modify: `crates/julie-extractors/src/cpp/relationships.rs:143-225` (`extract_call_relationships` target match)
- Modify: `crates/julie-extractors/src/zig/relationships.rs:190-220` (the site that builds `UnresolvedTarget { receiver: Some(receiver) ... }`)
- Modify: `crates/julie-extractors/src/javascript/mod.rs:94-170,261-300` (`walk_for_pending_calls`, `call_terminal_name`, `should_emit_pending_call`, `build_unresolved_target`) and `crates/julie-extractors/src/javascript/relationships.rs` if the chain walk is shared there
- Modify: `crates/julie-extractors/src/typescript/relationships.rs:249-275` (`extract_call_target`)
- Test: `crates/julie-extractors/src/tests/{cpp,zig,javascript,typescript}/cross_file_pending.rs`

**Interfaces:**
- Consumes: Task 1 constructors.
- Produces: pending targets in the Global Constraints shape, and a pending row for chains that emitted nothing before.

**Contract inputs:** Survey results with julie 3.1.0: C++ `Two::part()` gives terminal only and `Outer::Inner::chain()` emits no pending row (the `function` node is a `qualified_identifier`, which falls to the `_` arm and takes the first identifier only); `outer.inner.chain2()` gives compound receiver `outer.inner`. Zig `Two.part()` works, `Outer.Inner.chain()` emits no row. JavaScript and TypeScript `Two.part()` works; `Outer.Inner.chain()` emits no pending row and no call identifier (only a `member_access` identifier for `Inner`). TypeScript `extract_call_target` takes the whole `object` node text as the receiver.

**File ownership:** julie: `crates/julie-extractors/src/cpp/relationships.rs`, `crates/julie-extractors/src/zig/relationships.rs`, `crates/julie-extractors/src/javascript/mod.rs`, `crates/julie-extractors/src/javascript/relationships.rs`, `crates/julie-extractors/src/typescript/relationships.rs`, `crates/julie-extractors/src/tests/{cpp,zig,javascript,typescript}/cross_file_pending.rs`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**What to build:** C++: add a `qualified_identifier` arm that collects the scope chain (`namespace_identifier`/`identifier` parts through nested `qualified_identifier`) and calls `from_chain`; for `field_expression`/`pointer_expression` build the chain with `from_qualified_text(expression_text, &[".", "->", "::"])` and fall back to today's behavior when it returns `None`. Zig: walk nested `field_expression` nodes to collect the chain and call `from_chain`; the chain must produce a row. JavaScript and TypeScript: when the callee is a `member_expression` whose `object` is itself a `member_expression` (or `identifier`), collect the identifier chain and call `from_chain`; anything else (call results, `this`, computed members) keeps today's behavior. Find why the JavaScript walker drops the chain today (`call_terminal_name` / `should_emit_pending_call`) and make the chain emit a row.

**Approach:** Same test naming and assertions as Task 2. For C++ assert both `Outer::Inner::chain()` (receiver `Inner`, namespace `["Outer"]`) and `Two::part()` (receiver `Two`). For JavaScript and TypeScript assert `Outer.Inner.chain()` emits a pending row and `Two.part()` keeps receiver `Two`. Check `is_ecmascript_global_direct_target` and `should_emit_pending_call` so a chain rooted at a known global (`console.log`, `Math.max`) stays excluded as today.

**Acceptance criteria:**
- [x] Each new test fails on the unmodified extractor and passes after the change.
- [x] `cargo xtask test language cpp`, `zig`, `javascript`, `typescript` all pass.
- [x] Worker-scope verification passes and the change is committed by the worker on `qualified-call-chain`.

### Task 4: Python, Swift, Dart, Lua, GDScript compound receivers

**Files:**
- Modify: `crates/julie-extractors/src/python/relationships.rs:215-245` (`extract_target_from_call`, `attribute` arm)
- Modify: `crates/julie-extractors/src/swift/relationships.rs:427-445` (`unresolved_call_target`, the `rsplit_once('.')` branch)
- Modify: `crates/julie-extractors/src/dart/pending_calls.rs:55-70,180-205` (every `receiver: Some(...)` site)
- Modify: `crates/julie-extractors/src/lua/relationships.rs:65-85` and `crates/julie-extractors/src/lua/core.rs:125-145`
- Modify: `crates/julie-extractors/src/gdscript/relationships.rs:285-380` (every `rsplit_once('.')` receiver site)
- Test: `crates/julie-extractors/src/tests/{python,swift,dart,lua,gdscript}/cross_file_pending.rs`

**Interfaces:**
- Consumes: Task 1 constructors.
- Produces: pending targets in the Global Constraints shape.

**Contract inputs:** Survey results with julie 3.1.0: all five give a compound receiver string `Outer.Inner` for `Outer.Inner.chain()`. Python also passes a `receiver_type` alongside the target (`self`/`cls` handling) that must stay unchanged.

**File ownership:** julie: `crates/julie-extractors/src/python/relationships.rs`, `crates/julie-extractors/src/swift/relationships.rs`, `crates/julie-extractors/src/dart/pending_calls.rs`, `crates/julie-extractors/src/lua/relationships.rs`, `crates/julie-extractors/src/lua/core.rs`, `crates/julie-extractors/src/gdscript/relationships.rs`, `crates/julie-extractors/src/tests/{python,swift,dart,lua,gdscript}/cross_file_pending.rs`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**What to build:** At each site that builds a target from `"{receiver}.{terminal}"` text, first try `UnresolvedTarget::from_qualified_text(&format!("{receiver}.{terminal_name}"), &["."])`; when it returns `Some`, use it; otherwise keep the existing target unchanged. Lua also uses `:` for method calls (`obj:method()`); treat `:` like `.` only for the final separator, which the existing code already isolates.

**Approach:** Same test naming and assertions as Task 2. Python test: `Outer.Inner.chain()` gives receiver `Inner`, namespace `["Outer"]`; `self.repo.save()` gives receiver `repo`, namespace `["self"]`, and the structured pending `receiver_type` is unchanged from today for `self.method()`. Swift, Dart, Lua, GDScript: three-part and two-part assertions.

**Acceptance criteria:**
- [x] Each new test fails on the unmodified extractor and passes after the change.
- [x] `cargo xtask test language python`, `swift`, `dart`, `lua`, `gdscript` all pass.
- [x] Worker-scope verification passes and the change is committed by the worker on `qualified-call-chain`.

### Task 5: C, PHP, Ruby, R compound receivers

**Files:**
- Modify: `crates/julie-extractors/src/c/relationships.rs:140-165`
- Modify: `crates/julie-extractors/src/php/call_relationships.rs:263-340` (`unresolved_call_target` and the receiver-target merge at 319-334)
- Modify: `crates/julie-extractors/src/ruby/relationships.rs:390-410` (the `rsplit_once('.')` branch; keep the `::` constant-path branch at 370-385 that already fills `namespace_path`)
- Modify: `crates/julie-extractors/src/r/relationships.rs:290-300,410-420` (receiver sites)
- Test: `crates/julie-extractors/src/tests/{c,php,ruby,r}/cross_file_pending.rs`

**Interfaces:**
- Consumes: Task 1 constructors.
- Produces: pending targets in the Global Constraints shape.

**Contract inputs:** Survey results with julie 3.1.0: C `outer.inner.chain()` gives receiver `outer.inner`; PHP `Outer::Inner::chain()` gives receiver `Outer::Inner` and `$outer->inner->chain2()` gives `outer->inner`; Ruby `Outer::Inner.chain` gives `Outer::Inner`; R `Outer$Inner$chain()` gives `Outer$Inner`.

**File ownership:** julie: `crates/julie-extractors/src/c/relationships.rs`, `crates/julie-extractors/src/php/call_relationships.rs`, `crates/julie-extractors/src/ruby/relationships.rs`, `crates/julie-extractors/src/r/relationships.rs`, `crates/julie-extractors/src/tests/{c,php,ruby,r}/cross_file_pending.rs`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**What to build:** Same pattern as Task 4 with the language separators from Global Constraints: C `[".", "->"]`, PHP `["::", "->", "."]`, Ruby `["::", "."]`, R `["$", "@"]` only (R method names legitimately contain `.`, such as `print.default`, so `.` is never a separator in R).

**Approach:** Same test naming and assertions as Task 2. PHP asserts both the static chain and the `->` chain; the `$` prefix on `$outer` is kept in the namespace part. R asserts `Outer$Inner$chain()` gives receiver `Inner`, namespace `["Outer"]`, and `print.default(x)` stays a simple target.

**Acceptance criteria:**
- [x] Each new test fails on the unmodified extractor and passes after the change.
- [x] `cargo xtask test language c`, `php`, `ruby`, `r` all pass.
- [x] Worker-scope verification passes and the change is committed by the worker on `qualified-call-chain`.

### Task 6: julie branch gate, fixture review, version 3.1.1

**Files:**
- Modify: `fixtures/**` rows the golden or capability tiers report as changed, only when the diff is the accepted receiver split or a newly emitted chain row
- Modify: `crates/julie-extract-artifact/Cargo.toml`, `crates/julie-extractors/Cargo.toml`, `crates/julie-extract-cli/Cargo.toml`, `Cargo.lock` (version `3.1.1`)
- Create: `docs/release-notes/v3.1.1.md` (same layout as `docs/release-notes/v3.1.0.md`)
- Modify: `docs/contracts/extracted-data-v3.md:42` (one sentence stating the receiver is the last qualifier and the namespace path holds the rest)

**Interfaces:**
- Consumes: the merged Tasks 1-5 on `qualified-call-chain`.
- Produces: a green branch at version 3.1.1, ready to tag.

**Contract inputs:** `docs/release.md` branch gates and `cargo xtask release preflight --version 3.1.1`.

**File ownership:** julie: `fixtures/**` rows that legitimately change, `crates/*/Cargo.toml`, `Cargo.lock`, `docs/release-notes/v3.1.1.md`, `docs/contracts/extracted-data-v3.md`

**Serialization required:** Yes

**Dependency reason:** Needs Tasks 1-5 merged into the branch.

**What to build:** Run the julie branch gate from the Verification Strategy. Review every fixture diff against the hard-gate rule; regenerate only accepted rows. Bump the version, write the release note (what changed, which languages, the target shape), run `cargo deny check` and `cargo xtask release preflight --version 3.1.1`. Commit. Do not tag, push, or publish: that is an approval boundary reported to the user.

**Acceptance criteria:**
- [x] `cargo fmt --check`, `cargo test -p xtask`, `cargo xtask test default`, `cargo xtask test contract`, `cargo xtask test capability`, `cargo xtask test golden`, `cargo deny check` all pass on the branch head.
- [x] `cargo xtask release preflight --version 3.1.1` passes.
- [x] Scanning the survey fixtures from the diagnosis (scratchpad `langs/` and `e2e/`) with the branch build shows receiver `Inner` and namespace `["Outer"]` for every language in Tasks 1-5.
- [x] Worker-scope verification passes and the change is committed on `qualified-call-chain`.

### Task 7: code-kb accepts ancestor namespaces and honours identifier receivers

**Files:**
- Modify: `crates/code-kb-core/src/queries.rs:1048-1102` (`pending_target_predicate`)
- Modify: `crates/code-kb-core/src/queries.rs:1324-1355` (the `identifiers` fallback query inside `find_references_internal`)
- Test: `crates/code-kb-core/tests/references_test.rs`

**Interfaces:**
- Consumes: pending rows in the Global Constraints shape from the Task 6 extractor build, reached through `JULIE_EXTRACT_BIN=/home/murphy/source/julie-extractors/target/release/julie-extract` (build it with `cargo build --release -p julie-extract-cli` in the julie worktree with the shared `CARGO_TARGET_DIR`). `identifiers.metadata_json` carries `{"receiver": "...", "receiver_qualifier": "..."}` for member accesses.
- Produces: `find_references_scoped(conn, name, "callers", ...)` returns every chained call site of a nested target.

**Contract inputs:** Verified with code-kb 1.1.0: a Scala nested target whose outer object lives in `Defs.scala` returns 0 callers because the namespace check in `pending_target_predicate` only accepts a namespace segment that appears in the target's file path. The C# collision repro returns the string constant's `member_access` identifier as a caller of the method.

**File ownership:** code-kb: `crates/code-kb-core/src/queries.rs`, `crates/code-kb-core/tests/references_test.rs`

**Serialization required:** Yes

**Dependency reason:** Tests scan with the Task 6 extractor build through `JULIE_EXTRACT_BIN`.

**What to build:** (1) In the first branch of `pending_target_predicate`, extend the `NOT IN` allow-list of the `NOT EXISTS` clause so a namespace value is also accepted when it equals the name of any ancestor of the target symbol (walk `parent_symbol_id` with a recursive CTE inside the subquery). (2) In the identifier fallback, when the resolved target has a parent, exclude rows whose `json_extract(i.metadata_json, '$.receiver')` is non-null and differs from the parent's name. Rows without receiver metadata keep matching by name. Keep both queries indexed on `identifiers(name, kind)` and `symbols(symbol_id)`.

**Approach:** Tests use the existing `scanned_repo` helper in `references_test.rs`. Test A (C#): `Outer.cs` declares `class Outer { static class Inner { static int Chain() } }`, `Caller.cs` calls `Outer.Inner.Chain()`; callers of `Chain` include `Call`. Test B (Scala): the outer object in `Defs.scala`, caller in `Caller.scala`; callers of `chain` include `caller`. Test C (C#): the collision repro from the report (constant and method sharing a name in sibling nested classes, three callers of the method); callers of `EmailSettingPlaceholderGenerators.AnnualAttestationNotification` are exactly the three method call sites and never the constant's line; callers of `EmailSettingNames.AnnualAttestationNotification` are exactly the constant's usage. Write the tests first and confirm A and C fail on the unmodified code-kb with the new extractor.

**Acceptance criteria:**
- [x] Tests A, B, and C pass; A and C fail before the change.
- [x] `cargo test -p code-kb-core` and `cargo test -p code-kb-cli` pass with `JULIE_EXTRACT_BIN` pointing at the 3.1.1 build.
- [x] `code-kb refs AnnualAttestationNotification` on the report repro (scratchpad `repro/`) lists three `calls` rows and no `member_access` row.
- [x] Worker-scope verification passes and the change is committed on the code-kb task branch.

### Task 8: code-kb pin bump and release preparation

**Files:**
- Modify: `scripts/julie-pins.json` (version `3.1.1` and the four sha256 values from the published `.sha256` sidecars)
- Modify: `CLAUDE.md` and `AGENTS.md` (the "currently `3.1.0`" text under Pinned Extractor)
- Modify: `crates/*/Cargo.toml`, `Cargo.lock` (code-kb `1.1.1`)
- Create: `docs/release-notes/v1.1.1.md` (same layout as `docs/release-notes/v1.1.0.md`)

**Interfaces:**
- Consumes: the published julie `v3.1.1` GitHub release archives.
- Produces: a code-kb branch whose build guard accepts only julie 3.1.1, ready for the release steps in `docs/RELEASING.md`.

**Contract inputs:** `docs/RELEASING.md`, `scripts/release-preflight.sh`, `crates/code-kb-cli/build.rs` pin guard, `tests/plugin/*.test.cjs` version consistency checks.

**File ownership:** code-kb: `scripts/julie-pins.json`, `CLAUDE.md`, `AGENTS.md`, `docs/release-notes/v1.1.1.md`, `crates/*/Cargo.toml`, `Cargo.lock`

**Serialization required:** Yes

**Dependency reason:** Needs the julie 3.1.1 archives published (approval boundary).

**What to build:** After the user approves and performs (or authorizes) the julie 3.1.1 tag, push, and release, fetch the four archive checksums, update the pin file, restore the extractor, run `cargo build --release`, `cargo test --workspace`, `node --test tests/plugin/*.test.cjs`, and `scripts/release-preflight.sh`. Write the release note. Commit. Tag, push, and publish only with explicit user approval.

**Acceptance criteria:**
- [ ] `cargo build --release` passes the build guard with the 3.1.1 extractor restored.
- [ ] `cargo test --workspace` and `node --test tests/plugin/*.test.cjs` pass.
- [ ] `scripts/release-preflight.sh` passes.
- [ ] `CLAUDE.md` and `AGENTS.md` are byte-identical.
- [ ] Worker-scope verification passes and the change is committed on the code-kb task branch.
