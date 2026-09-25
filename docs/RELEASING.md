# Releasing `code-kb`

This guide details the end-to-end release process for `code-kb`. Any agent or maintainer can follow these reproducible steps to publish a new release.

---

## 1. Release Architecture & Principles

- **Semantic Versioning:** Releases follow `MAJOR.MINOR.PATCH` (e.g. `0.5.0`).
- **Bundled Distribution (Invariant 7):** Every GitHub Release archive bundles matching `code-kb` and pinned `julie-extract` binaries side-by-side. Users download a single `.tar.gz` or `.zip` archive with zero additional installation steps.
- **Dual Distribution:**
  1. **GitHub Releases:** Precompiled binary archives for 6 targets:
     - Linux x86_64 (`x86_64-unknown-linux-gnu`, `.tar.gz`)
     - Linux ARM64 (`aarch64-unknown-linux-gnu`, `.tar.gz`)
     - macOS Apple Silicon (`aarch64-apple-darwin`, `.tar.gz`)
     - macOS Intel (`x86_64-apple-darwin`, `.tar.gz`)
     - Windows x86_64 (`x86_64-pc-windows-msvc`, `.zip`)
     - Windows ARM64 (`aarch64-pc-windows-msvc`, `.zip`)
  2. **Crates.io:** Library crate `code-kb-core` followed by CLI crate `code-kb-cli`.
