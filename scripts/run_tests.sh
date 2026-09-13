#!/usr/bin/env bash
set -euo pipefail

echo "==> Running test suite with exact execution timings..."
CI=1 cargo nextest run

echo ""
echo "==> Measuring test coverage across workspace..."
cargo llvm-cov --workspace --no-fail-fast

echo ""
echo "==> Verifying core invariants: MCP schema contains zero workspace parameters..."
cargo test -p code-kb-cli --test mcp_test

echo ""
echo "==> Checking AGENTS.md and CLAUDE.md sync..."
cmp AGENTS.md CLAUDE.md
echo "AGENTS.md and CLAUDE.md are byte-for-byte identical."

echo ""
echo "All test and coverage checks passed successfully!"
