# Releasing `code-kb`

This guide details the end-to-end release process for `code-kb`. Any agent or maintainer can follow these reproducible steps to publish a new release.

---

## 1. Release Architecture & Principles

- **Semantic Versioning:** Releases follow `MAJOR.MINOR.PATCH` (e.g. `0.5.0`).
- **Bundled Distribution (Invariant 7):** Every GitHub Release archive bundles matching `code-kb` and pinned `julie-extract` binaries side-by-side. Users download a single `.tar.gz` or `.zip` archive with zero additional installation steps.
- **Dual Distribution:**
  1. **GitHub Releases:** Precompiled binary archives for 4 targets:
     - Linux x86_64 (`x86_64-unknown-linux-gnu`, `.tar.gz`)
     - macOS Apple Silicon (`aarch64-apple-darwin`, `.tar.gz`)
     - macOS Intel (`x86_64-apple-darwin`, `.tar.gz`)
     - Windows x86_64 (`x86_64-pc-windows-msvc`, `.zip`)
  2. **Crates.io:** Library crate `code-kb-core` followed by CLI crate `code-kb-cli`.
- **Pre-flight Invariants:**
  - `AGENTS.md` and `CLAUDE.md` must stay byte-for-byte identical (`cmp AGENTS.md CLAUDE.md`).
  - No `workspace` parameter exposed in any MCP tool schema (`mcp_test.rs`).
  - Retained memory < 15 MB.
  - Windows NTFS compatibility validated via `win-test` or CI.

---

## 2. Pre-Flight Verification Checklist

Run the automated pre-flight script, or execute each check manually:

```bash
# Automated pre-flight
./scripts/release-preflight.sh
```

### Manual Checks:

1. **Sync Contract (Byte-for-byte equivalence):**
   ```bash
   cmp AGENTS.md CLAUDE.md
   ```
2. **Formatting & Lints:**
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   ```
3. **Full Local Test Suite:**
   ```bash
   cargo test --workspace --locked
   ```
4. **Local Windows NTFS Test Suite (via `win-test`):**
   ```bash
   win-test status
   win-test sync code-kb
   win-test run code-kb -- cargo test
   ```
5. **Crates.io Package Dry-Run:**
   ```bash
   cargo package -p code-kb-core
   ```

---

## 3. Version Bump Procedure

When bumping to version `X.Y.Z` (e.g. `0.5.0`):

### 1. Update Manifests
- **`Cargo.toml`** (root workspace):
  ```toml
  [workspace.package]
  version = "X.Y.Z"
  ```
- **`crates/code-kb-cli/Cargo.toml`**:
  ```toml
  [dependencies]
  code-kb-core = { version = "X.Y.Z", path = "../code-kb-core" }
  ```
- **`.claude-plugin/plugin.json`**, **`.codex-plugin/plugin.json`**, **`.claude-plugin/marketplace.json`**, and root **`plugin.json`**:
  ```json
  "version": "X.Y.Z",
  ```
  The plugin launcher (`bin/code-kb-launcher.cjs`) downloads the release archive for this
  version on first run. Push the tag and let the release finish soon after the bump lands on
  `main`, because a plugin installed from `main` in between cannot download its binaries.
  A plugin installed from a source checkout with a `target/release/code-kb` build runs that
  build instead and never downloads, and so does any plugin on a machine with a binary at
  `~/.code-kb/bin/code-kb`.
  `tests/plugin/plugin-manifests.test.cjs` fails when the four manifests disagree.
- **`.github/workflows/release-binaries.yml`**:
  Update default version input to `"X.Y.Z"`.

### 2. Synchronize Lockfile
```bash
cargo check --workspace --all-targets
```

### 3. Prepare Release Notes
Draft comprehensive markdown release notes at `docs/release-notes/vX.Y.Z.md`:
- **Highlights & Summary:** Key theme of the release.
- **What's Changed / New Features:** Architectural, performance, and tooling capabilities.
- **Ergonomics & Invariants:** Agent UX improvements (parameter aliases, zero-workspace adherence).
- **Bug Fixes:** Defect resolutions and test isolation updates.
- **Assets & Checksums:** Table of artifact archives and their SHA256 hashes.

### 4. Commit Version Bump & Release Notes
```bash
git add Cargo.toml Cargo.lock crates/code-kb-cli/Cargo.toml .claude-plugin/plugin.json .codex-plugin/plugin.json .claude-plugin/marketplace.json plugin.json .github/workflows/release-binaries.yml docs/release-notes/vX.Y.Z.md
git commit -m "chore: release vX.Y.Z"
```

---

## 4. Triggering the GitHub Release

Pushing a signed or annotated git tag `vX.Y.Z` automatically triggers `.github/workflows/release-binaries.yml`:

```bash
# 1. Create annotated tag
git tag -a vX.Y.Z -m "Release vX.Y.Z"

# 2. Push commit and tag to GitHub
git push origin main
git push origin vX.Y.Z
```

### What GitHub Actions Does:
1. Matrix builds binaries on `ubuntu-latest`, `macos-latest`, `macos-13`, and `windows-latest`.
2. Downloads and verifies the pinned `julie-extract` binary matching `scripts/julie-pins.json`.
3. Packages `code-kb`, `julie-extract`, `README.md`, `LICENSE-MIT`, and `LICENSE-APACHE`.
4. Generates `.sha256` checksums for each archive.
5. Softprops `action-gh-release` publishes the GitHub Release with downloadable assets. If `docs/release-notes/vX.Y.Z.md` exists, it uses the file content as the release notes body via `body_path`; otherwise, it falls back to auto-generated commit logs.

### Updating Release Notes on GitHub:
To update or publish release notes for an existing release without triggering a new CI build:
```bash
gh release edit vX.Y.Z --notes-file docs/release-notes/vX.Y.Z.md
```

---

## 5. Monitoring & Verifying the Release

Use the GitHub CLI (`gh`) to monitor workflow execution:

```bash
# Check workflow run status
gh run list --workflow=release-binaries.yml

# Watch live run until completion
gh run watch

# Verify release assets and checksums
gh release view vX.Y.Z
```

### Verify Downloaded Archive:
```bash
gh release download vX.Y.Z -p "*linux-gnu.tar.gz*"
tar -tzf code-kb-vX.Y.Z-x86_64-unknown-linux-gnu.tar.gz
# Should list:
# ./code-kb
# ./julie-extract
# ./README.md
# ./LICENSE-MIT
# ./LICENSE-APACHE
```

---

## 6. Crates.io Publication (Optional / Staged)

When ready to publish to crates.io:

```bash
# 1. Publish core library first
cargo publish -p code-kb-core

# 2. Wait 60-120 seconds for crates.io index propagation
sleep 60

# 3. Publish CLI binary
cargo publish -p code-kb-cli
```

---

## 7. Rollback & Troubleshooting

- **Tag Build Failure:** If a CI matrix job fails, inspect the log with `gh run view --log-failed`.
  To re-trigger after fixing on `main`:
  ```bash
  # Delete local and remote tag
  git tag -d vX.Y.Z
  git push origin :refs/tags/vX.Y.Z
  # Re-tag and push after pushing the fix
  git tag -a vX.Y.Z -m "Release vX.Y.Z"
  git push origin vX.Y.Z
  ```
- **Manual Workflow Dispatch:** The workflow can also be triggered manually from GitHub Actions UI or CLI without tagging:
  ```bash
  gh workflow run release-binaries.yml -f version=X.Y.Z
  ```