- **Pre-flight Invariants:**
  - `AGENTS.md` and `CLAUDE.md` must stay byte-for-byte identical (`cmp AGENTS.md CLAUDE.md`).
  - No `workspace` parameter exposed in any MCP tool schema (`mcp_test.rs`).
  - Retained memory about 25 MB, measured on a live `serve` process.
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
   node --test tests/plugin/*.test.cjs
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
6. **Real-Project Corpus Check (preflight step 9 requires it):**
   Compare this build with the previous release on the real projects in
   `scripts/release-corpus.txt` (Rust, Swift, Go, JS, TS, Python, Java, Kotlin, C#, Razor, C, C++,
   Ruby, and the julie fixtures for the other languages). Run it on the local version-bump commit
   (section 3), because the report must be newer than `HEAD` and name the new version.
   ```bash
   PREV=2.1.0   # the previous release
   mkdir -p ~/.code-kb/search-eval/bin/v$PREV && cd ~/.code-kb/search-eval/bin/v$PREV
   gh release download v$PREV -R anortham/code-kb -p '*x86_64-unknown-linux-gnu*' --clobber
   sha256sum -c *.sha256 && tar xzf *.tar.gz && cd -
   mkdir -p target/candidate
   ln -sf "$PWD/target/release/code-kb" target/candidate/code-kb
   ln -sf "$PWD/.tools/julie-extract" target/candidate/julie-extract   # the pinned extractor
   scripts/release-corpus-check.py --old ~/.code-kb/search-eval/bin/v$PREV --new target/candidate
   ```
   The script fails on a failed scan, a symbol loss over 0.5%, a scan more than 1.5 times slower,
   an old index the candidate does not bring up to date, or a tool call that worked before and now
   errors or returns nothing. A PASS is not enough. Read `target/release-corpus/report.md`:
   - Check each gained and lost call-edge sample against its source line. A gained edge that is not
     a real call is a wrong caller in `find_references` and `blast_radius`. Fix it before release.
   - Open the changed tool outputs. Each change must come from a change in this release.
7. **Harness Dogfood Round (preflight step 10 requires it): one round per release.**
   The corpus check finds only output that changed since the previous release, so a defect that
   both releases share passes it. The dogfood round finds those wrong answers. Run exactly one
   round, on the release candidate, before the version bump.
   ```bash
   scripts/release-dogfood.sh   # DOGFOOD_PROJECT defaults to ~/source/flask
   ```
   The script builds the local code and runs one Claude Code and one Codex session on detached
   worktrees of the project. The sessions use every tool and check each answer against the source.
   `code-kb` on `PATH` must be `target/release/code-kb`. The script writes
   `target/release-dogfood/report.md`. Check each reported defect against the source, then list it
   in the report as one of:
   - `[fixed <commit>]`: the tool gave a wrong answer (a false statement, a wrong location, a
     wrong count, a crash). Fix it before the tag, add a regression test, and rerun the exact call
     from the report to confirm the new answer. A fix does not need a new round.
   - `[gap]`: the answer is true but incomplete, a heuristic misses or over-includes, or the
     session asks for a new feature. A gap does not block the release. List the gaps in the
     release notes, and tell the user after the release.
   - `[not a defect]` with the reason.
   - `[deferred: "<the user's own words>"]`: a wrong answer the user chose to ship.

   Set `## Result: PASS` when no line is `[open]`. Do not run a second round for the same
   release: the sessions always find something new, so a second round only delays the release.
   Preflight fails when the report is not `PASS`, lists an `[open]` defect, or tested a commit that
   is not an ancestor of `HEAD`. Commits after the round are allowed.
8. **New julie-extract version:** Do not tag it in `julie-extractors` first. Copy the local
   release build to `.tools/julie-extract`, set the version in `scripts/julie-pins.json` and
   `PINNED_JULIE_VERSION`, and run the dogfood round on that code-kb build. Tag and publish julie
   when the round has no `[open]` line. Then restore the published binary, set its checksums in
   `scripts/julie-pins.json`, commit, and run the corpus check (step 6) on the version-bump commit.

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
  version on first run, so the bump must never reach `main` without its tag. Section 3.5 pushes
  them together. A plugin that cannot download its version runs the newest cached version.
  A plugin installed from a source checkout with a `target/release/code-kb` build runs that
  build instead and never downloads, and so does any plugin on a machine with a binary at
  `~/.code-kb/bin/code-kb`.
  `tests/plugin/plugin-manifests.test.cjs` fails when the four manifests disagree.
- **`.github/workflows/release-binaries.yml`**:
  Update default version input to `"X.Y.Z"`.
- **`docs/site/index.html`**: the version badge in the nav (`<span class="version">vX.Y.Z</span>`)
  and the cache-busting query on the stylesheet link (`style.css?v=X.Y.Z`).

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
Before the bump, push every code commit to `main` and wait for green CI on it:
```bash
git push origin main
gh run watch --exit-status   # the CI run for that commit
```
Then commit the bump locally. It changes only the version files and the release notes below.
Keep it local.
```bash
git add Cargo.toml Cargo.lock crates/code-kb-cli/Cargo.toml .claude-plugin/plugin.json .codex-plugin/plugin.json .claude-plugin/marketplace.json plugin.json .github/workflows/release-binaries.yml docs/release-notes/vX.Y.Z.md
git commit -m "chore: release vX.Y.Z"
```

### 5. Preflight, Then Push the Bump and the Tag Together
Run the corpus check (section 2, step 6) on the bump commit, then the preflight. Step 8 passes
when the commit before the bump has green CI on `origin/main` and the bump changes only version
files.
```bash
./scripts/release-preflight.sh
git tag -a vX.Y.Z -m "Release vX.Y.Z"
git push --atomic origin main vX.Y.Z
```
`--atomic` pushes the bump and the tag together or not at all, so `main` never names a version
that has no release in progress. The release workflow waits for CI on the tagged commit before
it builds. Watch it to the end (section 5 below). If it fails, fix it on `main` at once:
plugins installed from `main` meanwhile run their cached version or cannot start.

---

## 4. Triggering the GitHub Release

The atomic push in section 3.5 pushes the tag `vX.Y.Z`, which triggers
`.github/workflows/release-binaries.yml`.

### What GitHub Actions Does:
1. Confirms the tag or dispatch version matches Cargo and every plugin manifest, then waits up to
   60 minutes for CI to pass on the exact commit. It fails when CI fails or does not finish.
2. Matrix builds binaries on `ubuntu-latest`, `ubuntu-24.04-arm`, `macos-latest`, `macos-15-intel`, `windows-latest`, and `windows-11-arm`.
3. Downloads and verifies the pinned `julie-extract` binary matching `scripts/julie-pins.json`.
4. Packages `code-kb`, `julie-extract`, `README.md`, `LICENSE-MIT`, and `LICENSE-APACHE`.
5. Generates `.sha256` checksums for each archive.
6. Softprops `action-gh-release` publishes the GitHub Release with downloadable assets. If `docs/release-notes/vX.Y.Z.md` exists, it uses the file content as the release notes body via `body_path`; otherwise, it falls back to auto-generated commit logs.

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

### Verify a Clean Plugin Install:
Run the launcher the way a new user does: an empty cache, no override binary, and no source
checkout next to it. It must download, verify, and run the new release before you publish to
crates.io.
```bash
SMOKE=$(mktemp -d)
mkdir -p "$SMOKE/plugin/bin" "$SMOKE/plugin/.claude-plugin"
cp bin/code-kb-launcher.cjs "$SMOKE/plugin/bin/"
cp .claude-plugin/plugin.json "$SMOKE/plugin/.claude-plugin/"
env -u CODE_KB_BIN CODE_KB_HOME="$SMOKE/home" node "$SMOKE/plugin/bin/code-kb-launcher.cjs" --version
env -u CODE_KB_BIN CODE_KB_HOME="$SMOKE/home" node "$SMOKE/plugin/bin/code-kb-launcher.cjs" \
  --root target/release-corpus/corpus/cobra lookup Command --limit 1
```

---

## 6. Crates.io Publication

Publish after the GitHub Release exists, because `cargo binstall code-kb-cli` reads the
crate's `[package.metadata.binstall]` block and then downloads that release's archive.

Prerequisites (once per machine): a crates.io account that owns both crates, a token with
publish scope, and `cargo login`. The crate manifests point `readme` at the repository
README, so the crates.io pages show it.

```bash
# 1. Publish the core library first; cargo waits until the index serves it
cargo publish -p code-kb-core --locked

# 2. Publish the CLI, which depends on the core version just published
cargo publish -p code-kb-cli --locked
```

Verify both install paths from the public registry into a scratch root:

```bash
cargo install code-kb-cli --locked --root /tmp/code-kb-check && /tmp/code-kb-check/bin/code-kb --version
cargo binstall code-kb-cli --no-confirm --root /tmp/code-kb-binstall && /tmp/code-kb-binstall/bin/code-kb --version
```

A published version cannot be edited or deleted, only yanked. Fix manifests before
publishing, not after.

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
- **Release Gate Failure:** The `Verify release commit` job fails when CI has not
  completed successfully for the tagged commit, or when the tag version differs from
  `Cargo.toml` or a plugin manifest. A version mismatch needs a fix on `main` and a
  re-tag (above). A CI timing failure needs no re-tag: wait for CI, then re-run the
  workflow on the existing tag:
  ```bash
  gh workflow run release-binaries.yml -f version=X.Y.Z --ref vX.Y.Z
  ```
- **Manual Workflow Dispatch:** The workflow can also be triggered manually from GitHub Actions UI or CLI without tagging:
  ```bash
  gh workflow run release-binaries.yml -f version=X.Y.Z
  ```
