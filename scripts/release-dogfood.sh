#!/usr/bin/env bash
#
# release-dogfood.sh — run one dogfood round of the local build in Claude Code and Codex.
#
# Each round writes target/release-dogfood/report.md for the lead to fill in, and the
# session answers under target/release-dogfood/<UTC time>/. Preflight step 10 reads the report.
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
COMMIT="$(git rev-parse HEAD)"
CODE_KB_VER="$(target/release/code-kb --version | awk '{print $2}')"
JULIE_VER="$(.tools/julie-extract --version | awk '{print $2}')"

mkdir -p "${OUT}"
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
      --skip-git-repo-check -s read-only -o "${OUT}/codex.md" - <"${prompt}" >"${OUT}/codex.log" 2>&1) & ;;
  esac
}

run_session claude
run_session codex
FAILED=0
wait -n || FAILED=1
wait -n || FAILED=1

cat >"${OUT_ROOT}/report.md" <<EOF
# Release dogfood report

Commit: ${COMMIT}
New: code-kb ${CODE_KB_VER}, julie-extract ${JULIE_VER}
Sessions: ${ROUND}/claude.md, ${ROUND}/codex.md

## Defects

Check every defect that a session reports against the source. List each one here as:
- [open] <defect>
- [not a defect] <defect>: <why the output is correct>
- [deferred: "<the user's own words>"] <defect>

A fixed defect needs a new round, because this report covers commit ${COMMIT:0:7} only.

## Result: PENDING
EOF

echo "Round ${ROUND}: answers in ${OUT}, report template at ${OUT_ROOT}/report.md"
if [[ "${FAILED}" -ne 0 ]]; then
  echo "error: a session failed; read ${OUT}/*.log." >&2
  exit 1
fi
