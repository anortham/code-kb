# code-kb v1.6.1 release verification receipt

This receipt was added after the v1.6.1 release. Verification ran in an isolated `/tmp/code-kb-release-v1.6.1` workspace; no repository files were changed during the checks.

## Published release provenance

- Commit: `2485892b9512425886bdd5f894811ac2b317b73a`
- Tag and release: [v1.6.1](https://github.com/anortham/code-kb/releases/tag/v1.6.1), published 2026-09-22T17:39:58Z; non-draft and non-prerelease.
- [CI](https://github.com/anortham/code-kb/actions/runs/35761141303), [Pages deployment](https://github.com/anortham/code-kb/actions/runs/35761141083), and [release build](https://github.com/anortham/code-kb/actions/runs/35761695575) all succeeded for that exact commit.

## Release assets

The release has exactly 12 assets: these six archives and their six matching `.sha256` sidecars. Downloaded all assets directly from the public release and ran `sha256sum -c *.sha256`; every archive passed. Each archive's manifest contains exactly its `code-kb` executable, matching `julie-extract` executable, `README.md`, `LICENSE-MIT`, and `LICENSE-APACHE`.

| Target | Archive SHA-256 |
| --- | --- |
| aarch64-apple-darwin | `1acd2c11ef5a2c46997b0ca3116a631e4cb709fddd6153536b5180cd1a84af2a` |
| x86_64-apple-darwin | `40b7354d21de9148a2431eb02c400b1a4dd014fc73d87da71a44494463d2540e` |
| aarch64-unknown-linux-gnu | `79ef7e66c14c3f03f15f820f1caa25315e745740b7eabfa86980e5f813cf6363` |
| x86_64-unknown-linux-gnu | `12ddcf62bc24684cdd660e2e49712ee126de7dda3d2db21a9740fb39d877d8ba` |
| aarch64-pc-windows-msvc | `9c2814e68ee4e8856abed9043c58402f36073ddb0ff5e4b0257bac41d22ccd06` |
| x86_64-pc-windows-msvc | `86f0e53d62b4d1162f98ff85cdace285988f24adfb8acd8a9d8529d34574ce2d` |

Downloaded the six upstream Julie 3.3.1 archives, verified every archive against `scripts/julie-pins.json`, extracted them, and compared their executable SHA-256 values with every bundled release copy. All six bundled Julie executables byte-match the verified upstream 3.3.1 executables.

## Runtime and registry checks

- Extracted Linux x86_64 archive reports `code-kb 1.6.1` and `julie-extract 3.3.1`; it fresh-scanned and looked up a symbol in a new Rust workspace.
- `cargo install code-kb-cli --version 1.6.1 --locked` completed from crates.io into an isolated root. The installed binary reports 1.6.1 and fresh-scanned/looked up a symbol with `JULIE_EXTRACT_BIN` set to the verified release Julie 3.3.1 binary.
- Official `cargo-binstall` 1.23.0 was downloaded only into the isolated workspace; its SHA-256 matched the official GitHub Release API digest. `cargo-binstall code-kb-cli --version 1.6.1 --no-confirm` downloaded the public GitHub release and installed `code-kb` 1.6.1. It also fresh-scanned and looked up a symbol with the verified release Julie 3.3.1 binary.

`cargo-binstall` installs the declared `code-kb` Cargo binary only; it does not copy the adjacent `julie-extract` from the downloaded archive into its Cargo root. The binstall runtime smoke therefore explicitly used `JULIE_EXTRACT_BIN` pointing to the verified 3.3.1 release binary. This follows the crate's binstall metadata and runtime-discovery contract, so crates.io/binstall users need Julie available through that documented discovery path.

## External evidence

The durable external record at `~/.code-kb/release-audits/v1.6.1/` retains the command outputs, manifests, reports, and a checksum manifest. It excludes downloaded archives, binaries, and extracted build trees.
