#!/usr/bin/env bash
#
# release-preflight.sh — automated pre-flight verification for code-kb releases
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

cd "${REPO_ROOT}"
mkdir -p "${REPO_ROOT}/target/tmp"
export TMPDIR="${REPO_ROOT}/target/tmp"

echo "========================================================"
echo " Starting code-kb Release Pre-Flight Verification"
echo "========================================================"

# 1. Sync contract (AGENTS.md vs CLAUDE.md and SKILL.md)
echo -n "[1/7] Verifying sync contracts (AGENTS.md vs CLAUDE.md, SKILL.md copies)... "
if ! cmp -s AGENTS.md CLAUDE.md; then
  echo "FAIL"
  echo "error: AGENTS.md and CLAUDE.md differ. Keep them byte-for-byte identical." >&2
  cmp AGENTS.md CLAUDE.md || true
  exit 1
fi
for skill in skills/*/SKILL.md; do
  if ! cmp -s "$skill" ".claude-plugin/$skill"; then
    echo "FAIL"
    echo "error: $skill and .claude-plugin/$skill differ." >&2
    cmp "$skill" ".claude-plugin/$skill" || true
    exit 1
  fi
done
echo "OK (byte-for-byte identical)"

# 2. Version consistency check
echo -n "[2/7] Checking version consistency across manifests... "
WS_VER=$(grep -m 1 '^version = ' Cargo.toml | awk -F'"' '{print $2}')
CLI_CORE_VER=$(grep 'code-kb-core = { version = ' crates/code-kb-cli/Cargo.toml | awk -F'"' '{print $2}')
PLUGIN_VER=$(grep '"version":' .claude-plugin/plugin.json | awk -F'"' '{print $4}')
CODEX_PLUGIN_VER=$(grep '"version":' .codex-plugin/plugin.json | awk -F'"' '{print $4}')
MARKETPLACE_VER=$(grep '"version":' .claude-plugin/marketplace.json | awk -F'"' '{print $4}')
ROOT_PLUGIN_VER=$(grep '"version":' plugin.json | awk -F'"' '{print $4}')

if [[ "${WS_VER}" != "${CLI_CORE_VER}" ]]; then
  echo "FAIL"
  echo "error: Cargo.toml version (${WS_VER}) != code-kb-cli dependency version (${CLI_CORE_VER})" >&2
  exit 1
fi
if [[ "${WS_VER}" != "${PLUGIN_VER}" ]]; then
  echo "FAIL"
  echo "error: Cargo.toml version (${WS_VER}) != .claude-plugin/plugin.json version (${PLUGIN_VER})" >&2
  exit 1
fi
if [[ "${WS_VER}" != "${CODEX_PLUGIN_VER}" ]]; then
  echo "FAIL"
  echo "error: Cargo.toml version (${WS_VER}) != .codex-plugin/plugin.json version (${CODEX_PLUGIN_VER})" >&2
  exit 1
fi
if [[ "${WS_VER}" != "${MARKETPLACE_VER}" ]]; then
  echo "FAIL"
  echo "error: Cargo.toml version (${WS_VER}) != .claude-plugin/marketplace.json version (${MARKETPLACE_VER})" >&2
  exit 1
fi
if [[ "${WS_VER}" != "${ROOT_PLUGIN_VER}" ]]; then
  echo "FAIL"
  echo "error: Cargo.toml version (${WS_VER}) != plugin.json version (${ROOT_PLUGIN_VER})" >&2
  exit 1
fi
echo "OK (v${WS_VER})"

# 3. Formatting check
echo -n "[3/7] Checking code formatting (cargo fmt)... "
if cargo fmt --all -- --check >/dev/null 2>&1; then
  echo "OK"
else
  echo "FAIL"
  echo "Run 'cargo fmt --all' to fix formatting." >&2
  exit 1
fi

# 4. Extractor build guard & clippy
echo "[4/7] Running clippy across workspace..."
cargo clippy --workspace --all-targets -- -D warnings
echo "OK (clippy clean)"

# 5. Full test suite
echo "[5/7] Running full test suite (cargo test --workspace)..."
cargo test --workspace
echo "OK (all tests passed)"
echo "      Running plugin launcher and manifest tests..."
node --test tests/plugin/*.test.cjs
echo "OK (plugin tests passed)"

# 6. Workspace package dry-run
echo "[6/7] Verifying code-kb workspace packaging..."
cargo package --workspace --no-verify --allow-dirty
echo "OK (workspace packages cleanly)"

# 7. Local Windows NTFS verification (if win-test is running)
echo -n "[7/7] Checking Prax Windows 11 VM (win-test)... "
if command -v win-test >/dev/null 2>&1; then
  STATE=$(win-test status 2>/dev/null || echo "shut off")
  if [[ "${STATE}" =~ "running" ]]; then
    echo "running."
    echo "      Running quick Windows test on guest..."
    # If working directory is clean, run sync and test
    if [[ -z "$(git status --porcelain)" ]]; then
      REPO_BASENAME="$(basename "${REPO_ROOT}")"
      win-test sync "${REPO_ROOT}"
      win-test run "${REPO_BASENAME}" -- cargo test -p code-kb-core --test worktree_test
      echo "OK (Windows guest verified)"
    else
      echo "SKIPPED Windows sync (working directory has uncommitted files)."
    fi
  else
    echo "guest is ${STATE} (skipping local Windows test; CI will run it)."
  fi
else
  echo "win-test CLI not found on PATH (skipping local Windows test)."
fi

echo "========================================================"
echo " Pre-Flight PASSED for code-kb v${WS_VER}"
echo " Ready for release tagging: git tag -a v${WS_VER} -m \"Release v${WS_VER}\""
echo "========================================================"
