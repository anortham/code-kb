#!/usr/bin/env python3
"""Compare the time and memory of `code-kb blast-radius` for two binaries on the corpus projects.

For each project the script picks fixed targets from its index: the symbols with the most calls to
their name, a seeded sample of functions and methods, and the source files with the most symbols.
It runs both binaries on each target, in alternate order on each repeat, and records the wall time
and the peak memory of each call. Both binaries read the same index, so they must use the same
julie-extract.

Usage:
  scripts/blast_perf.py --old PATH --new PATH [--repo NAME] [--targets 30] [--repeat 3] [--depth 2]

It exits 1 when a project's median or p95 time, or its peak memory, regresses past the limits below.
"""

import argparse
import json
import os
import random
import sqlite3
import statistics
import subprocess
import sys
import time
from pathlib import Path

from edit_replay import load_json

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "evals/reach/corpus.json"
REPOS_DIR = Path.home() / ".cache/code-kb-accuracy/phase3/repos"
TIME_RATIO_LIMIT = 1.5
TIME_FLOOR_SECONDS = 0.05
MEMORY_RATIO_LIMIT = 1.3
MEMORY_FLOOR_MIB = 20
CALLABLE = ("function", "method", "constructor")
TEST_PATH = ("test", "spec", "fixture")


def is_test_path(path):
    lowered = path.lower()
    return any(part in lowered for part in TEST_PATH)


def pick_targets(db, name, count):
    """Symbol ids and file paths to call, fixed for one index: hubs, a seeded sample, and big files."""
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    marks = ",".join("?" * len(CALLABLE))
    rows = [row for row in conn.execute(
        f"""SELECT s.symbol_id, s.name, s.path,
                   (SELECT count(*) FROM pending_relationships p WHERE p.target_terminal_name = s.name)
                 + (SELECT count(*) FROM relationships r WHERE r.to_symbol_id = s.symbol_id)
            FROM symbols s WHERE s.kind IN ({marks})""", CALLABLE) if not is_test_path(row[2])]
    files = [row[0] for row in conn.execute(
        "SELECT path FROM symbols GROUP BY path ORDER BY count(*) DESC, path LIMIT 200") if not is_test_path(row[0])]
    conn.close()
    hubs = sorted(rows, key=lambda row: (-row[3], row[2], row[1]))[: count // 3]
    rest = sorted(row for row in rows if row not in hubs)
    sample = random.Random(name).sample(rest, min(len(rest), count - len(hubs) - count // 6))
    targets = [["--symbol-id", row[0]] for row in hubs + sample]
    return targets + [["--file", path] for path in files[: count // 6]]


def call(binary, root, target, depth):
    """Wall time, peak memory in MiB, and the test count of one CLI call."""
    started = time.perf_counter()
    process = subprocess.Popen([binary, "--root", str(root), "blast-radius", *target, "--depth", str(depth),
                                "--limit", "20", "--json"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               env=dict(os.environ, CODE_KB_NO_TELEMETRY="1"))
    out = process.stdout.read()
    process.stdout.close()
    _, status, usage = os.wait4(process.pid, 0)
    process.returncode = os.waitstatus_to_exitcode(status)
    row = {"seconds": time.perf_counter() - started, "rss_mib": usage.ru_maxrss / 1024}
    try:
        answer = json.loads(out)
    except json.JSONDecodeError:
        return {**row, "error": True}
    return {**row, "tests": answer.get("likely_tests_found", 0),
            "ceiling": answer.get("traversal_ceiling_reached", False)}


def percentile(values, share):
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(share * len(ordered)))]


def summarize(rows):
    seconds = [row["seconds"] for row in rows]
    return {"p50": statistics.median(seconds), "p95": percentile(seconds, 0.95), "max": max(seconds),
            "rss": max(row["rss_mib"] for row in rows), "errors": sum(1 for row in rows if row.get("error")),
            "ceiling": sum(1 for row in rows if row.get("ceiling")),
            "tests": statistics.median(row.get("tests", 0) for row in rows)}


def regressed(old, new):
    slow = any(new[key] > old[key] * TIME_RATIO_LIMIT and new[key] - old[key] > TIME_FLOOR_SECONDS
               for key in ("p50", "p95"))
    heavy = new["rss"] > old["rss"] * MEMORY_RATIO_LIMIT and new["rss"] - old["rss"] > MEMORY_FLOOR_MIB
    return slow or heavy


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--old", required=True)
    parser.add_argument("--new", required=True)
    parser.add_argument("--repo", action="append")
    parser.add_argument("--targets", type=int, default=30)
    parser.add_argument("--repeat", type=int, default=3)
    parser.add_argument("--depth", type=int, default=2)
    parser.add_argument("--json", help="write every call to this file")
    args = parser.parse_args(argv)
    binaries = {"old": str(Path(args.old).resolve()), "new": str(Path(args.new).resolve())}
    calls = []
    failed = False
    print("| Repo | Calls | Old p50 / p95 / max | New p50 / p95 / max | Old / new peak MiB | Median tests old / new | Ceiling old / new |")
    print("|---|---|---|---|---|---|---|")
    corpus = load_json(CORPUS_PATH)
    for entry in corpus["repos"] if isinstance(corpus, dict) else corpus:
        if args.repo and entry["name"] not in args.repo:
            continue
        root = REPOS_DIR / entry["name"]
        if not (root / ".code-kb/artifact.db").exists():
            continue
        for binary in binaries.values():
            subprocess.run([binary, "--root", str(root), "stats"], capture_output=True, check=False,
                           env=dict(os.environ, CODE_KB_NO_TELEMETRY="1"))
        targets = pick_targets(root / ".code-kb/artifact.db", entry["name"], args.targets)
        rows = {"old": [], "new": []}
        for repeat in range(args.repeat):
            for target in targets:
                order = ("old", "new") if (repeat + len(rows["old"])) % 2 == 0 else ("new", "old")
                for side in order:
                    row = call(binaries[side], root, target, args.depth)
                    rows[side].append(row)
                    calls.append({"repo": entry["name"], "side": side, "target": target, **row})
        old, new = summarize(rows["old"]), summarize(rows["new"])
        flag = " REGRESSED" if regressed(old, new) else ""
        failed |= bool(flag)
        print(f"| {entry['name']}{flag} | {len(rows['new'])} | {old['p50']:.3f} / {old['p95']:.3f} / {old['max']:.3f} | "
              f"{new['p50']:.3f} / {new['p95']:.3f} / {new['max']:.3f} | {old['rss']:.0f} / {new['rss']:.0f} | "
              f"{old['tests']:g} / {new['tests']:g} | {old['ceiling']} / {new['ceiling']} |", flush=True)
    if args.json:
        Path(args.json).write_text("".join(json.dumps(row) + "\n" for row in calls))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
