# Debouncer copy

This module is a copy of `notify-debouncer-full` from the notify repository at commit
[7d09a5808f5a99e7e8ef46c4e6cd34db0781301f](https://github.com/notify-rs/notify/tree/7d09a5808f5a99e7e8ef46c4e6cd34db0781301f/notify-debouncer-full),
the commit that released notify 9.0.0-rc.7 and notify-types 2.2.0. That source is the published
`notify-debouncer-full` 0.8.0-rc.2 plus the unreleased fixes that its changelog lists, and it
builds against notify `=9.0.0-rc.7`. The original MIT and Apache-2.0 license files are in this
folder, and `test_cases/` is the upstream set.

The copy takes these upstream changes after 0.7.0:

- A removal replaces a queued creation instead of deleting that path's event queue
  ([#900](https://github.com/notify-rs/notify/issues/900)). Earlier copies backported this from
  [PR #902](https://github.com/notify-rs/notify/pull/902).
- Unwatching a path removes only that root. The roots below it stay, and the cache adds their
  file IDs again.
- An event on a direct child of a nonrecursive root caches only that entry
  (`FileIdCache::add_single_path`).
- The debouncer thread waits on a condition variable while it has no events, and a watcher that
  fails to start no longer leaks that thread or the event handler
  ([#1040](https://github.com/notify-rs/notify/issues/1040)).
- `Debouncer::update_paths` and `Debouncer::watched_paths`.

The copy lives inside `code-kb-core` because a `[patch.crates-io]` override does not reach a
crates.io publish, so `cargo install code-kb-cli` would build without the fixes. It differs from
upstream in these ways:

- Module paths, `tracing` in place of `log`, and `std::collections::HashMap` in place of
  `rustc-hash`. No `crossbeam-channel`, `flume`, `futures`, or `tokio` handler.
- The watched roots are a `BTreeMap` from path to mode, not a sorted `VecDeque`. Adding a root,
  removing one, finding the roots below a path, and finding a path's watch mode do not scan every
  root. `FileIdCache::rescan` takes that map.
- `unwatch` also forgets a registered root when the backend has already dropped the watch
  (`WatchNotFound`) or the path is gone, and still returns the backend error.

Return to the published crate after a stable release includes these changes and passes the
project's Linux, macOS, and Windows regression checks.
