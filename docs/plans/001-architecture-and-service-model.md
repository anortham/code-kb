# code-kb: System Architecture & Service Model

## 1. Executive Summary

`code-kb` is an agent-facing knowledge and code-intelligence engine designed to provide fast, token-efficient navigation and understanding to AI coding agents (Antigravity, Claude Code, Cursor, Windsurf, etc.). It consumes the rich AST extraction artifacts produced by `julie-extractors` (SQLite schema v7 and Versioned Family Store v2) and presents a compact, progressive-disclosure interface via the Model Context Protocol (MCP) and CLI.

This document analyzes three critical architectural questions:
1. **Service Scope & Multi-Project Topology** (managing dozens of repos on a single developer workstation).
2. **Git Worktrees & Parallel Agent Concurrency** (coordinating multiple concurrent agents across isolated worktrees without duplicate extraction or database contention).
3. **Semantic Search & RAG Capabilities** (evaluating deterministic AST graphs vs. vector embeddings vs. hybrid FTS5).

---

## 2. Service Scope: Multi-Project Workstation Architecture

### The Problem
A developer workstation may have 20–50 repositories checked out. An agent operating in repo A should not be blocked by, or interfere with, repo B. Furthermore, running 30 persistent background daemons consumes memory, leaks handles, and creates port-management friction, especially on Windows.

### Architectural Options

| Topology | Description | Pros | Cons | Verdict |
| :--- | :--- | :--- | :--- | :--- |
| **A. Monolithic Machine Daemon** | One system service/daemon managing all projects on port/socket | Single background process | Fragile failure domain, complex multi-tenant cache management, cross-project crashes | **Rejected** |
| **B. Project-Level Daemon** | One daemon per active project directory | Isolated project state | Resource bloat (30 daemons), port conflicts, zombie process accumulation | **Rejected** |
| **C. Session-Scoped MCP Server + Shared SQLite** | Agent spawns MCP server as child process over `stdio`; connects directly to SQLite store | Zero persistent background bloat, instant startup, crash isolation, perfect per-session lifecycle | Multi-agent coordination requires file/lease locking in SQLite | **Recommended** |

### Selected Architecture: The "Daemonless" Agent Service

```text
┌─────────────────────────────────────────────────────────────┐
│                       Agent Session                         │
│             (Spawns MCP server over stdio)                  │
└──────────────────────────────┬──────────────────────────────┘
                               │ JSON-RPC (stdio)
┌──────────────────────────────▼──────────────────────────────┐
│                    code-kb MCP Server                       │
│  - In-process Rust service (<15MB RAM)                      │
│  - Query-time reference resolution                          │
│  - Source byte-slice cache                                  │
│  - Token-optimized compact formatting                       │
└──────────────────────────────┬──────────────────────────────┘
                               │ Read-only WAL connection (Lock-free)
┌──────────────────────────────▼──────────────────────────────┐
│                  Project / Family Store                     │
│               (SQLite .db in WAL Mode)                      │
└─────────────────────────────────────────────────────────────┘
```

#### Storage Location
- **Default:** Global user cache directory:
  - Windows: `%LOCALAPPDATA%\code-kb\stores\<repo-slug>-<repo-id>\`
  - Linux/macOS: `~/.cache/code-kb/stores/<repo-slug>-<repo-id>/`
- **Optional In-Tree:** `.code-kb/` in project root when explicit local portability is desired.
- **Why Centralized Store?** Avoids polluting git repositories with SQLite files, keeps git status clean, and prevents issues with file watchers and build tools.

---

## 3. Git Worktrees & Parallel Agent Swarms

### The Challenge
Modern agentic workflows spawn multiple concurrent subagents, each running in its own **git worktree** (e.g., `main`, `agent-fix-auth`, `agent-refactor-db`).
- In a 100,000-line codebase, 5 worktrees sharing 99.5% identical code should **not** parse the same files 5 times.
- Multiple agents must read and update code concurrently without triggering SQLite `database is locked` errors.

### The Solution: Family Store with Content-Addressed Deduplication

`code-kb` adopts the **Family Store** architecture proven in `julie-extractors`:

```text
Git Common Repo (.git)
       │
       ├── Worktree 1 (main)          ──> View: view_main
       ├── Worktree 2 (agent-auth)    ──> View: view_auth
       └── Worktree 3 (agent-db)      ──> View: view_db
                                                │
                                                ▼
                         ┌─────────────────────────────────────────┐
                         │           Family Store Directory         │
                         │                                         │
                         │  ┌───────────────────────────────────┐  │
                         │  │ store.db (Immutable CAS Versions)  │  │
                         │  │ - file_versions(hash, path, ast)  │  │
                         │  │ - manifest_generations per view   │  │
                         │  └───────────────────────────────────┘  │
                         │  ┌───────────────────────────────────┐  │
                         │  │ coord.db (Concurrency & Fencing)  │  │
                         │  │ - writer_leases & heartbeats      │  │
                         │  │ - reader_pins                     │  │
                         │  └───────────────────────────────────┘  │
                         └─────────────────────────────────────────┘
