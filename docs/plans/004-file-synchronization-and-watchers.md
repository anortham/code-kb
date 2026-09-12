# code-kb: File Synchronization & Watcher Strategy

## 1. The Challenge of Keeping the Index Fresh

In an AI agent workflow, code changes occur rapidly:
- An agent calls `write_to_file` or `replace_symbol_body`.
- The human developer edits a file in their IDE.
- Git operations (`git checkout`, `git pull`, `git merge`) touch hundreds or thousands of files in milliseconds.

If the index is stale, the agent hallucinates or operates on old signatures. However, relying **solely on a naive background file watcher** introduces classic failure modes:
1. **Watcher Event Latency / Race Conditions:** An agent writes a file in step 1 and immediately requests its skeleton in step 2. If the background watcher is debouncing (e.g. 200ms delay), the agent reads stale data.
2. **Git Storms:** A branch switch firing 2,000 file change events can peg CPU and flood SQLite with sequential update transactions.
3. **Windows Handle Locking:** Directory-watching handles on Windows can block external tools or Git from renaming/deleting directories (`ERROR_SHARING_VIOLATION`).

---

## 2. The 3-Tier Synchronization Architecture

`code-kb` adopts a 3-tier hybrid synchronization model that guarantees freshness without watcher race conditions.

```text
┌─────────────────────────────────────────────────────────────┐
│ Tier 1: Tool-Driven Synchronous Update (0ms delay)          │
│ Triggered immediately by code-kb edit tools                 │
└─────────────────────────────────────────────────────────────┘
                               ▲
                               │
┌─────────────────────────────────────────────────────────────┐
│ Tier 2: Just-In-Time (JIT) Read-Time Staleness Guard         │
│ Targeted queries check file mtime/size against SQLite       │
│ If dirty: performs 3ms single-file update before responding │
└─────────────────────────────────────────────────────────────┘
                               ▲
                               │
┌─────────────────────────────────────────────────────────────┐
│ Tier 3: Background Debounced Watcher (External Edits)       │
│ Watches workspace files with .gitignore filtering           │
│ Includes circuit-breaker for git checkout storms            │
└─────────────────────────────────────────────────────────────┘
```

---

### Tier 1: Tool-Driven Synchronous Update
When changes are made through `code-kb` (such as the `replace_symbol_body` edit tool):
- The tool writes the modification to disk.
- It immediately invokes `julie-extract update --file <path>` in the same execution turn.
- The SQLite catalog is 100% fresh before the tool response is returned to the agent.
- **Latency:** ~3–5ms. Zero dependency on async events.

---

### Tier 2: Just-In-Time (JIT) Read-Time Staleness Guard
When an agent or developer modifies a file externally (e.g. using the harness's built-in edit tool or VS Code save):
- Whenever a targeted query is called (`file_skeleton(file_path)`, `get_symbol_body(name, file_path)`):
  1. `code-kb` stat-checks the file on disk (`mtime`, `bytes`).
  2. Compares against `files.indexed_at` and `files.content_bytes` in SQLite.
  3. **If dirty:** It triggers a synchronous single-file update in **< 5ms** before generating the skeleton or outline.
- **Result:** The agent is mathematically guaranteed to **never see stale code** for any file it directly inspects, completely eliminating race conditions.

---

### Tier 3: Background Debounced File Watcher
To keep global symbol lookups (`find_symbol`, `find_references`) fresh after external edits, `code-kb serve` runs a background watcher thread using the `notify` crate.

#### Essential Watcher Hardening Rules:
1. **Strict Ignore Rule Propagation:**
   - Must use the `ignore` crate (`.gitignore`, `.julieignore`, and hard safety exclusions).
   - Never attach watches or process events from `target/`, `node_modules/`, `.git/`, or build output dirs.
2. **Debounce Window:**
   - 150–200ms debounce per file path to collapse multi-event editor saves (temp file create &rarr; write &rarr; rename).
3. **Git Checkout Storm Circuit-Breaker:**
   - If the watcher queue exceeds **50 file events within a 500ms sliding window** (e.g. `git checkout` or `git stash pop`):
     - Cancel all pending micro-updates.
     - Fall back to a single background `julie-extract scan` (which processes the diff efficiently via CAS hashes).
4. **Lock-Free Concurrency:**
   - SQLite WAL mode ensures background updates never block incoming agent read queries.
