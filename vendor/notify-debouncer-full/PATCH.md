# Local patch

Based on the published crates.io `notify-debouncer-full` 0.7.0 source, with crate
checksum `c02b49179cfebc9932238d04d6079912d26de0379328872846118a0fa0dbb302`.

`src/lib.rs` backports [PR #902](https://github.com/notify-rs/notify/pull/902),
merged as [033b5000855b6f4a956b79c2c9c0257750d9e6db](https://github.com/notify-rs/notify/commit/033b5000855b6f4a956b79c2c9c0257750d9e6db).
A removal replaces a queued creation instead of deleting that path's event
queue. The two affected fixture expectations retain the removal event.

Remove this override after a compatible stable release includes this fix and
passes the project's Linux and Windows regression checks.
