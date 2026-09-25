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
echo -n "[1/10] Verifying sync contracts (AGENTS.md vs CLAUDE.md, SKILL.md copies)... "
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
echo -n "[2/10] Checking version consistency across manifests... "
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
echo -n "[3/10] Checking code formatting (cargo fmt)... "
if cargo fmt --all -- --check >/dev/null 2>&1; then
  echo "OK"
else
  echo "FAIL"
  echo "Run 'cargo fmt --all' to fix formatting." >&2
  exit 1
fi

# 4. Extractor build guard & clippy
echo "[4/10] Running clippy across workspace..."
cargo clippy --workspace --all-targets -- -D warnings
echo "OK (clippy clean)"

# 5. Full test suite
echo "[5/10] Running full test suite (cargo test --workspace)..."
cargo test --workspace
echo "OK (all tests passed)"
echo "      Running plugin launcher and manifest tests..."
node --test tests/plugin/*.test.cjs
echo "OK (plugin tests passed)"

# 6. Workspace package dry-run
echo "[6/10] Verifying code-kb workspace packaging..."
cargo package --workspace --no-verify --allow-dirty
echo "OK (workspace packages cleanly)"

# 7. Local Windows NTFS verification (if win-test is running)
echo -n "[7/10] Checking Prax Windows 11 VM (win-test)... "
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

# 8. GitHub CI must have passed on the exact commit that will be tagged
echo -n "[8/10] Checking GitHub CI for HEAD... "
HEAD_SHA=$(git rev-parse HEAD)
if [[ -n "$(git status --porcelain)" ]]; then
  echo "FAIL"
  echo "error: working tree is dirty; commit and push before tagging." >&2
  exit 1
fi
if ! git merge-base --is-ancestor "${HEAD_SHA}" "$(git rev-parse origin/main)" 2>/dev/null; then
  echo "FAIL"
  echo "error: HEAD ${HEAD_SHA} is not on origin/main; push it and let CI run first." >&2
  exit 1
fi
CI_STATUS=$(gh run list --workflow=ci.yml --commit "${HEAD_SHA}" --limit 1 --json status,conclusion \
  -q '.[0] | "\(.status)/\(.conclusion)"' 2>/dev/null || echo "unavailable")
if [[ "${CI_STATUS}" != "completed/success" ]]; then
  echo "FAIL"
  echo "error: CI for ${HEAD_SHA} is '${CI_STATUS}', not 'completed/success'." >&2
  echo "       Wait for it (gh run watch) or fix it before tagging." >&2
  exit 1
fi
echo "OK (CI passed on ${HEAD_SHA:0:7})"

# 9. Real-project corpus check of this build against the previous release
echo -n "[9/10] Checking the real-project corpus report... "
CORPUS_REPORT="${REPO_ROOT}/target/release-corpus/report.md"
PIN_VER=$(python3 -c "import json;print(json.load(open('${REPO_ROOT}/scripts/julie-pins.json'))['version'])")
if [[ ! -f "${CORPUS_REPORT}" ]] \
  || ! grep -q "^## Result: PASS" "${CORPUS_REPORT}" \
  || ! grep -q "new: code-kb ${WS_VER}, julie-extract ${PIN_VER}" "${CORPUS_REPORT}" \
  || [[ $(stat -c %Y "${CORPUS_REPORT}") -lt $(git log -1 --format=%ct HEAD) ]]; then
  echo "FAIL"
  echo "error: run scripts/release-corpus-check.py on this commit with code-kb ${WS_VER} and" >&2
  echo "       julie-extract ${PIN_VER}, get PASS, and review its call-edge samples (docs/RELEASING.md)." >&2
  exit 1
fi
echo "OK (PASS for code-kb ${WS_VER} with julie-extract ${PIN_VER})"

# 10. Dogfood round in real harness sessions with no open defect
echo -n "[10/10] Checking the dogfood report... "
DOGFOOD_REPORT="${REPO_ROOT}/target/release-dogfood/report.md"
DOGFOOD_COMMIT=$(sed -n 's/^Commit: //p' "${DOGFOOD_REPORT}" 2>/dev/null || true)
DOGFOOD_ERROR=""
if [[ ! -f "${DOGFOOD_REPORT}" ]]; then
  DOGFOOD_ERROR="no report; run scripts/release-dogfood.sh"
elif ! grep -q "^## Result: PASS$" "${DOGFOOD_REPORT}"; then
  DOGFOOD_ERROR="the result is not '## Result: PASS'"
elif grep -q "^- \[open\]" "${DOGFOOD_REPORT}"; then
  DOGFOOD_ERROR="the report lists an open defect"
elif ! grep -q "julie-extract ${PIN_VER}$" "${DOGFOOD_REPORT}"; then
  DOGFOOD_ERROR="the round did not use julie-extract ${PIN_VER}"
elif ! git merge-base --is-ancestor "${DOGFOOD_COMMIT}" HEAD 2>/dev/null; then
  DOGFOOD_ERROR="the round commit '${DOGFOOD_COMMIT}' is not an ancestor of HEAD"
elif ! git diff --quiet "${DOGFOOD_COMMIT}" HEAD -- crates ':!crates/*/Cargo.toml'; then
  DOGFOOD_ERROR="code changed after the round; run a new round"
fi
if [[ -n "${DOGFOOD_ERROR}" ]]; then
  echo "FAIL"
  echo "error: ${DOGFOOD_ERROR} (docs/RELEASING.md, step 7)." >&2
  exit 1
fi
echo "OK (no open defect at ${DOGFOOD_COMMIT:0:7})"

echo "========================================================"
echo " Pre-Flight PASSED for code-kb v${WS_VER}"
echo " Ready for release tagging: git tag -a v${WS_VER} -m \"Release v${WS_VER}\""
echo "========================================================"
