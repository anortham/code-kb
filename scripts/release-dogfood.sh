#!/usr/bin/env bash
#
# release-dogfood.sh — run one dogfood round of the local build in Claude Code and Codex.
#
# Each round writes target/release-dogfood/report.md for the lead to fill in, and the
# session answers under target/release-dogfood/<UTC time>/. Preflight validates both artifacts.
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECT="${DOGFOOD_PROJECT:-$HOME/source/flask}"
OUT_ROOT="${REPO_ROOT}/target/release-dogfood"
ROUND="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="${OUT_ROOT}/${ROUND}"

cd "${REPO_ROOT}"
cargo build --release --quiet

MCP_BIN="$(command -v code-kb || true)"
if [[ -z "${MCP_BIN}" || "$(realpath "${MCP_BIN}")" != "$(realpath target/release/code-kb)" ]]; then
  echo "error: code-kb on PATH is '${MCP_BIN:-missing}', not ${REPO_ROOT}/target/release/code-kb." >&2
  echo "       The harness sessions would test another binary. Point PATH or ~/.code-kb/bin/code-kb at it." >&2
  exit 1
fi
for tool in claude codex; do
  command -v "${tool}" >/dev/null || { echo "error: ${tool} is not on PATH." >&2; exit 1; }
done

if ! git diff --quiet HEAD -- crates scripts/julie-pins.json; then
  echo "error: uncommitted code changes; commit them so the report names the code it tested." >&2
  exit 1
fi
CODE_KB_VER="$(target/release/code-kb --version | awk '{print $2}')"
JULIE_VER="$(.tools/julie-extract --version | awk '{print $2}')"

mkdir -p "${OUT}"
EVIDENCE="${OUT_ROOT}/evidence.json"
python3 "${REPO_ROOT}/scripts/release_evidence.py" start-dogfood \
  --repo "${REPO_ROOT}" --extractor "${REPO_ROOT}/.tools/julie-extract" \
  --binary "${REPO_ROOT}/target/release/code-kb" --session-dir "${OUT}" --out "${EVIDENCE}"
WORKTREES=()
cleanup() {
  for wt in "${WORKTREES[@]}"; do git -C "${PROJECT}" worktree remove --force "${wt}" || true; rmdir "$(dirname "${wt}")" || true; done
}
trap cleanup EXIT

run_session() {
  local harness="$1" wt
  wt="$(mktemp -d)/dogfood-${harness}"
  case "${wt}" in
    "${REPO_ROOT}"/*) echo "error: ${wt} is inside ${REPO_ROOT}; the harness would load its .mcp.json. Set TMPDIR outside the repo." >&2; exit 1 ;;
  esac
  git -C "${PROJECT}" worktree add --detach --quiet "${wt}"
  WORKTREES+=("${wt}")
  # A worktree copies its parent's index, and a local julie build keeps the pinned version
  # string, so only a forced scan makes the session read what this julie-extract writes.
  target/release/code-kb scan --root "${wt}" --force >/dev/null
  local prompt="${OUT}/${harness}-prompt.md"
  sed "s#PROJECT_ROOT#${wt}#g" scripts/release-dogfood-prompt.md >"${prompt}"
  case "${harness}" in
    claude) (cd "${wt}" && claude -p --permission-mode bypassPermissions <"${prompt}" >"${OUT}/claude.md" 2>"${OUT}/claude.log") & ;;
    codex) (cd "${wt}" && codex exec -c 'mcp_servers.code-kb.default_tools_approval_mode="approve"' \
      -c "mcp_servers.code-kb.command=\"${REPO_ROOT}/target/release/code-kb\"" \
      --skip-git-repo-check -s workspace-write -o "${OUT}/codex.md" - <"${prompt}" >"${OUT}/codex.log" 2>&1) & ;;
  esac
}

run_session claude
run_session codex
FAILED=0
wait -n || FAILED=1
wait -n || FAILED=1

if [[ "${FAILED}" -ne 0 ]]; then
  echo "error: a session failed; read ${OUT}/*.log." >&2
  exit 1
fi

python3 "${REPO_ROOT}/scripts/release_evidence.py" finalize-dogfood --evidence "${EVIDENCE}"
python3 "${REPO_ROOT}/scripts/release_evidence.py" render-dogfood-report \
  --evidence "${EVIDENCE}" --out "${OUT_ROOT}/report.md" \
  --code-kb-version "${CODE_KB_VER}" --julie-version "${JULIE_VER}" --round "${ROUND}"

echo "Round ${ROUND}: answers in ${OUT}, report template at ${OUT_ROOT}/report.md"
