#!/usr/bin/env bash
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
python3 -I "$here/../fixtures/telemetry_seed.py" "$here/../../.claude-plugin/plugin.json"
