# code-kb: Retrospective & Architecture Lessons from Miller

## 1. Context

`miller` (`c:\source\miller`) was a feature-rich C# / .NET 10 code-intelligence server built on top of `julie-extractors`. It ran real agent benchmarks (Codex `gpt-5.6-sol`) and accumulated extensive performance logs, architectural findings, and calibration data.

Examining Miller reveals both **brilliant successes** to incorporate into `code-kb` and **critical traps** that led to bloat, latency, and agent failures.

---

## 2. What Miller Got Right (Core Principles to Preserve)

1. **Exact AST Symbol Identity is the Decisive Advantage:**
   In Miller’s August 25 calibration (`2026-08-25-miller-vs-bare-agent-v1.22.1-calibration.md`), the bare agent solved **0 out of 5 exact-identity tasks**. Miller solved 11–12 of 15 total tasks. Exact symbol identity from `julie-extract` is the single biggest performance differentiator over text grep.
2. **Query-Time Reference Resolution:**
   Miller proved that computing reference resolution at query time from fact tables (rather than materializing resolution graphs at write time) was faster, eliminated gigabytes of disk bloat, and reached 99.9997% parity.
3. **CAS Family Store for Worktrees:**
   Content-addressed deduplication (`(path, content_hash)`) allows multiple worktrees to share AST extraction with zero re-parsing.
4. **Rigorous Calibration & Benchmarking:**
   Measuring real agent behavior against frozen budgets (8 calls / 12,000 tokens / 120s) exposed real friction that unit tests could never surface.

---

## 3. The 5 Major Traps in Miller (And How code-kb Avoids Them)

### Trap 1: The "Workspace Registry" & `workspace_id` Tax
- **What Miller Did:** `McpWorkspaceTargetPolicy` strictly enforced that every workspace-bound tool call (`search`, `inspect`, `context`, `trace`, `edit`) had to pass an explicit `workspace_id`. If omitted, the call was rejected with `ToolDiagnostic.Refusal(WorkspaceIdRequiredCode)`.
- **The Failure:** Agents constantly forgot the ID, hallucinated UUIDs, or wasted tool calls running `workspace operation=list` and `workspace operation=open`.
- **code-kb Solution:** **1:1 Process-to-Workspace Binding.** The server is bound to the workspace root via MCP protocol roots or launch flags. The LLM agent **never sees or passes a workspace ID**.

---

### Trap 2: In-Memory Heap Hydration (PERF-001 & PERF-002)
- **What Miller Did:** On startup and on every revision update, Miller loaded 200,000+ symbols into C# heap objects (`MillerRepositoryIndex` and `SymbolGraph`).
- **The Failure:** 
  - Memory ballooned to **1.5–2.0 GB RAM per host** (PERF-001).
  - Background freshness re-parsed and did **101.5 GB of logical reads** and 24.8 million read syscalls on idle (PERF-002).
- **code-kb Solution:** **Zero In-Memory Repository Objects.** `code-kb` treats SQLite in WAL mode as the query engine. All lookups, skeletons, and graph traversals are direct, indexed SQL queries against pinned disk views. Retained memory stays **< 15 MB**.

---

### Trap 3: Tool Call Budget Burn (Preview-First Edits & God Tools)
- **What Miller Did:** 
  - `edit` required a 2-step handshake: `apply=false` (preview unified diff), followed by `apply=true` (commit).
  - In an 8-call budget, an agent spent 2 calls on a single edit, blowing its call budget.
  - Tools were multi-verb "God tools" (`edit operation=...`, `search mode=...`, `workspace operation=...`).
- **code-kb Solution:**
  - **Single-Turn Atomic Edits:** `replace_symbol_body` validates syntax via tree-sitter, checks `body_hash`, and writes atomically in 1 turn.
  - **Single-Purpose Tools:** Distinct, intuitive tools (`file_skeleton`, `find_symbol`, `get_context_slice`) with minimal parameters.

---

### Trap 4: Distribution & Packaging Bloat
- **What Miller Did:**
  - Release archive was **103 MB zipped / 325 MB uncompressed**, bundling a 112 MB web dashboard executable (`Miller.Dashboard.exe`), a 58 MB Vulkan DLL (`ggml-vulkan.dll`), .NET runtime binaries, and the extractor.
  - Used a Node.js launcher script (`bin/miller-plugin-launcher.cjs`) that tried to download 100MB+ archives at runtime into `~/.miller/plugin-cache/`.
  - On Windows, downloads stalled silently, hit MCP 30-second timeouts, and crashed without logs (`2026-08-25-silent-plugin-startup-failure.md`).
- **code-kb Solution:**
  - **Single Native Rust Binary:** Statically linked, compiled to a single executable (< 15 MB).
  - No Node.js launcher, no web dashboard, no Vulkan/GPU dependencies. Zero runtime download during MCP startup.

---

### Trap 5: The "Semantic Noise" Trap (The Calibration Proof)
- **What Miller Found:**
  - In the August 25 calibration (`2026-08-25-miller-vs-bare-agent-v1.22.1-calibration.md`), when semantic embeddings were tested at the frozen budget, **the semantic arm performed WORSE than the lexical-only arm** (`baseline_only = 2`, `critical_loss_count = 1`).
  - Semantic results burned **12,074 tokens** (exceeding budget limits) and distracted the agent with semantic noise.
- **code-kb Solution:**
  - AST-first deterministic queries are the primary engine.
  - Full-text search (SQLite FTS5 / BM25) covers 95% of conceptual search for free with zero extra processes.
  - Semantic embeddings remain strictly opt-in and off by default.