```

#### 1. Content-Addressed Storage (CAS) Deduplication
- Files are extracted and keyed by `(path, content_hash)`.
- When an agent checks out a new worktree, 99.5% of the files already exist in `file_versions`.
- Generating the index for a new worktree simply writes a new manifest linking existing versions. Total setup time: **< 1 second**.

#### 2. View-Based State Isolation
- Each worktree registers an independent `view_id`.
- When Agent 2 edits `src/auth.rs` in `worktree-2`, it triggers an incremental update for `view_auth` only.
- Agent 1 reading `main` sees its own unmodified snapshot without interference.

#### 3. Concurrency Fencing & Non-Blocking Reads
- SQLite **WAL (Write-Ahead Logging)** enables infinite concurrent readers while a write occurs.
- `coord.db` manages short-lived writer leases (< 200ms) with PID heartbeats.
- Agents reading symbol outlines or references never block each other.

---

## 4. Semantic Embeddings & RAG vs. Deterministic AST Graphs

### The Limitations of Code RAG (Vector Search)
While vector search is popular for prose, traditional vector RAG exhibits severe failure modes in code intelligence:

1. **AST Boundary Fragmentation:** Naive token/line chunking cuts functions, classes, and scopes across chunk borders, destroying syntactic integrity.
2. **Imprecise Identifier Matching:** Vector similarity fails at exact symbol resolution (e.g., confusing `UserToken`, `UserTokenFactory`, and `OAuthToken`).
3. **Expensive Stale State:** Re-embedding files on every agent edit incurs token cost, network latency, or local GPU spikes.
4. **Context Waste:** RAG chunks return whole blocks of implementation logic, defeating the primary goal of context economy.

### Comparison: AST Facts vs. Hybrid Search vs. Vector RAG

| Feature | AST Knowledge (`code-kb`) | SQLite FTS5 (BM25) | Vector RAG / Embeddings |
| :--- | :--- | :--- | :--- |
| **Exact Symbol Lookup** | Instant, 100% exact | High (prefix/trigram) | Poor (fuzzy confusion) |
| **Call & Type Graphs** | Deterministic relationships | None | None |
| **Outlines / Skeletons** | Native tree-sitter spans | None | None |
| **Token Cost to Build** | 0 tokens (local parser) | 0 tokens (local index) | High API/GPU cost |
| **Incremental Speed** | < 10ms per file | < 5ms per file | 200–800ms per file |
| **Conceptual Search** (*"rate limit"*) | Only if in name/attr | **High** (via doc comments) | **Very High** |

### The Recommendation: Tiered Hybrid Search

```text
Tier 1: Deterministic AST Graph (90% of agent actions)
   ├── Outlines, Skeletons, Signatures, Body Slices
   └── Caller/Callee traversal, Type definitions

Tier 2: In-Database Full-Text Search (SQLite FTS5) (8% of agent actions)
   ├── BM25 search over symbols, signatures, and doc_comments
   └── Zero extra processes, zero external API costs, instant updates

Tier 3: Opt-in Semantic Embeddings (2% of agent actions)
   ├── ONLY for conceptual queries where symbol names are unknown
   ├── Embeds (Name + Signature + Docstring), NEVER raw implementation bodies
   └── Pluggable / Optional (offline-first by default)
```

By keeping Tier 1 and Tier 2 at the core, `code-kb` delivers 98% of all code intelligence locally, deterministically, and with zero embedding overhead.

---

## 5. Next Steps

1. **Specification of the MCP Interface:** Define JSON-RPC tool signatures and token formatting standards.
2. **Storage Adapter:** Build the thin read-only client library connecting to `julie-extractors` SQLite schema v7 and Family Store v2.
3. **Progressive Disclosure Formatter:** Implement the token-minimizing syntax serializer for file skeletons and context slices.
