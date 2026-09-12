# code-kb: Cold-Start Reconciliation (Handling Offline Edits)

## 1. The Scenario

A developer opens a repository in VS Code, makes substantial edits across 10 files, git-pulls upstream changes, or reverts a commit—all while **no agent session is active and no `code-kb` process is running**.

Later, the developer launches an AI agent session (Cursor, Claude Code, Antigravity), which spawns `code-kb`. 

How does `code-kb` efficiently detect which files were modified, created, or deleted without:
1. Blocking the agent on startup.
2. Burning seconds of CPU re-hashing or re-parsing the entire codebase.
3. Serving stale data to the agent.

---

## 2. The 3-Phase Cold-Start Reconciliation

```text
Time 0ms: Agent launches session
│
├── [Main Thread / MCP Engine]
│   └── Opens SQLite in WAL mode (<5ms)
│   └── Ready to receive tool calls immediately (ZERO startup lag)
│
└── [Background Worker Thread]
    ├── Phase 1: Rapid Directory Stat Scan (~10-30ms)
    │   └── Compares dirent mtime & bytes against files.indexed_at
    │
    ├── Phase 2: Candidate Hash Verification (~10-20ms)
    │   └── Hashes only the candidate files flagged by Phase 1
    │
    └── Phase 3: Batch Incremental Update (~20-50ms)
        └── Invokes julie-extract incremental update for stale files
        └── Purges deleted files / inserts new files
```

---

### Phase 1: Fast Metadata / Stat Sweep (10–30ms)

On Windows and Unix, directory traversal APIs (`FindFirstFileExW` / `readdir`) return file size and modification timestamps (`mtime`) directly inside the directory entry without requiring an extra `stat()` syscall per file.

1. `code-kb` reads `SELECT path, content_bytes, indexed_at, content_hash FROM files` into an in-memory hash map (~2ms for 10,000 files).
2. It walks the workspace using the `ignore` crate (skipping `.git`, `node_modules`, `target/`).
3. For each file, it performs a zero-cost comparison:
   - **Deleted:** Path exists in SQLite `files` but not on disk.
   - **New:** Path exists on disk but not in SQLite `files`.
   - **Candidate Modified:** File on disk has `disk_mtime > files.indexed_at` OR `disk_bytes != files.content_bytes`.
   - **Unchanged:** `disk_mtime <= files.indexed_at` AND `disk_bytes == files.content_bytes`.

In a 10,000-file repository where the user edited 10 files offline, this step discards **9,990 files in ~15ms**.

---

### Phase 2: Content Hash Verification (< 10ms)

Timestamps alone can have false positives (e.g. `touch` or git checkout of identical files).
- For the ~10 candidate files flagged in Phase 1, `code-kb` computes their content SHA-256.
- If `new_hash == files.content_hash`, the file is marked clean (updating only `indexed_at`).
- If `new_hash != files.content_hash`, the file is marked **stale**.

---

### Phase 3: Batch Incremental Update (< 50ms)

- If **< 50 files changed**: `code-kb` triggers incremental updates for those specific files in parallel (`julie-extract update --file <path>`).
- If **> 50 files changed** (e.g. user ran a massive `git merge` or rebase while offline): `code-kb` triggers a fast `julie-extract scan`, which uses CAS hashing to reconcile the whole tree in bulk.

---

## 3. Why the Agent Experiences Zero Latency (The JIT Intercept)

What happens if the agent sends its first tool request (e.g. `file_skeleton("src/edited_file.rs")`) **before** the background reconciliation finishes?

The **Tier 2 Just-In-Time (JIT) Staleness Guard** intercepts the call:
1. `file_skeleton` checks `src/edited_file.rs`.
2. Sees that its disk timestamp is newer than `files.indexed_at`.
3. Re-indexes `src/edited_file.rs` on the spot in **3 milliseconds**.
4. Returns the 100% fresh skeleton to the agent.

The agent never waits for the whole repository to finish reconciling, and never reads stale code.
