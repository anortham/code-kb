# code-kb: Comparative Analysis, Benchmarking Strategy, and Architectural Lessons from Miller & Julie

## 1. Executive Summary

`code-kb` was engineered as the next-generation, agent-facing code-intelligence engine for AI coding agents. It incorporates over 1,000 hours of development, debugging, and empirical agent calibration accumulated across two prior systems:

1. **Miller (`~/source/miller`):** A C# / .NET 10 code-intelligence server with family-store CAS architectures, multi-phase search routes, and extensive agent efficiency calibrations (`PERF.md`, `2026-08-25-miller-vs-bare-agent-v1.22.1-calibration.md`).
2. **Julie (`~/source/julie`):** A Rust code-intelligence platform with Tantivy full-text search indexing, ONNX embedding models (`bge-small-en-v1.5`), and multi-crate modular architectures.

This document details the comparative findings, benchmark evidence, and architectural guardrails that make `code-kb` substantially faster, lighter, and more reliable for LLM agents.

---

## 2. Comparative Matrix: code-kb vs Miller vs Julie

| Architectural Dimension | code-kb | Miller (.NET 10) | Julie (Rust) |
|---|---|---|---|
| **Binary & Packaging** | Single static executable (**13.7 MB**) | 103 MB zip / **325 MB bundle** (web dashboard, Vulkan DLLs, runtime) | Multi-binary bundle (**~120 MB** with Tantivy/ONNX) |
| **Retained Heap RAM** | **< 15 MB** (WAL SQLite with mmap) | **1,500–2,000 MB** (hydrated SymbolGraph & `MillerRepositoryIndex`) | **300–800 MB** (Tantivy segments, vector sidecar) |
| **Startup / First Tool Call** | **< 30 ms** cold, **< 5 ms** warm | **6,800 ms** (loaded 223k symbols on boot; PERF-002) | **500–2,000 ms** (Tantivy index reader warmup) |
| **Tool Execution Latency** | **1–6 ms** (median) | **474–1,938 ms** (PERF-001) | **50–250 ms** |
| **Workspace Parameter Contract** | **ZERO (0)** in tool schemas. Auto-bound via MCP roots / CWD. | **Mandatory `workspace_id`**. Rejected with `ToolDiagnostic.Refusal` if missing. | **Mandatory `workspace`** string in parameters. |
| **Edit Protocol** | **Single-Turn Atomic Edit** (`replace_symbol_body`). Pre-flight tree-sitter validation. | **2-Step Preview Handshake** (`apply=false` then `apply=true`). | **2-Step Preview Handshake** (`dry_run=true` then `false`). |
| **Search Engine** | SQLite **FTS5 BM25 + Porter Stemming** + path prefix scoping. | Complex multi-phase (exact -> trigram -> vector). Prone to noise. | Tantivy BM25 + ONNX embedding sidecar + centrality graph. |
| **File Freshness & Watcher** | In-process notify watcher + mtime/size checks in SQLite (<10ms). | `FreshnessService` rebuilt entire index on every revision (101.5 GB reads; PERF-002). | File watcher + Tantivy commit pipeline. |
| **Worktree Lifecycle** | Per-worktree `.code-kb/artifact.db` (self-cleaning with worktree directory removal; superseded earlier centralized index `code-kb prune` design). | CAS family store with shared blobs, but required manual cache purge. | Multi-workspace index directory. |

---

## 3. The 5 Crucial Lessons Learned

### Lesson 1: Zero Workspace Parameters (Eliminate LLM Prompt Pollution)
- **The Problem:** In Miller and Julie, exposing `workspace` or `workspace_id` caused agents to constantly hallucinate paths, miss slashes on Windows, or get trapped in loop errors (`WORKSPACE_MISSING`). LLMs spent cognitive budget on tracking workspace paths rather than coding.
- **The Solution:** In `code-kb`, **never** expose `workspace`, `workspace_id`, `repo_path`, or `root_dir` in any tool schema. Workspace binding is established automatically at startup via MCP protocol handshake (`params.roots`), project CWD, or path discovery. CI enforcement in `mcp_test.rs` guarantees this invariant cannot regress.

### Lesson 2: Zero Heap Objects for Repositories (Prevent RAM Ballooning)
- **The Problem:** Miller hydrated whole-repository symbol graphs and file lists into RAM heap objects (`PERF-001`). Memory grew to 1.5–2.0 GB per process, and background watchers performed 24.8 million syscalls during idle periods (`PERF-002`).
- **The Solution:** `code-kb` treats SQLite in WAL mode as the query engine. All symbol lookups, skeletons, and context slices execute as direct indexed SQL queries with `open_read_only`. Retained heap memory remains strictly under 15 MB.

