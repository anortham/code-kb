# Filtered watcher registration implementation

## Result

`start_watcher` keeps its signature and `WatcherError` keeps its existing variants. The watcher now registers the root and each scanner-eligible directory separately with `RecursiveMode::NonRecursive`. A single worker owns the debouncer, registration map, and database updates; the notify callback sends through a bounded, nonblocking queue.

The directory walk uses the same `WalkBuilder` filters as workspace reconciliation, including standard ignore files, the three custom ignore names, hidden-file behavior, and `is_hard_excluded`. Its `filter_entry` predicate prunes excluded directories before descent. New directories are watched as the walk yields them; a full scan follows registration reconciliation so files created before a new watch was installed are indexed. Directory create/remove, rename, ignore-file changes, notify rescan signals, oversized batches, and queue overflow trigger registration reconciliation plus a full scan. Ordinary file changes retain incremental updates.

Registration state retains directory paths only. Windows key equality uses the repository's `paths_equal` behavior after `dunce::simplified`; removed Linux watches that notify already discarded are cleared when `unwatch` returns `WatchNotFound`. `WatcherHandle` signals shutdown and joins its worker. Startup failures return through `start_watcher` after the worker drops the debouncer.

## Regression evidence

- Before the fix, the Linux inotify integration test counted 75 added watches for an ignored tree containing 65 directories. The new watcher adds at most 3 for the same fixture (workspace root and allowed `src`), and a subsequently created ignored tree does not increase the count.
- Before the fix, creating `.gitignore` to ignore an indexed `src/` tree left its symbols indexed after five seconds. The test now covers ignore-file creation, a rule modification that re-enables one sibling, deletion that re-enables both files, and matching directory-watch removal/reinstallation.
- Dynamic coverage creates a nested tree and writes immediately, moves in a populated subtree, deletes it, recreates it, and checks that no watcher failure remains. A 75-file burst verifies the full index catches up; a missing-root startup test checks that notify backends are released.

## Pinned API evidence

