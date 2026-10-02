#!/usr/bin/env bash
# Runs the plugin eval suite in evals/ with `claude plugin eval`.
# The eval command refuses a plugin folder with more than 20000 entries, and target/
# alone exceeds that, so the suite runs against a copy of the tracked files. The copy
# links target/release/code-kb so the plugin hooks run the checkout build.
# Extra arguments go to `claude plugin eval`, for example `--runs 3` or `--case 'telemetry-*'`.
set -euo pipefail

repo=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

git -C "$repo" ls-files -z --cached --others --exclude-standard |
  (cd "$repo" && xargs -0 cp --parents -t "$stage")
if [[ -x "$repo/target/release/code-kb" ]]; then
  mkdir -p "$stage/target/release"
  ln -s "$repo/target/release/code-kb" "$stage/target/release/code-kb"
fi

results="$repo/evals/results"
mkdir -p "$results"
claude plugin eval "$stage" --scaffold --allow-tools Bash Write --trust-plugin \
  --output-dir "$results/$(date -u +%Y%m%dT%H%M%SZ)" "$@"
