#!/usr/bin/env python3
"""Check the caller-facing Flask WSGI search ranking against a real checkout."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys

QUERY = "incoming WSGI request response error handling"
EXPECTED_TOP_TWO = ["wsgi_app", "full_dispatch_request"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path, help="built code-kb executable")
    parser.add_argument("--root", required=True, type=Path, help="Flask repository checkout")
    args = parser.parse_args()

    completed = subprocess.run(
        [
            str(args.binary.resolve()),
            "search",
            QUERY,
            "--root",
            str(args.root.resolve()),
            "--path",
            "src/flask",
            "--json",
            "--limit",
            "20",
        ],
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        print(completed.stderr, file=sys.stderr, end="")
        return completed.returncode

    try:
        rows = json.loads(completed.stdout)
        names = [row["symbol"]["name"] for row in rows]
    except (json.JSONDecodeError, KeyError, TypeError) as error:
        print(f"invalid code-kb JSON response: {error}", file=sys.stderr)
        return 2

    actual_top_two = names[:2]
    if actual_top_two != EXPECTED_TOP_TWO:
        print(
            f"FAIL: {QUERY!r} expected top two {EXPECTED_TOP_TWO}, "
            f"got {actual_top_two}; top 20: {names}",
            file=sys.stderr,
        )
        return 1

    print(f"PASS: {QUERY!r} top two: {actual_top_two}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