- `notify 8.2.0` documents `NonRecursive` as watching only the selected path: [RecursiveMode](https://docs.rs/notify/8.2.0/notify/enum.RecursiveMode.html).
- `notify-debouncer-full 0.7.0` exposes `watch`, `unwatch`, and a debouncer that stops on drop: [Debouncer](https://docs.rs/notify-debouncer-full/0.7.0/notify_debouncer_full/struct.Debouncer.html).
- A debounced event preserves the original event and exposes `need_rescan()` when events may have been missed: [DebouncedEvent](https://docs.rs/notify-debouncer-full/0.7.0/notify_debouncer_full/struct.DebouncedEvent.html).
- Pinned `ignore 0.4.33` documents that `filter_entry` omits entries and prevents descent into rejected directories, and that rebuilt matchers observe changed ignore files: [WalkBuilder](https://docs.rs/ignore/0.4.33/ignore/struct.WalkBuilder.html).

## Verification

- Red at 2026-10-27 15:02:03 UTC: `cargo test -p code-kb-core --test watcher_registration_test watcher_does_not_register_ignored_descendants -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/red.txt`. The test binary failed with 75 added watches; the shell pipeline returned `tee`'s status because this first red command did not enable `pipefail`.
- Red at 2026-10-27 15:04:33 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_reconciles_ignore_file_transitions -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/red-ignore.txt`. Exit 101; the indexed symbol remained after the new ignore rule.
- Green at 2026-10-27 15:09:02 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_reconciles_new_moved_deleted_and_recreated_subtrees -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/green-dynamic.txt`. The test passed. It also exposed Linux `WatchNotFound` after removal, which the registration cleanup now treats as an already-removed native watch.
- Green at 2026-10-27 15:10:04 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_startup_failure_releases_notify_backend -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/green-startup-cleanup.txt`. The test passed.
- Green at 2026-10-27 15:18:46 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_does_not_register_ignored_descendants -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/green-watches.txt`. The test passed for the existing and newly created ignored trees.
- Green at 2026-10-27 15:20:01 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_recovers_a_large_file_event_storm -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/green-storm.txt`. The 75-file storm test passed.
- Green at 2026-10-27 15:20:23 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_reconciles_ignore_file_transitions -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/green-ignore.txt`. Ignore-file creation, modification, deletion, indexed-content transitions, and registration-count transitions passed.
- Final target at 2026-10-27 15:20:35 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test 2>&1 | tee .razorback/watcher-registration/final-registration.txt`; 5 passed.
- Final existing watcher suite at 2026-10-27 15:20:40 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_test 2>&1 | tee .razorback/watcher-registration/final-watcher-test.txt`; 6 passed, including nested ignores and the `targeted` directory case.
- Formatting at handoff: `rustfmt --edition 2024 --check crates/code-kb-core/src/watcher.rs crates/code-kb-core/tests/watcher_registration_test.rs`; passed.
- Baseline CLI `target/edit-evaluation/lead/watcher-baseline/code-kb check --root /home/murphy/.codex/worktrees/1e6f/code-kb`: exit 0. It found no changed public caller contract; it recommends the CLI integration test `a_panicked_prepare_returns_an_error_and_retries`, which remains for the lead's integration gate.

## Boundaries and remaining verification

- The 75-file test verifies reconciliation after a burst. It does not deterministically force `SyncSender::try_send` to return `Full`; the queue-full branch sets the same coalesced overflow flag as oversized batches and `need_rescan`, and the worker runs registration reconciliation plus a full scan for that flag.
- Startup cleanup is exercised with a missing root and confirms the process's inotify fd count is unchanged after `start_watcher` returns an error. A native watch-limit failure after partial registration is not forced in this test; the startup worker returns an error and drops the debouncer on the same cleanup path.
- The new-directory race is covered by immediate mkdir/write and populated-subtree move. The actor registers a yielded directory before advancing the walker, then runs a full index scan; create events caught during traversal are processed after startup/reconciliation.
- No Windows run was available in this worker slice. The implementation normalizes watch paths with `dunce::simplified`, uses `paths_equal` in Windows registration-key equality, and sends all registration operations through the pinned cross-platform `Debouncer::watch`/`unwatch` API. The lead owns Windows runtime, rename/casing, and shutdown verification.
- The lead's baseline live probe remains preserved at `target/edit-evaluation/lead/watcher-probe-before-v2/result.json`: 1,016 watches and 15,760 KiB RSS. This worker did not measure post-fix retained RSS; the in-process test verifies the ignored-tree watch delta is at most 3.

## Acceptance coverage

| Requirement | Evidence |
| --- | --- |
| Existing and new ignored trees avoid descendant watches | Linux backend count test checks the existing `vendor/` tree and a new `late-ignore/` tree. |
| Allowed siblings and root files remain visible | Existing `watcher_test` root-file flow and the `targeted` sibling-directory test pass. |
| Immediate nested creation and populated moves sync | Dynamic registration test creates directories and immediately writes, then moves an already-populated tree into the workspace. |
| Delete, rename, and recreate remove stale index/watch state | Dynamic test moves the populated subtree, deletes it, recreates the same path, and checks the recreated symbol; no watcher failure remains. |
| Ignore-file create/modify/delete reconcile contents and watches | Ignore-transition test checks indexed symbols and Linux watch counts as rules narrow and widen. |
| Startup races, storms, overflow and dynamic errors are handled | Root is watched before startup traversal; directory events trigger a full scan after registration; 75-file storm passes; bounded queue overflow/rescan paths coalesce into the same reconciliation flag; failures update `WatcherFailureState`. |
| Shutdown and traversal avoid callback ownership cycles and file inventory | One worker owns the debouncer; callback uses `try_send`; only directory paths are retained; startup-failure fd test passes. |
| Windows compatibility | Code path preserves simplified/case-insensitive path identity; runtime verification remains lead-owned. |

## Exact artifacts

- `crates/code-kb-core/src/watcher.rs` SHA-256: `9392c4961a07bcb790e9c7da13086dab53aa0368a0daa97b9d8b8995e9a6d1bf`.
- `crates/code-kb-core/tests/watcher_registration_test.rs` SHA-256: `9eb6d98fdfb6b1bbbfab96e033b2684dc276480ce78b0697d4252102c6eea0bf`.
- Preserved baseline CLI SHA-256: `dab4bbdb3957ea81f2bd9d0a150c3a32411bd347ede7da1736943b44c1c3a07b` (`target/edit-evaluation/lead/watcher-baseline/code-kb`).
- Pinned extractor SHA-256: `777fe822a0eddbdfd9f357c018c7a1a881907bedc698f913736a30ffc0ce0d12` (`target/edit-evaluation/lead/watcher-baseline/julie-extract`).

## State and scope

- Worktree: `/home/murphy/.codex/worktrees/1e6f/code-kb`
- Branch: `fix/cross-language-review`
- HEAD at handoff: `fe7ede045be57d6e5905025e2eaf658f5d1550ac`
- Owned changes: `crates/code-kb-core/src/watcher.rs`, `crates/code-kb-core/tests/watcher_registration_test.rs`.
- No files were staged or committed. At handoff, the worktree also has sibling-owned modified evaluation references and untracked evaluation cases/scripts.
- Windows execution and process RSS measurement remain with the lead's integration gate.

## Second-round inline review fixes

**Completed:** 2026-10-27 15:58 UTC. This round addresses the lead's inline findings on startup reconciliation, incremental file renames, path-key consistency, registration failure recovery, and queue overflow.

### Behavior and coverage

- Startup now registers the root and eligible directories, then runs `scan_workspace(..., false)` before reporting the watcher healthy. A deep-file edit made after the prior scan but before `start_watcher` is indexed without another edit. A startup scan error returns an explicit error and drops the debouncer; Linux tests check both missing-root and scan-failure cleanup.
- Access events no longer start a full reconciliation. Ignore-control files and explicit `need_rescan()` events still reconcile. Hard-excluded and ignored structural paths are filtered before directory-change checks. File rename events stay incremental; allowed directory create, remove, and rename events reconcile registration and index state.
- `RegistrationPath::new` creates one normalized identity used by both `Eq` and `Hash`. It applies `dunce::simplified`, canonicalizes existing paths, uses forward slashes, and lowercases the key on Windows. `Eq` and `Hash` perform no filesystem access. A Windows-only test inserts trailing-separator, verbatim-prefix, and case variants into a `HashSet` and expects one identity.
- When native removal has already dropped a stale directory watch, registration synchronization resets the nonrecursive workspace root through the debouncer, re-watches that root, clears the directory map, and traverses again. The workspace root watch is restored before traversal and before the ensuing full index scan. This also clears all descendant debouncer roots and cache entries; pinned source evidence follows below.
- Registration failures have separate state from index failures. Database reconciliation cannot acknowledge a physical registration failure. The owner retries registration synchronization with a 1-second initial delay and exponential backoff capped at 30 seconds; retries do not run a full index scan. A successful registration retry clears the registration failure and runs one scan to catch up. A Unix mode-000 directory test observes the failure, attempts and fails to clear it, restores permission, waits for recovery, then edits a nested file and verifies the new symbol is indexed. Existing index-failure acknowledgement behavior remains covered.
- The callback queue uses `try_send`; full queue, oversized batches, and backend rescan signals set the same coalesced atomic reconciliation flag. A deterministic capacity-one test fills the queue, submits a second batch, and verifies the overflow flag and one queued batch.
- The subtree lifecycle test now edits a nested file after delete/recreate and checks the old/new symbol transition. On Linux, inotify watch count returns to its pre-move level after deletion and increases only for the recreated subtree.
- The atomic replacement integration test verifies old-symbol removal and new-symbol indexing. Its unit-level event assertion verifies that a file rename does not request full reconciliation. The existing populated-directory move and ignored-new-tree tests still pass.

### Pinned API and implementation evidence

`Cargo.lock` pins `notify 8.2.0` and `notify-debouncer-full 0.7.0` (checksums are recorded in the lockfile). The root-reset behavior relies on these exact implementation details:

- In `notify-debouncer-full 0.7.0`, [`Debouncer::unwatch`](https://docs.rs/crate/notify-debouncer-full/0.7.0/source/src/lib.rs#L610) first calls the backend's `unwatch`; only after success does it call `remove_root`. `remove_root` at [`src/lib.rs:592–598`](https://docs.rs/crate/notify-debouncer-full/0.7.0/source/src/lib.rs#L592) retains only roots that do not start with the supplied path and calls `cache.remove_path` for that path. Therefore unwatching the still-present workspace root removes its root entry and every descendant root from the debouncer's bookkeeping.
- The pinned recommended cache is `NoCache` on Linux and `FileIdMap` on Windows and other non-Linux targets ([`src/cache.rs:60–65`](https://docs.rs/crate/notify-debouncer-full/0.7.0/source/src/cache.rs#L60)). `FileIdMap::remove_path` drops every cached path prefixed by the root ([`src/file_id_map.rs:56–58`](https://docs.rs/crate/notify-debouncer-full/0.7.0/source/src/file_id_map.rs#L56)). This is the source evidence for internal cache cleanup; the Linux inotify count is separate evidence for backend watch cleanup.
- In `notify 8.2.0`, an inotify `DELETE`/`DELETE_SELF` queues removal of the native watch ([`src/inotify.rs:300–315`](https://docs.rs/crate/notify/8.2.0/source/src/inotify.rs#L300)); the event loop applies those removals at [`src/inotify.rs:379–381`](https://docs.rs/crate/notify/8.2.0/source/src/inotify.rs#L379). Its `remove_watch` returns `WatchNotFound` when the path is already absent from backend bookkeeping and otherwise removes the watch ([`src/inotify.rs:473–500`](https://docs.rs/crate/notify/8.2.0/source/src/inotify.rs#L473)). This explains why a deleted child can make `Debouncer::unwatch(child)` fail before its cache root is removed.
- The repaired order is visible in this worktree at `watcher.rs:458–465`: root unwatch, root re-watch with `NonRecursive`, map reset, then traversal restarts. Startup scan occurs only after registration sync succeeds at `watcher.rs:257–268`.
- `enqueue_events` uses pinned `DebouncedEvent::need_rescan`; the unit-test event constructor is `DebouncedEvent::new` in `notify-types 2.1.0/src/debouncer_full.rs`, re-exported by `notify-debouncer-full 0.7.0`. No new third-party dependency or caller-facing API was added.

### Red/green verification

- Red, 2026-10-27 15:42 UTC: `set -o pipefail; cargo test -p code-kb-core --lib watcher::tests::file_renames_and_accesses_do_not_trigger_full_reconciliation -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/round2-red-event-classification.txt`. Exit 101: an ordinary file `Modify(Name)` incorrectly requested full reconciliation.
- Red, 2026-10-27 15:47 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_reconciles_edits_made_before_startup_registration -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/round2-red-startup-gap.txt`. Exit 101: the deep-file edit stayed absent because no event followed watcher installation. Adding the startup scan made it pass.
- Green, 2026-10-27 15:57 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_reconciles_new_moved_deleted_and_recreated_subtrees -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/round2-root-cache-repair.txt`. Passed after stale-native-watch root/cache cleanup; Linux watch counts dropped after deletion and returned only for recreated directories.
- Green, 2026-10-27 15:58 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test 2>&1 | tee .razorback/watcher-registration/round2-final-registration.txt`. 9 passed, including startup gap, atomic replace, actual ignored-tree watch count, failure/recovery, lifecycle, and both startup cleanup paths.
- Green, 2026-10-27 15:58 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_test 2>&1 | tee .razorback/watcher-registration/round2-final-watcher-test.txt`. 6 passed.
- Green, 2026-10-27 15:58 UTC: `set -o pipefail; cargo test -p code-kb-core --lib watcher::tests 2>&1 | tee .razorback/watcher-registration/round2-final-watcher-unit.txt`. 7 passed, including file rename classification, ignored/hard-excluded directory filtering, bounded queue overflow, and both failure acknowledgement paths.
- Green, 2026-10-27 15:58 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test watcher_startup_reconcile_failure_releases_notify_backend -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/round2-startup-scan-cleanup.txt`. Passed; the failed startup scan left no extra Linux inotify instance.
- `rustfmt --edition 2024 --check crates/code-kb-core/src/watcher.rs crates/code-kb-core/tests/watcher_registration_test.rs` and `git diff --check` passed after the final code changes.

### Remaining integration gate

- Windows runtime and workspace-gate verification remain with the lead. The Windows-only `RegistrationPath` equality/hash test was not executed in this Linux worker slice. The lead should verify Windows root unwatch/re-watch behavior alongside path-case, verbatim-prefix, and trailing-separator behavior.
- The startup scan adds one reconciliation scan before the server marks the watcher healthy; this is intentional to close the pre-registration edit gap. Retry scans occur only after a successful registration retry, not on each failed retry.

### Final worker state

- Worktree: `/home/murphy/.codex/worktrees/1e6f/code-kb`
- Branch: `fix/cross-language-review`
- HEAD: `fe7ede045be57d6e5905025e2eaf658f5d1550ac`
- Owned files: `crates/code-kb-core/src/watcher.rs`, `crates/code-kb-core/tests/watcher_registration_test.rs`, this report.
- SHA-256: watcher.rs `5924f2ee27f545518ecdd877dc5d147cf40bd79d0a177b5ba82c3e04794e3d2d`; watcher_registration_test.rs `acafcddc872a52cfd50ea6b8406320aac1da60aa223ca8af35dc918e66c843f3`.
- Preserved baseline CLI remains unchanged: `dab4bbdb3957ea81f2bd9d0a150c3a32411bd347ede7da1736943b44c1c3a07b`.
- No files were staged or committed. Evaluation files/scripts are sibling-owned dirty changes and were not edited or reverted.

## Third-round root re-watch retry

**Completed:** 2026-10-27 16:13 UTC. This round addresses the follow-up where removing the debouncer's workspace-root watch succeeds but re-registering that root fails.

### Behavior and coverage

- After `debouncer.unwatch(root)` succeeds, registration state is cleared and `root_watch_requires_registration` is set before the fallible root re-watch. A failure therefore leaves no map entries that could make a retry report a physically missing root as healthy.
- Every synchronization retry checks the root registration requirement before starting the directory walk. Successful root reattachment is recorded before traversal and before the later recovery scan.
- Unknown root-unwatch errors preserve the current registration map and set `root_unwatch_requires_retry`, so a retry must resolve that operation before it can clear the registration failure. `WatchNotFound` invalidates the known-missing root registration; `PathNotFound` does so only when the root itself no longer exists. Other errors do not discard active-path bookkeeping.
- A test-only one-shot fault in `watch_workspace_root` forces the first root re-watch to fail after a removed subtree triggers root/cache reset. The test observes the pending registration failure, waits for recovery, then creates a root-level file and edits it; both index transitions are observed without another trigger.
- The startup-backend cleanup integration test now waits for the notify 8.2.0 inotify event-loop thread to close its fd. The suite had exposed a race in its previous immediate fd-count assertion; the bounded deadline ends with an exact fd-count assertion against the baseline.

### Red/green verification

- Red, 2026-10-27 16:06:23 UTC: `set -o pipefail; cargo test -p code-kb-core --lib watcher::tests::failed_root_rewatch_is_retried_before_future_root_edits -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/round3-red-root-rewatch-retry.txt`. Exit 101: the registration failure did not recover because the map still claimed the root was registered.
- Green, 2026-10-27 16:06:52 UTC: `set -o pipefail; cargo test -p code-kb-core --lib watcher::tests::failed_root_rewatch_is_retried_before_future_root_edits -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/round3-green-root-rewatch-retry.txt`. Passed after invalidating map state before the fallible re-watch.
- Green, 2026-10-27 16:10:56 UTC: `set -o pipefail; cargo test -p code-kb-core --lib watcher::tests::failed_root_rewatch_is_retried_before_future_root_edits -- --exact --nocapture 2>&1 | tee .razorback/watcher-registration/round3-final-root-rewatch-retry.txt`. 1 passed after the retry-state refinement.
- Green, 2026-10-27 16:11:24 UTC: `set -o pipefail; cargo test -p code-kb-core --lib watcher::tests 2>&1 | tee .razorback/watcher-registration/round3-final-watcher-unit.txt`. 8 passed.
- Initial integration attempt, 2026-10-27 16:11:46 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test 2>&1 | tee .razorback/watcher-registration/round3-final-registration.txt`. Eight passed; the startup-failure cleanup assertion sampled the fd before notify's asynchronous close. The isolated cleanup test passed at 16:11:57 UTC. After adding a bounded close wait, the serialized full registration suite passed.
- Green, 2026-10-27 16:14:24 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_registration_test -- --test-threads=1 2>&1 | tee .razorback/watcher-registration/round3-final-registration-serial.txt`. 9 passed after retaining the bounded cleanup wait and final exact fd-count assertion.
- Green, 2026-10-27 16:13:32 UTC: `set -o pipefail; cargo test -p code-kb-core --test watcher_test 2>&1 | tee .razorback/watcher-registration/round3-final-watcher-test.txt`. 6 passed.
- `cargo fmt --all -- --check` and `git diff --check` passed after the final test change.

### Final round hashes and state

- `crates/code-kb-core/src/watcher.rs` SHA-256: `4711212bf71c287a5b5f5b881bce2a3069f87f2de4e137a3a5717131f6c108f7`.
- `crates/code-kb-core/tests/watcher_registration_test.rs` SHA-256: `49bb01000c80f00b16f838524e36d1b903fe7658fcccf3b21d1cd9709038b317`.
- No caller-facing API or new dependency was added. Windows runtime remains with the lead's integration gate; this test-only root fault runs on Linux.
- Worktree: `/home/murphy/.codex/worktrees/1e6f/code-kb`; branch `fix/cross-language-review`; HEAD `fe7ede045be57d6e5905025e2eaf658f5d1550ac`.
- No files were staged or committed. Sibling-owned evaluation and documentation changes remain untouched.
