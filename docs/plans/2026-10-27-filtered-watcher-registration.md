# Filtered watcher registration

**Goal:** Keep ignored build and dependency trees out of operating-system watch registration while preserving automatic synchronization for every supported language.

**Evidence:** During six-language edit preparation, a new MCP session failed to initialize its watcher. Nine live code-kb processes each held roughly 60,000 inotify watches. `start_watcher` registers the entire root recursively, then filters ignored events. No process was killed and no global limit was changed. Completed workers safely switched their own active root to tiny temporary projects to release their watches.

**Architecture:** Keep `start_watcher` and `WatcherHandle` caller contracts. One owner manages the debouncer and nonrecursive directory registrations. Stream the scanner's traversal policy and prune ignored subtrees before registering. Keep no repository file inventory. Read-only Astra approved this direction with immediate reconciliation of populated new subtrees and refreshed registrations on ignore-rule changes.

**Authority:** The owner approved continued fixes. The completed-edit plan requires a separate owned regression/fix slice for product defects. No push, release, kernel setting, unrelated process control, or provider call is included.

## Ownership and verification

A fresh Luna owns `crates/code-kb-core/src/watcher.rs`, optional submodules under `src/watcher/`, `crates/code-kb-core/tests/watcher_test.rs`, and optional watcher registration tests. It may change `workspace.rs` only to share existing traversal semantics, and must report that need. The lead owns docs, evidence, integration, commits, and Windows verification. Other workers own evaluation scripts and cases.

Tests come first. The worker runs focused red/green checks and the watcher test target once on its final tree. The lead runs the Rust workspace gate once after integration, Clippy/fmt/release, Windows watcher/runtime verification, and live Linux watch-count/RSS comparisons. The Python evaluation gate remains separate. Preserve the 72-run pilot and all prior raw results.

## Acceptance

- [x] Existing and newly created ignored trees allocate no descendant watches; allowed siblings and similarly named directories remain observable.
- [x] Root edits, newly created nested directories, and populated moves synchronize without requiring another edit.
- [x] Delete, rename and recreate remove stale registrations and indexed descendants.
- [x] Supported ignore-file creation, modification and deletion reconcile both registrations and indexed contents.
- [x] Startup races, event storms and queue overflow reconcile or fail explicitly; dynamic failures use the existing watcher failure state.
- [x] Ownership and shutdown avoid callback mutex cycles, deadlocks and leaked threads; traversal stores no second file inventory.
- [ ] Focused tests and Linux/Windows integration pass; before/after watch counts and retained RSS are recorded.


## Integration

Freeze the edit-study binary and executable hashes only after this fix and its verification. The six source-case readiness checks use subject toolchains and may proceed independently. No model performance claim follows from those local readiness checks.
