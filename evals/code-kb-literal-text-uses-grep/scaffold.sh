#!/usr/bin/env bash
set -euo pipefail
mkdir -p src
cat > src/index_wait.rs <<'RS'
pub fn not_ready_message(root: &str) -> String {
    format!("Indexing {root} started; call again in a few seconds.")
}
RS
cat > src/lib.rs <<'RS'
mod index_wait;
RS
