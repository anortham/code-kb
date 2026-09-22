# code-kb v2.0.0 release verification receipt

## Published provenance

- Release commit and annotated tag `v2.0.0`: `54036188daed5332815fa4e0057de13536ccee85`.
- [GitHub Release](https://github.com/anortham/code-kb/releases/tag/v2.0.0) published 2026-09-22T22:52:05Z; neither draft nor prerelease.
- [Exact-commit CI](https://github.com/anortham/code-kb/actions/runs/35793277416) passed on Linux x64/ARM64, macOS ARM64, Windows x64/ARM64, and lint. [Release workflow](https://github.com/anortham/code-kb/actions/runs/35793761616) passed on retry attempt 2.
- [Pages deployment](https://github.com/anortham/code-kb/actions/runs/35792504924) passed for the v2.0.0 site changes; the live site showed the v2.0.0 badge and exact-symbol comparison row.

## Local gate and recovery

- The release preflight passed all eight checks: synchronized guidance, version consistency, formatting, clippy, 394 Linux Rust tests, 17 plugin tests, both workspace packages, Windows NTFS worktree tests, and green CI on the tag commit. The full Windows NTFS guest suite passed 388 tests with checksum-verified julie-extract 3.3.1.
- The first CI run failed only because a Windows watcher test assumed an asynchronous header refresh finished within 700 ms. Commit `5403618` changed that test to poll for the indexed hash, retaining its one-revision assertion. The focused test, local suites, and subsequent five-platform CI all passed.
- The first release workflow built the Intel macOS archive but GitHub's artifact service timed out on upload. Rerunning failed jobs against the same tag uploaded it and completed the release. No release source or tag was changed.

## Assets and extractor pin

The public release has exactly six archives and six `.sha256` sidecars. Every downloaded archive passed its sidecar check and contained exactly the platform `code-kb` executable, `julie-extract` executable, README, and two licenses. Every bundled extractor byte-matched the upstream 3.3.1 binary from an archive verified against `scripts/julie-pins.json`.

| Target | Archive SHA-256 |
| --- | --- |
| aarch64-apple-darwin | `9a931933b0ef7e5f2a41535c3251868218ef336bfb86628c5474f4866e595d32` |
| x86_64-apple-darwin | `df36b4a29cef59acefbedd752ea95ad638d519d3e7cabddf745c17ae46266a2e` |
| aarch64-unknown-linux-gnu | `bf80371ecb3fe86c24303b596644dce3b97dbc72aacd12106c3043633765b96b` |
| x86_64-unknown-linux-gnu | `236031605d976ff30e5eb36a2b6b5c5cca3c1a9917af6bbe622ac13627a34cc1` |
| aarch64-pc-windows-msvc | `d8831076d8d82805eeed93d364f129dbc02d9990f543c297c48bdc16d932a5d1` |
| x86_64-pc-windows-msvc | `806a26a79a6fd017083462441a7e95cb4d43eb15dd17c20e9fb5de55a8806744` |

## Registry and runtime checks

- [code-kb-core 2.0.0](https://crates.io/crates/code-kb-core/2.0.0) and [code-kb-cli 2.0.0](https://crates.io/crates/code-kb-cli/2.0.0) were published in that order and resolved from crates.io.
- The extracted Linux release binary reported `code-kb 2.0.0` beside `julie-extract 3.3.1`. It fresh-scanned a new Rust workspace and found a symbol by lookup.
- Public `cargo install code-kb-cli --version 2.0.0 --locked` installed a binary reporting 2.0.0. It fresh-scanned and looked up a symbol with `JULIE_EXTRACT_BIN` pointing to the verified release extractor.
- Checksum-verified official `cargo-binstall` 1.23.0 downloaded the v2.0.0 GitHub archive and installed a binary reporting 2.0.0. It also fresh-scanned and looked up a symbol with that `JULIE_EXTRACT_BIN` setting. Binstall installs the Cargo binary only; the extractor must be supplied through the documented runtime discovery path.

Detailed command output is retained under `~/.code-kb/release-audits/v2.0.0/`. Downloaded archives and binaries remain outside the repository.
