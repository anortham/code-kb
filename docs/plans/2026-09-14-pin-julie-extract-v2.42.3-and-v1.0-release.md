# Pin julie-extract v2.42.3 & Prepare v1.0.0 Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use razorback:subagent-driven-development whenever delegation is available and permitted, including for one task; serialize dependent tasks. Use razorback:executing-plans only when delegation is unavailable or the user/session explicitly selected single-agent execution.

**Goal:** Pin `code-kb` to the newly released `julie-extract` v2.42.3, verify 100% extraction and query compatibility across Linux, macOS, and Windows (NTFS), and prepare the project for the `v1.0.0` milestone release with end-to-end preflight verification.

**Architecture:** Update the pinned extractor version across `scripts/julie-pins.json`, `crates/code-kb-core/src/sync.rs`, and build/telemetry guards. Restore the updated binary and execute full multi-platform regression testing (including on the live Windows 11 VM via `win-test`). Bump version to `1.0.0` across all workspace manifests, synchronization lockfiles, plugin descriptors, and release workflows, verifying clean execution through `scripts/release-preflight.sh`.

**Tech Stack:** Rust, SQLite (via `rusqlite`), `julie-extract` (v2.42.3), Model Context Protocol (MCP), Windows VM (`win-test`).

**Spec:** User request ("pin to new version of julie-extract, make sure we have full compatibility with, and after that do a v1.0 release"), [docs/RELEASING.md](file:///home/murphy/source/code-kb/docs/RELEASING.md), and [AGENTS.md](file:///home/murphy/source/code-kb/AGENTS.md) Core Invariant 7.

**Architecture Quality:** No Architecture Impact. Preserves pure SQLite storage, retained memory < 15MB, zero workspace parameter invariant, and exact 1:1 MCP-to-CLI parity.

---

## Global Constraints

- Never expose `workspace`, `workspace_id`, `repo_path`, or `root_dir` in any MCP tool schema (Core Invariant 1).
- Retained memory must stay < 15 MB (Core Invariant 2).
- Single-turn atomic edits remain intact (Core Invariant 3).
- Windows compatibility: all paths normalized with forward slashes `/` and `dunce::simplified` (Core Invariant 5).
- Pinned Extractor & Bundled Distribution: `scripts/julie-pins.json` pins `2.42.3` with accurate SHA256 checksums across all 4 architectures (Core Invariant 7).
- Sync Contract: `AGENTS.md` and `CLAUDE.md` must remain byte-for-byte identical (`cmp AGENTS.md CLAUDE.md`).
- Exact 1:1 MCP-to-CLI command parity.

---

## Verification Strategy

**Project source of truth:** `AGENTS.md`, `CLAUDE.md`, `Cargo.toml`, `docs/RELEASING.md`.

**Worker red/green scope:**
- Extractor pin & restore: `bash scripts/restore-julie-extract.sh` & `.tools/julie-extract --version`
- Workspace build & test: `cargo test --workspace`
- Clippy & Formatting: `cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check`
- Windows NTFS verification: `win-test sync code-kb && win-test run code-kb -- cargo test`
- Release pre-flight: `./scripts/release-preflight.sh`

**Worker ceiling:** `cargo test --workspace`

**Worker gate invariant:** Each task must compile, pass tests, pass clippy with 0 warnings, maintain formatting, and pass preflight checks.

**Branch gate:**
```bash
./scripts/release-preflight.sh
```

**Security scope:** `none declared`

**Replay/metric evidence:** All 199+ tests passing with `julie-extract` v2.42.3, 0 clippy warnings, clean formatting, and Windows NTFS test suite passing.

---

## Parallel Execution Contract

| Task | Parallel batch | File ownership | Serialization required | Dependency reason |
|---|---|---|---|---|
| Task 1: Pin julie-extract v2.42.3 and update extractor references | Batch A | `scripts/julie-pins.json`, `crates/code-kb-core/src/sync.rs`, `crates/code-kb-cli/build.rs`, `crates/code-kb-core/src/telemetry.rs`, `AGENTS.md`, `CLAUDE.md` | No | None - standalone dependency update. |
| Task 2: Full Compatibility Verification against julie-extract v2.42.3 | Batch B | Full test execution across Linux and Windows | Yes | Depends on Task 1 binary restore. |
| Task 3: Bump version to v1.0.0 across all manifests and workflows | Batch C | `Cargo.toml`, `crates/code-kb-cli/Cargo.toml`, `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`, `.claude-plugin/marketplace.json`, `.github/workflows/release-binaries.yml`, `docs/site/index.html`, `Cargo.lock` | Yes | Serialized after compatibility verification. |
| Task 4: Release Pre-Flight Verification and Final Validation | Batch D | Validation & release readiness | Yes | Final gate before merge and release. |

---

## Tasks

### Task 1: Pin `julie-extract` v2.42.3 and update extractor references

**Files:**
- `scripts/julie-pins.json`
- `crates/code-kb-core/src/sync.rs`
- `crates/code-kb-cli/build.rs`
- `crates/code-kb-core/src/telemetry.rs`
- `AGENTS.md`
- `CLAUDE.md`

**Contract inputs:**
- New version: `2.42.3`
- Platform asset SHA256 checksums from GitHub release `v2.42.3`:
  - `aarch64-apple-darwin`: `1eb4dc22908514c068a51a440bd75281053df4e8afa86ecbd0571bff26250266`
  - `x86_64-apple-darwin`: `821c663b87a6a3c8fc59df377d9a88d9411749041faaf5963a0e7d18e1836d33`
  - `x86_64-unknown-linux-gnu`: `01cf191625f5f1d59d67df44aab0117c1703501b05d31c8cb446c387b76155a9`
  - `x86_64-pc-windows-msvc`: `6409c3a2e9cce73c6981703997970fd48ec92e04cebb806a8c30a096435482ec`

**File ownership:**
`scripts/julie-pins.json`, `crates/code-kb-core/src/sync.rs`, `crates/code-kb-cli/build.rs`, `crates/code-kb-core/src/telemetry.rs`, `AGENTS.md`, `CLAUDE.md`.

**Serialization required:** No.

**Dependency reason:** None - safe parallel batch.

**What to build:**
1. Update `scripts/julie-pins.json` to version `2.42.3` and set the corresponding SHA256 hashes for all 4 targets.
2. Update `crates/code-kb-core/src/sync.rs` `PINNED_JULIE_VERSION` constant to `"2.42.3"`.
3. Update `crates/code-kb-cli/build.rs` fallback string to `"2.42.3".to_string()`.
4. Update `crates/code-kb-core/src/telemetry.rs` test assertion to use `crate::sync::PINNED_JULIE_VERSION`.
5. Update `AGENTS.md` and `CLAUDE.md` to reference `currently 2.42.3`, ensuring `cmp AGENTS.md CLAUDE.md` remains identical.
6. Run `bash scripts/restore-julie-extract.sh` to download and install `julie-extract` v2.42.3 into `.tools/julie-extract`.
7. Verify `.tools/julie-extract --version` reports `julie-extract 2.42.3`.

**Acceptance criteria:**
- [x] `scripts/julie-pins.json` contains `"version": "2.42.3"` and verified SHA256 hashes.
- [x] `crates/code-kb-core/src/sync.rs` declares `pub const PINNED_JULIE_VERSION: &str = "2.42.3";`.
- [x] `crates/code-kb-cli/build.rs` uses `"2.42.3"`.
- [x] `AGENTS.md` and `CLAUDE.md` match byte-for-byte and document `2.42.3`.
- [x] `scripts/restore-julie-extract.sh` successfully downloads and verifies checksum of v2.42.3.
- [x] `cargo check --workspace` compiles cleanly.

---

### Task 2: Full Compatibility Verification against `julie-extract` v2.42.3

**Files:**
- Entire test suite

**Contract inputs:**
- Installed binary `.tools/julie-extract` (v2.42.3).
- Windows VM (`win-test`) running.

**File ownership:**
Workspace test suites.

**Serialization required:** Yes.

**Dependency reason:** Requires Task 1 binary restore.

**What to build:**
1. Execute full local test suite: `cargo test --workspace`.
2. Run code style and lint verification: `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
3. Synchronize worktree with Windows guest via `win-test sync code-kb` and run the test suite on Windows NTFS:
   `win-test run code-kb -- cargo test`.
4. Verify AST facts extraction, SQLite schema v7 integrity, and query behavior against real repository indexes.

**Acceptance criteria:**
- [ ] All workspace tests pass on Linux (`cargo test --workspace`).
- [ ] Clippy reports zero warnings (`cargo clippy --workspace --all-targets -- -D warnings`).
- [ ] Code formatting is pristine (`cargo fmt --all -- --check`).
- [ ] Windows test suite passes on the Prax Windows 11 VM (`win-test`).
- [ ] AST extraction facts with `julie-extract` v2.42.3 are fully compatible with `code-kb`.

---

### Task 3: Bump version to `v1.0.0` across all manifests and workflows

**Files:**
- `Cargo.toml`
- `crates/code-kb-cli/Cargo.toml`
- `.claude-plugin/plugin.json`
- `.codex-plugin/plugin.json`
- `.claude-plugin/marketplace.json`
- `.github/workflows/release-binaries.yml`
- `docs/site/index.html`
- `Cargo.lock`

**Contract inputs:**
- Target version: `1.0.0`.

**File ownership:**
`Cargo.toml`, `crates/code-kb-cli/Cargo.toml`, `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`, `.claude-plugin/marketplace.json`, `.github/workflows/release-binaries.yml`, `docs/site/index.html`, `Cargo.lock`.

**Serialization required:** Yes.

**Dependency reason:** Requires verified compatibility in Task 2.

**What to build:**
1. Bump workspace version in root `Cargo.toml` to `"1.0.0"`.
2. Bump `code-kb-core` dependency in `crates/code-kb-cli/Cargo.toml` to `"1.0.0"`.
3. Bump `"version"` in `.claude-plugin/plugin.json` to `"1.0.0"`.
4. Bump `"version"` in `.codex-plugin/plugin.json` to `"1.0.0"`.
5. Bump `"version"` in `.claude-plugin/marketplace.json` to `"1.0.0"`.
6. Update default version input in `.github/workflows/release-binaries.yml` to `"1.0.0"`.
7. Update `docs/site/index.html` badge and stylesheet link to `v1.0.0`.
8. Synchronize `Cargo.lock` by running `cargo check --workspace --all-targets`.

**Acceptance criteria:**
- [ ] Root `Cargo.toml` version is `"1.0.0"`.
- [ ] `crates/code-kb-cli/Cargo.toml` specifies `code-kb-core = { version = "1.0.0" }`.
- [ ] All 3 plugin manifests reflect `"1.0.0"`.
- [ ] Release workflow default input is `"1.0.0"`.
- [ ] `Cargo.lock` is cleanly updated.

---

### Task 4: Release Pre-Flight Verification and Final Validation

**Files:**
- `scripts/release-preflight.sh`

**Contract inputs:**
- Clean working directory or staged release commits.

**File ownership:**
Read-only validation.

**Serialization required:** Yes.

**Dependency reason:** Final end-to-end gate.

**What to build:**
1. Run `./scripts/release-preflight.sh` and verify all 7 steps pass:
   - Sync contracts (`AGENTS.md` == `CLAUDE.md`, `SKILL.md` copies match).
   - Version consistency across all manifests.
   - `cargo fmt --all -- --check`.
   - `cargo clippy --workspace --all-targets -- -D warnings`.
   - `cargo test --workspace`.
   - `cargo package --workspace --no-verify --allow-dirty`.
   - Windows NTFS verification on VM.
2. Confirm package readiness and prepare release documentation and merge steps.

**Acceptance criteria:**
- [ ] `./scripts/release-preflight.sh` exits with code 0 and reports all 7 checks OK.
- [ ] Workspace packages cleanly for crates.io publication.
- [ ] Release instructions and verification report prepared.
