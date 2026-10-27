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

# 1. Sync contract (AGENTS.md vs CLAUDE.md)
echo -n "[1/11] Verifying sync contract (AGENTS.md vs CLAUDE.md)... "
if ! cmp -s AGENTS.md CLAUDE.md; then
  echo "FAIL"
  echo "error: AGENTS.md and CLAUDE.md differ. Keep them byte-for-byte identical." >&2
  cmp AGENTS.md CLAUDE.md || true
  exit 1
fi
echo "OK (byte-for-byte identical)"

# 2. Version consistency check
echo -n "[2/11] Checking version consistency across manifests... "
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
echo -n "[3/11] Checking code formatting (cargo fmt)... "
if cargo fmt --all -- --check >/dev/null 2>&1; then
  echo "OK"
else
  echo "FAIL"
  echo "Run 'cargo fmt --all' to fix formatting." >&2
  exit 1
fi

# 4. Extractor build guard & clippy
echo "[4/11] Running clippy across workspace..."
cargo clippy --workspace --all-targets -- -D warnings
echo "OK (clippy clean)"

# 5. Full test suite
echo "[5/11] Running full test suite (cargo test --workspace)..."
cargo test --workspace
echo "OK (all tests passed)"
echo "      Running plugin launcher and manifest tests..."
node --test tests/plugin/*.test.cjs
echo "OK (plugin tests passed)"

# 6. Workspace package dry-run
echo "[6/11] Verifying code-kb workspace packaging..."
cargo package --workspace --no-verify --allow-dirty
echo "OK (workspace packages cleanly)"

# 7. Local Windows NTFS verification (if win-test is running)
echo -n "[7/11] Checking Prax Windows 11 VM (win-test)... "
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

# 8. GitHub CI must have passed on the code being released. The version bump reaches main only
# in `git push --atomic origin HEAD:main vX.Y.Z` (docs/RELEASING.md section 3.5), so CI must pass
# on the commit before it, and the bump may change only version files. A HEAD already on origin/main needs green CI itself.
echo -n "[8/11] Checking GitHub CI... "
HEAD_SHA=$(git rev-parse HEAD)
if [[ -n "$(git status --porcelain)" ]]; then
  echo "FAIL"
  echo "error: working tree is dirty; commit before tagging." >&2
  exit 1
fi
VERSION_FILES='^(Cargo\.toml|Cargo\.lock|crates/code-kb-cli/Cargo\.toml|crates/code-kb-core/Cargo\.toml|\.claude-plugin/plugin\.json|\.claude-plugin/marketplace\.json|\.codex-plugin/plugin\.json|plugin\.json|\.github/workflows/release-binaries\.yml|docs/site/index\.html|docs/release-notes/.*)$'
if git merge-base --is-ancestor "${HEAD_SHA}" "$(git rev-parse origin/main)" 2>/dev/null; then
  CI_SHA="${HEAD_SHA}"
else
  CI_SHA=$(git rev-parse HEAD~1)
  if ! git merge-base --is-ancestor "${CI_SHA}" "$(git rev-parse origin/main)" 2>/dev/null; then
    echo "FAIL"
    echo "error: ${CI_SHA:0:7}, the commit before the bump, is not on origin/main; merge it through a pull request and wait for CI." >&2
    exit 1
  fi
  OTHER=$(git diff --name-only "${CI_SHA}" HEAD | grep -Ev "${VERSION_FILES}" || true)
  if [[ -n "${OTHER}" ]]; then
    echo "FAIL"
    echo "error: HEAD is not on origin/main and changes more than version files:" >&2
    echo "${OTHER}" | sed 's/^/       /' >&2
    exit 1
  fi
fi
CI_STATUS=$(gh run list --workflow=ci.yml --commit "${CI_SHA}" --limit 1 --json status,conclusion \
  -q '.[0] | "\(.status)/\(.conclusion)"' 2>/dev/null || echo "unavailable")
if [[ "${CI_STATUS}" != "completed/success" ]]; then
  echo "FAIL"
  echo "error: CI for ${CI_SHA} is '${CI_STATUS}', not 'completed/success'." >&2
  echo "       Wait for it (gh run watch) or fix it before tagging." >&2
  exit 1
fi
echo "OK (CI passed on ${CI_SHA:0:7})"

# 9. Real-project corpus check of this build against the previous release
echo -n "[9/11] Checking the real-project corpus report... "
CORPUS_REPORT="${REPO_ROOT}/target/release-corpus/report.md"
PIN_VER=$(python3 -c "import json;print(json.load(open('${REPO_ROOT}/scripts/julie-pins.json'))['version'])")
CORPUS_ERROR=""
if [[ ! -f "${CORPUS_REPORT}" ]]; then
  CORPUS_ERROR="no report at ${CORPUS_REPORT}"
elif ! grep -q "^## Result: PASS" "${CORPUS_REPORT}"; then
  CORPUS_ERROR="the result is not '## Result: PASS'"
elif ! grep -q "new: code-kb ${WS_VER}, julie-extract ${PIN_VER}" "${CORPUS_REPORT}"; then
  CORPUS_ERROR="the report is not for code-kb ${WS_VER} with julie-extract ${PIN_VER}"
else
  CORPUS_MTIME=$(python3 -c 'import os, sys; print(int(os.path.getmtime(sys.argv[1])))' "${CORPUS_REPORT}" 2>/dev/null || true)
  if [[ ! "${CORPUS_MTIME}" =~ ^[0-9]+$ ]]; then
    CORPUS_ERROR="cannot read the modification time of ${CORPUS_REPORT}"
  elif (( CORPUS_MTIME < $(git log -1 --format=%ct HEAD) )); then
    CORPUS_ERROR="the report is older than the HEAD commit"
  fi
fi
if [[ -n "${CORPUS_ERROR}" ]]; then
  echo "FAIL"
  echo "error: ${CORPUS_ERROR}." >&2
  echo "       Run scripts/release-corpus-check.py on this commit with code-kb ${WS_VER} and" >&2
  echo "       julie-extract ${PIN_VER}, get PASS, and review its call-edge samples (docs/RELEASING.md)." >&2
  exit 1
fi
echo "OK (PASS for code-kb ${WS_VER} with julie-extract ${PIN_VER})"

# 10. Reach gate: ground-truth score of this build on the pinned corpus
echo -n "[10/11] Checking the reach gate report... "
REACH_REPORT="${REPO_ROOT}/target/reach-gate/report.md"
REACH_JSON="${REPO_ROOT}/target/reach-gate/report.json"
REACH_COMMIT=$(sed -n 's/^Commit: //p' "${REACH_REPORT}" 2>/dev/null || true)
REACH_ERROR=""
if [[ ! -f "${REACH_REPORT}" ]]; then
  REACH_ERROR="no report; run scripts/reach_gate.py run"
elif ! grep -q "^## Result: PASS$" "${REACH_REPORT}"; then
  REACH_ERROR="the result is not '## Result: PASS'"
elif ! git merge-base --is-ancestor "${REACH_COMMIT}" HEAD 2>/dev/null; then
  REACH_ERROR="the report commit '${REACH_COMMIT}' is not an ancestor of HEAD"
elif ! python3 scripts/release_evidence.py validate-reach --repo "${REPO_ROOT}" --report "${REACH_JSON}"; then
  REACH_ERROR="the PASS report does not match current semantic inputs; rerun scripts/reach_gate.py run"
fi
if [[ -n "${REACH_ERROR}" ]]; then
  echo "FAIL"
  echo "error: ${REACH_ERROR} (evals/reach/README.md)." >&2
  exit 1
fi
echo "OK (PASS at ${REACH_COMMIT:0:7})"

# 11. Dogfood round in real harness sessions with no open defect
echo -n "[11/11] Checking the dogfood report... "
DOGFOOD_REPORT="${REPO_ROOT}/target/release-dogfood/report.md"
DOGFOOD_EVIDENCE="${REPO_ROOT}/target/release-dogfood/evidence.json"
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
elif ! python3 scripts/release_evidence.py validate-dogfood --repo "${REPO_ROOT}" --evidence "${DOGFOOD_EVIDENCE}" --report "${DOGFOOD_REPORT}"; then
  DOGFOOD_ERROR="the one full round or its recorded local fixes do not cover current inputs"
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