### Lesson 3: Single-Turn Atomic Edits (Conserve Agent Turn Budget)
- **The Problem:** 2-step preview edits (`apply=false` followed by `apply=true`) burned 25% of an agent's standard 8-call budget on a single modification. If an agent got distracted between preview and commit, changes were abandoned.
- **The Solution:** `replace_symbol_body` performs pre-flight tree-sitter syntax validation, optional `expected_body_hash` concurrency verification, atomic file replacement, and SQLite re-indexing in a single turn.

### Lesson 4: Lexical/AST Precision Beats Semantic Noise (Calibration Evidence)
- **The Problem:** In Miller's August 25 calibration (`2026-08-25-miller-vs-bare-agent-v1.22.1-calibration.md`), semantic vector retrieval actually degraded agent performance compared to AST/lexical queries. The semantic arm exceeded token budgets (12,074 tokens) and distracted the LLM with irrelevant semantic associations.
- **The Solution:** `code-kb` relies on deterministic AST queries (`find_symbol`), token-dense structural facts (`find_structural_facts`), and SQLite FTS5 BM25 search with Porter stemming (`search_symbols`). This provides accurate natural language discovery without hallucinated vector drift.

### Lesson 5: Zero-Friction Ergonomics (Aliases & Scoping)
- **The Problem:** In prior iterations, minor parameter mismatches (e.g. `file` vs `file_path`, `symbol` vs `symbol_name`, missing `direction`) caused tool rejections and agent turn retries.
- **The Solution:** All MCP tool handlers accept intuitive aliases (`file`, `path`, `symbol`, `name`, `body`, `code`), supply safe defaults (`direction="callers"` in `find_references`, listing all categories when `category` is omitted in `find_structural_facts`), and provide path scoping (`--path`) to eliminate cross-module noise.

---

## 4. Empirical Benchmark Evidence

The benchmark suite (`scripts/benchmark_quality.py`) measures live performance against the codebase:

### 1. Token Compression: Skeletons vs Full File Reads
| File | Raw Lines | Raw Tokens | Skeleton Tokens | Token Savings | Latency (median) |
|---|---:|---:|---:|---:|---:|
| `queries.rs` | 1,192 | ~11,287 | ~1,403 | **87.6%** | 6.11 ms |
| `server.rs` | 835 | ~9,049 | ~220 | **97.6%** | 4.41 ms |
| `workspace.rs` | 476 | ~3,968 | ~986 | **75.2%** | 5.54 ms |

### 2. Surgical Context Slicing vs Full File Reads
| Target Symbol | Raw File Tokens | Slice Tokens | Token Savings | Latency (median) |
|---|---:|---:|---:|---:|
| `search_symbols_scoped` | ~11,287 | ~674 | **94.0%** | 4.65 ms |
| `file_skeleton_op` | ~2,531 | ~309 | **87.8%** | 5.00 ms |
| `format_file_skeleton` | ~3,576 | ~367 | **89.7%** | 5.76 ms |

### 3. Query Latency & Search Quality
| Query Type | Command | Accuracy (Top Hit) | Tokens Injected | Latency (median) |
|---|---|:---:|---:|---:|
| Exact Symbol Lookup | `code-kb symbol search_symbols_scoped` | **PASSED** | ~188 | 4.41 ms |
| Prefix Symbol Lookup | `code-kb symbol load_scoped` | **PASSED** | ~173 | 4.66 ms |
| Conceptual FTS5 Search | `code-kb search syntax validation` | **PASSED** | ~492 | 4.90 ms |
| Unscoped Search | `code-kb symbol QueryError` | **PASSED** | ~40 | 4.56 ms |
| Scoped Search (`--path`) | `code-kb symbol QueryError --path crates/code-kb-core` | **PASSED** | ~40 | 4.36 ms |

---

## 5. Worktree Freshness & Pruning Verification

`code-kb` was validated against isolated git worktrees (`crates/code-kb-core/tests/worktree_test.rs`):
1. **Isolated Indexes:** A new git worktree creates its own local index. Modifying files in the worktree updates only the worktree's database; the main repository's database remains unmodified.
2. **Self-Cleaning Storage:** Worktrees maintain their isolated database at `<worktree-root>/.code-kb/artifact.db`. Deleting the worktree directory or running `git worktree remove` automatically cleans up the database with no orphaned state left behind. *(Historical note: An early centralized database design used `code-kb prune`, which was deprecated in Plan 012 in favor of self-cleaning in-tree databases).*
