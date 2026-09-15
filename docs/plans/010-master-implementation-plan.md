# code-kb: Master Implementation Plan

## 1. Project Objective

Build `code-kb`, a lightweight, blazing-fast (<15 MB, <5ms query latency) Rust code-intelligence engine and Model Context Protocol (MCP) server for AI coding agents. `code-kb` queries the rich AST artifacts produced by `julie-extractors` (SQLite schema v7 and Family Store v2) to provide **progressive code disclosure**, **semantic symbol navigation**, and **surgical context slicing**—reducing agent token consumption by 80–90% and eliminating grep/view_file thrashing.

---

## 2. Architectural Guardrails (Learned from Miller)

1. **Pure Consumer Boundary:** `code-kb` does NOT parse code or own tree-sitter grammars directly. It invokes or queries `julie-extract` artifacts.
2. **Zero In-Memory Heap Graphs:** Do NOT hydrate 200,000 symbols into RAM. Query SQLite directly using indexes and WAL mode. Keep memory under 15 MB.
3. **1:1 Session-to-Workspace Binding:** Zero `workspace_id` parameter friction for agents. One process = one active workspace session.
4. **CLI-First Architecture:** Every MCP tool MUST have an identical 1:1 CLI command (`code-kb skeleton`, `code-kb symbol`, etc.) for instant 2ms testing and dogfooding without GUI client restarts.
5. **Atomic 1-Turn Edits:** `replace_symbol_body` validates syntax via tree-sitter, checks `body_hash`, and writes in a single turn (no 2-step preview/apply roundtrips).
6. **Stateless MCP 2.0 Ready:** Support pure stateless tool requests over `stdio`.

---

## 3. Implementation Roadmap

### Phase 1: Workspace Scaffolding & Core Architecture
- [x] Initialize Cargo workspace in `c:\source\code-kb`:
  - `crates/code-kb-core`: Library (storage reader, query-time resolver, byte-slicer, token-minimized formatters).
  - `crates/code-kb-cli`: Binary (`code-kb` CLI interface and `code-kb serve` MCP server).
- [x] Configure dependencies:
  - `rusqlite` (with bundled/WAL, fast reads)
  - `clap` (derive-based CLI parser)
  - `serde`, `serde_json`
  - `ignore` (for `.gitignore` traversal)
  - `notify` (debounced file watching)
  - `sha2` (content hashing)
  - `insta` (snapshot testing for token output formatting)
- [x] Implement Root & Workspace Discovery:
  - `--root <path>` CLI argument
  - Upward Git root / Git worktree traversal (`.git` file pointer detection)
  - Path canonicalization & Windows verbatim prefix stripping (`\\?\`)

### Phase 2: SQLite Storage & Query Engine (`code-kb-core`)
- [x] Database Connection Layer:
  - Read-only connection to SQLite artifact (`PRAGMA query_only = ON; PRAGMA journal_mode = WAL;`).
  - Support both Standalone Artifact (`artifact.db`) and Versioned Family Store views (`store.db`).
- [x] Fact Query Layer:
  - `load_file_symbols(path)`: Query `symbols` table for line ranges, body spans, docstrings, signatures, and visibility.
  - `search_symbols(query, kind, is_test)`: Indexed query on `symbols(name, kind)` with `is_test` filtering.
  - `find_structural_facts(category)`: Query `structural_facts` (routes, tables) and `literals` (endpoints, config keys).
- [x] Query-Time Reference Resolution:
  - Implement fast query-time resolution matching `pending_relationships.target_terminal_name` & `relationships` to target symbols.
  - `find_callers(symbol_name)` & `find_callees(symbol_name)`.

### Phase 3: Source Slicing & Progressive Disclosure Serializers
- [x] Byte-Range Slicer:
  - Direct file slicing using `start_byte..end_byte` and `body_start_byte..body_end_byte`.
  - Fallback line slicer with bounds validation.
- [x] Token-Minimized Output Serializers:
  - **`file_skeleton` serializer:** Formats types, traits, functions, visibility, and docstrings, with hidden body markers (e.g. `/* 45 lines hidden: L12-L57 */`).
  - **`codebase_outline` serializer:** Compact module tree with entry points.
  - **`get_context_slice` serializer:** Dense markdown bundle combining target body + callee signatures + parameter types + related tests.
- [x] Snapshot Tests:
  - Golden tests verifying token counts and formatting across C#, Rust, TypeScript, and Python fixtures.

### Phase 4: CLI Implementation (`code-kb-cli`)
- [x] Implement CLI subcommands mirroring every core tool:
  - `code-kb outline [path] [--depth <n>]`
  - `code-kb skeleton <file>`
  - `code-kb symbol <query> [--kind <k>] [--include-tests]`
  - `code-kb body <symbol> [--file <path>]`
  - `code-kb slice <symbol> [--depth <n>]`
  - `code-kb refs <symbol> --direction <callers|callees>`
  - `code-kb facts <category>`
  - `code-kb edit <symbol> --body <body> [--file <path>]`
- [x] Add human-friendly terminal formatting (with `--json` flag for machine consumption).

### Phase 5: File Synchronization & Staleness Guards
- [x] Tier 1: Synchronous Tool-Driven Sync
  - Trigger `julie-extract update --file <path>` immediately upon code mutations.
- [x] Tier 2: Just-In-Time (JIT) Staleness Guard
  - Stat-check target file on disk (`mtime`, `bytes`) before answering `file_skeleton` or `get_symbol_body`.
  - If dirty, trigger `julie-extract update --file <path>` in <5ms before generating response.
- [x] Tier 3: Background File Watcher
  - Background `notify` watcher thread with `.gitignore` filtering.
  - 150ms debounce window.
  - Git storm circuit-breaker (>50 events in 500ms triggers single scan instead of micro-updates).
- [x] Cold-Start Reconciliation:
  - Background metadata sweep on startup comparing dirents against `files.indexed_at`.

### Phase 6: Model Context Protocol (MCP) Server (`code-kb serve`)
- [x] Implement JSON-RPC stdio transport.
- [x] Support both MCP 1.x handshake (`initialize` with `roots`) and MCP 2.0 (2026-07-28 stateless requests in `_meta`).
- [x] Register the 7 Core Read Tools:
  - `codebase_outline`
  - `file_skeleton`
  - `find_symbol`
  - `get_symbol_body`
  - `get_context_slice`
  - `find_references`
  - `find_structural_facts`
- [x] Register the AST-Guided Edit Tool:
  - `replace_symbol_body` (with optimistic lock `body_hash` and tree-sitter preflight syntax check).

### Phase 7: Verification, Dogfooding & Benchmarks
- [x] Benchmark token consumption on real tasks (comparing bare agent vs `code-kb`).
- [x] Verify Windows compatibility (handle management, path separators, WAL concurrency).
- [x] Ship unified statically compiled executable.
