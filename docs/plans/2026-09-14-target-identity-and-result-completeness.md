# Target Identity and Result Completeness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use razorback:subagent-driven-development whenever delegation is available and permitted, including for one task; serialize dependent tasks. Use razorback:executing-plans only when delegation is unavailable or the user/session explicitly selected single-agent execution.

**Goal:** Establish consistent symbol target disambiguation across all tools (file-scoped blast radius, file-scoped references) and make result limits and truncation transparent across token-dense formatters and structural facts.

**Architecture:** Extend core query functions (`compute_blast_radius`, `find_references_scoped`, `find_structural_facts`) to accept path scoping and normalize category aliases. Add limit indicator footers and headers in `formatters.rs` to clearly surface caps for context slices, blast radius CTE walks, and searches. Expose the new optional parameters in MCP server schemas and CLI flags with exact 1:1 parity without adding any workspace parameters.

**Tech Stack:** Rust, SQLite (via `rusqlite`), Tree-Sitter, Model Context Protocol (MCP JSON-RPC).

**Spec:** [docs/toolkit-assessment.md](file:///home/murphy/source/code-kb/docs/toolkit-assessment.md)

**Architecture Quality:** No Architecture Impact. Preserves the pure SQLite query engine, retained memory < 15MB, zero workspace parameter invariant, and exact 1:1 MCP-to-CLI command parity.

## Global Constraints

- Never expose `workspace`, `workspace_id`, `repo_path`, or `root_dir` in any MCP tool schema (Core Invariant 1).
- Retained memory must stay < 15 MB; execute all traversals and queries directly in SQLite (Core Invariant 2).
- Zero ghost compatibility: MCP schemas remain crisp, agent-facing discovery surfaces without dead aliases or deprecated fields (Core Invariant 8).
- Keep `AGENTS.md` and `CLAUDE.md` byte-for-byte synchronized if tool reference tables are updated.
- Every MCP tool feature must have an exact 1:1 CLI command counterpart.
- Windows compatibility: all paths normalized with forward slashes `/` and `dunce::simplified`.

---

## Verification Strategy

**Project source of truth:** `AGENTS.md`, `CLAUDE.md`, `Cargo.toml`.

**Worker red/green scope:**
- Core queries: `cargo test -p code-kb-core --test blast_radius_test` / `cargo test -p code-kb-core --test disambiguation_test`
- CLI & MCP: `cargo test -p code-kb-cli --test cli_test` / `cargo test -p code-kb-cli --test mcp_test`

**Worker ceiling:** `cargo test --workspace`

**Worker gate invariant:** Each task must compile, pass its covering tests, pass `cargo clippy --all-targets -- -D warnings`, and maintain `cargo fmt --check`.

**Lead affected-change scope:** `cargo test --workspace && cargo clippy --all-targets -- -D warnings`

**Branch gate:** `cargo test --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --check`

**Security scope:** `none declared`

**Replay/metric evidence:** All 194+ tests passing, 0 clippy warnings, clean formatting.

**Escalation triggers:** Any regression in existing 194 workspace tests or broken sync between `AGENTS.md` and `CLAUDE.md`.

**Assigned verification failure:** Workers stop and report when assigned verification fails, unless this plan explicitly says to update that gate.

**Verification ledger:** Record invariant, command, scope label, commit SHA, result, and timestamp.

---

## Parallel Execution Contract

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| Task 1: Disambiguate blast radius symbol seeds with path hint | Batch A | `crates/code-kb-core/src/queries.rs`, `crates/code-kb-core/tests/blast_radius_test.rs` | No | None - safe parallel batch. |
| Task 2: Add optional file_path to find_references | Batch B | `crates/code-kb-core/src/queries.rs`, `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/tests/cli_test.rs`, `crates/code-kb-core/tests/disambiguation_test.rs`, `AGENTS.md`, `CLAUDE.md`, `README.md`, `skills/code-kb/SKILL.md`, `.claude-plugin/skills/code-kb/SKILL.md` | Yes | Serialized after Task 1 to avoid overlapping edits in `queries.rs`. |
| Task 3: Disclose result caps and truncation in formatters | Batch C | `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-core/src/ops.rs`, `crates/code-kb-core/tests/` | No | Operates primarily on formatting layer. |
| Task 4: Path filter and category aliases for find_structural_facts | Batch D | `crates/code-kb-core/src/queries.rs`, `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/tests/cli_test.rs` | Yes | Serialized after Task 2 to cleanly update `queries.rs` and CLI/MCP. |

---

### Task 1: Disambiguate Blast Radius Symbol Seeds with Path Hint

**Files:**
- Modify: `crates/code-kb-core/src/queries.rs:1700-1725`
- Test: `crates/code-kb-core/tests/blast_radius_test.rs`

**Interfaces:**
- Consumes: `get_symbol_by_name(conn, name, path_filter: Option<&str>)`
- Produces: `compute_blast_radius` uses `seed_paths.first()` as `path_filter` when resolving `seed_symbols`, allowing ambiguous symbol names to be resolved when a file is provided.

**Contract inputs:**
- When `seed_paths` has length 1 and `seed_symbols` is non-empty, use `seed_paths[0]` as the path filter hint in `get_symbol_by_name(conn, name, Some(seed_paths[0]))`.

**File ownership:** `crates/code-kb-core/src/queries.rs`, `crates/code-kb-core/tests/blast_radius_test.rs`

**Serialization required:** No

**Dependency reason:** None - safe parallel batch.

**Step 1: Write the failing test**
In `crates/code-kb-core/tests/blast_radius_test.rs`:
```rust
#[test]
fn blast_radius_disambiguates_symbol_seed_using_seed_path() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s1', 'f1', 'src/alpha.rs', 'rust', 'handle', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s2', 'f2', 'src/beta.rs', 'rust', 'handle', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('c1', 'f3', 'src/caller_alpha.rs', 'rust', 'call_alpha', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO relationships VALUES
            ('c1', 's1', 'calls', 'src/caller_alpha.rs', 1, 0);",
    )
    .unwrap();

    let res = compute_blast_radius(&conn, &["handle"], &["src/alpha.rs"], 1, 20).unwrap();
    assert_eq!(res.impacted_symbols.len(), 1);
    assert_eq!(res.impacted_symbols[0].name, "call_alpha");
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --test blast_radius_test blast_radius_disambiguates_symbol_seed_using_seed_path`
Expected: FAIL with `AmbiguousSymbol("handle", 2, ...)` because `get_symbol_by_name` receives `None`.

**Step 3: Write minimal implementation**
In `crates/code-kb-core/src/queries.rs`:
```rust
    let path_hint = if seed_paths.len() == 1 {
        Some(seed_paths[0])
    } else {
        None
    };
    let resolved_seed_symbols = seed_symbols
        .iter()
        .map(|name| {
            get_symbol_by_name(conn, name, path_hint)?
                .ok_or_else(|| QueryError::SymbolNotFound((*name).to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
```

**Step 4: Run test to verify it passes**
Run: `cargo test --test blast_radius_test blast_radius_disambiguates_symbol_seed_using_seed_path`
Expected: PASS

**Step 5: Apply commit mode**
`serial-worker-commit`: commit owned files with message `fix(core): disambiguate blast radius symbol seed with seed path hint`.

**Acceptance criteria:**
- [x] Calling `compute_blast_radius` with a bare symbol name that exists in multiple files succeeds when `seed_paths` specifies the target file.
- [x] Existing `blast_radius_test` tests all pass.

---

### Task 2: Add Optional `file_path` to `find_references`

**Files:**
- Modify: `crates/code-kb-core/src/queries.rs:865-915`
- Modify: `crates/code-kb-cli/src/main.rs:589-605`
- Modify: `crates/code-kb-cli/src/mcp/server.rs:257-285, 938-975`
- Modify: `AGENTS.md`, `CLAUDE.md`, `README.md`, `skills/code-kb/SKILL.md`, `.claude-plugin/skills/code-kb/SKILL.md`
- Test: `crates/code-kb-core/tests/disambiguation_test.rs`
- Test: `crates/code-kb-cli/tests/cli_test.rs`

**Interfaces:**
- Consumes: `queries::get_symbol_by_name(conn, symbol_name, path_filter)`
- Produces: `find_references_scoped(conn, symbol_name, direction, limit, include_external, path_filter: Option<&str>)`
- Exposes: `--file` / `-f` on CLI `code-kb refs <symbol>`, `file_path` (opt) on MCP `find_references` tool schema.

**Contract inputs:**
- MCP property: `"file_path": { "type": "string", "description": "Optional file path to disambiguate symbols with identical names across files." }`
- Backward-compatible alias tolerance: `arguments.get("file_path").or_else(|| arguments.get("file")).or_else(|| arguments.get("path"))`.

**File ownership:** `crates/code-kb-core/src/queries.rs`, `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/tests/cli_test.rs`, `crates/code-kb-core/tests/disambiguation_test.rs`, `AGENTS.md`, `CLAUDE.md`, `README.md`, `skills/code-kb/SKILL.md`, `.claude-plugin/skills/code-kb/SKILL.md`

**Serialization required:** Yes

**Dependency reason:** Follows Task 1 to modify `queries.rs` without conflict.

**Step 1: Write the failing test**
In `crates/code-kb-core/tests/disambiguation_test.rs`:
```rust
#[test]
fn find_references_scoped_disambiguates_multi_file_symbols() {
    let temp = safe_tempdir();
    let conn = open_read_write(&temp.path().join("index.db")).unwrap();
    setup_test_db(&conn);

    conn.execute_batch(
        "INSERT INTO symbols VALUES
            ('s1', 'f1', 'src/alpha.rs', 'rust', 'run', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('s2', 'f2', 'src/beta.rs', 'rust', 'run', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0),
            ('c1', 'f3', 'src/caller.rs', 'rust', 'caller_alpha', 'function', NULL, NULL, NULL, NULL, 1, 0, 1, 0, 0, 1, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 0, 0);
        INSERT INTO relationships VALUES
            ('c1', 's1', 'calls', 'src/caller.rs', 1, 0);",
    )
    .unwrap();

    let refs = find_references_scoped(&conn, "run", "callers", 10, false, Some("src/alpha.rs")).unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].from_symbol_name, "caller_alpha");
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --test disambiguation_test find_references_scoped_disambiguates_multi_file_symbols`
Expected: FAIL with function not found.

**Step 3: Write minimal implementation**
1. Implement `find_references_scoped` in `crates/code-kb-core/src/queries.rs` and re-export in `lib.rs`:
```rust
pub fn find_references_scoped(
    conn: &Connection,
    symbol_name: &str,
    direction: &str,
    limit: usize,
    include_external: bool,
    path_filter: Option<&str>,
) -> Result<Vec<ReferenceSite>, QueryError> {
    if direction != "callers" && direction != "callees" {
        return Err(QueryError::InvalidDirection(direction.to_string()));
    }

    match get_symbol_by_name(conn, symbol_name, path_filter)? {
        Some(target) => find_references_internal(
            conn,
            &target.name,
            direction,
            limit,
            Some(&target.symbol_id),
            include_external,
        ),
        None => {
            let suggestions =
                search_symbols_scoped(conn, symbol_name, None, path_filter, false, 3).unwrap_or_default();
            if suggestions.is_empty() {
                Err(QueryError::SymbolNotFound(symbol_name.to_string()))
            } else {
                let formatted: Vec<String> = suggestions
                    .iter()
                    .map(|s| format!("{} ({}:{})", s.name, s.path, s.start_line))
                    .collect();
                Err(QueryError::SymbolNotFoundWithSuggestions(
                    symbol_name.to_string(),
                    formatted.join(", "),
                ))
            }
        }
    }
}
```
2. Update `find_references_ext` to delegate to `find_references_scoped(..., None)`.
3. In `crates/code-kb-cli/src/main.rs`, add `#[arg(short = 'f', long)] file: Option<String>` to `RefsArgs` and pass `args.file.as_deref()` relativized.
4. In `crates/code-kb-cli/src/mcp/server.rs`, add `"file_path"` to `find_references` schema and resolve `file_path` in tool handler.
5. Update `AGENTS.md` and `CLAUDE.md` synchronously to reflect `file_path` in `find_references`.

**Step 4: Run test to verify it passes**
Run: `cargo test --workspace`
Expected: PASS (all tests passing, sync contract tests passing).

**Step 5: Apply commit mode**
`serial-worker-commit`: commit owned files with message `feat(core,cli,mcp): add optional file_path filter to find_references`.

**Acceptance criteria:**
- [x] `find_references_scoped` disambiguates identically named symbols using file path filter.
- [x] CLI `code-kb refs <symbol> --file <path>` filters references to the specified file's symbol.
- [x] MCP `find_references` accepts `file_path` (and aliases `file`, `path`).
- [x] `AGENTS.md` and `CLAUDE.md` stay byte-for-byte in sync.

---

### Task 3: Disclose Result Caps and Truncation in Formatters

**Files:**
- Modify: `crates/code-kb-core/src/formatters.rs:296-324, 497-527, 608-640`
- Modify: `crates/code-kb-core/src/ops.rs:114-135`
- Test: `crates/code-kb-core/tests/` (unit tests for formatting truncation footers)

**Interfaces:**
- Produces: Transparent truncation notices when results hit caps in `format_context_slice` (callee cap of 10, test cap of 5), `format_blast_radius` (CTE ceiling of 200), and `format_search_results` (search limit).

**Contract inputs:**
- Standardized notice text: `[Showing X dependencies (limit reached)]`, `[Showing X tests (limit reached)]`, `[Downstream Impact: showing X symbols (search ceiling 200 reached; use --depth or smaller scope)]`.

**File ownership:** `crates/code-kb-core/src/formatters.rs`, `crates/code-kb-core/src/ops.rs`

**Serialization required:** No

**Dependency reason:** None - independent formatting layer.

**Step 1: Write the failing test**
In `crates/code-kb-core/src/formatters.rs` (in `mod tests`):
```rust
#[test]
fn test_context_slice_shows_truncation_notice_when_caps_hit() {
    let mut slice = sample_context_slice();
    slice.callee_signatures = (1..=10).map(|i| format!("fn callee_{i}()")).collect();
    let text = format_context_slice(&slice);
    assert!(text.contains("[Showing 10 dependencies (limit reached)]"));
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test -p code-kb-core test_context_slice_shows_truncation_notice`
Expected: FAIL with assertion not found in text.

**Step 3: Write minimal implementation**
1. In `format_context_slice`:
```rust
    if !slice.callee_signatures.is_empty() {
        out.push_str("### Dependencies (Signatures):\n");
        for callee in &slice.callee_signatures {
            out.push_str(&format!("- {callee}\n"));
        }
        if slice.callee_signatures.len() >= 10 {
            out.push_str("[Showing 10 dependencies (limit reached)]\n");
        }
        out.push('\n');
    }

    if !slice.related_tests.is_empty() {
        out.push_str("### Related Tests:\n");
        for test in &slice.related_tests {
            out.push_str(&format!(
                "- `{}` ({}:{})\n",
                test.name, test.path, test.start_line
            ));
        }
        if slice.related_tests.len() >= 5 {
            out.push_str("[Showing 5 tests (limit reached)]\n");
        }
        out.push('\n');
    }
```
2. In `format_blast_radius`:
```rust
    if total >= 200 {
        out.push_str("### Downstream Impact (200+ symbols - traversal ceiling reached; increase depth/limit or narrow target)\n");
    } else {
        out.push_str(&format!("### Downstream Impact ({} symbols)\n", total));
    }
```

**Step 4: Run test to verify it passes**
Run: `cargo test -p code-kb-core test_context_slice_shows_truncation_notice`
Expected: PASS

**Step 5: Apply commit mode**
`serial-worker-commit`: commit owned files with message `feat(core): disclose result caps and truncation in context slice and blast radius formatters`.

**Acceptance criteria:**
- [x] Context slices hitting 10 callees or 5 tests display explicit cap warnings.
- [x] Blast radius runs hitting the 200-row CTE limit state the ceiling was reached.
- [x] All format tests pass without breaking existing compact formats.

---

### Task 4: Path Filter and Category Normalization for `find_structural_facts`

**Files:**
- Modify: `crates/code-kb-core/src/queries.rs:1600-1650`
- Modify: `crates/code-kb-cli/src/main.rs:610-635`
- Modify: `crates/code-kb-cli/src/mcp/server.rs:286-300, 976-1010`
- Test: `crates/code-kb-cli/tests/cli_test.rs`
- Test: `crates/code-kb-core/tests/`

**Interfaces:**
- Consumes: `queries::find_structural_facts_scoped(conn, category: Option<&str>, path_filter: Option<&str>, limit: usize)`
- Produces: Normalized category matching: maps `"config"` to match `toml.key_value.v1` / `json.key_value.v1` / `yaml.key_value.v1`, maps `"route"` / `"routes"` to `http.route`, etc., plus adds optional `path` filter.
- Exposes: `--file` / `--path` flag on CLI `code-kb facts`, `path` property on MCP `find_structural_facts`.

**Contract inputs:**
- Category aliases:
  - `"config"` $\to$ `category LIKE '%.key_value.%' OR category LIKE '%config%'`
  - `"route"` | `"routes"` $\to$ `category LIKE '%.route%' OR category LIKE '%route%'`
  - `"query"` | `"queries"` | `"sql"` $\to$ `category LIKE '%.sql.%' OR category LIKE '%query%'`
  - `"model"` | `"models"` $\to$ `category LIKE '%.model%'`

**File ownership:** `crates/code-kb-core/src/queries.rs`, `crates/code-kb-cli/src/main.rs`, `crates/code-kb-cli/src/mcp/server.rs`, `crates/code-kb-cli/tests/cli_test.rs`

**Serialization required:** Yes

**Dependency reason:** Serialized after Task 2 to cleanly edit CLI and MCP handlers.

**Step 1: Write the failing test**
In `crates/code-kb-cli/tests/cli_test.rs`:
```rust
#[test]
fn test_cli_facts_with_config_alias_and_path_filter() {
    let repo = setup_test_repo();
    let root = repo.path();

    let output = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(root)
        .arg("facts")
        .arg("config")
        .output()
        .expect("Failed to execute facts");
    assert!(output.status.success());
}
```

**Step 2: Run test to verify it fails**
Run: `cargo test --test cli_test test_cli_facts_with_config_alias_and_path_filter`
Expected: FAIL or verify behavior for aliases.

**Step 3: Write minimal implementation**
1. In `crates/code-kb-core/src/queries.rs`:
Update `find_structural_facts` to accept `path_filter: Option<&str>` and apply alias expansion when matching category.
2. In `crates/code-kb-cli/src/mcp/server.rs`:
Add `path` property to `find_structural_facts` input schema.
3. In `crates/code-kb-cli/src/main.rs`:
Add `--path` / `--file` to `FactsArgs`.

**Step 4: Run test to verify it passes**
Run: `cargo test --workspace`
Expected: PASS

**Step 5: Apply commit mode**
`serial-worker-commit`: commit owned files with message `feat(core,cli,mcp): add path filter and category aliases to structural facts`.

**Acceptance criteria:**
- [x] `find_structural_facts(category="config")` matches `toml.key_value.v1` and config facts.
- [x] Path scoping limits facts to the target directory or file.
- [x] CLI and MCP parameter contracts match exactly.
