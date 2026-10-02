#!/usr/bin/env bash
set -euo pipefail
git init -q
mkdir -p src/flask
printf 'class Flask:\n    def __init__(self, import_name):\n        self.import_name = import_name\n' > src/flask/app.py
