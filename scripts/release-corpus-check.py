#!/usr/bin/env python3
"""Compare a release candidate with the previous release on real projects.

Each side is a directory that holds a `code-kb` binary and its `julie-extract`. For every corpus
entry the script scans a `git archive` export with both sides, runs the same CLI tool calls on
both indexes, and writes `report.md`. It exits 1 when the candidate fails a scan, loses symbols,
scans much slower, or breaks a tool call that worked before.

Usage:
  scripts/release-corpus-check.py --old ~/.code-kb/search-eval/bin/v2.1.0 --new target/release
"""

import argparse
import difflib
import os
import random
import re
import shutil
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

COUNTED_TABLES = [
    "files",
    "symbols",
    "relationships",
    "pending_relationships",
    "type_facts",
    "identifiers",
    "parse_diagnostics",
    "skipped_files",
]
PROBE_KINDS = ("function", "method", "class", "struct", "interface", "trait", "enum")
SYMBOL_LOSS_LIMIT = 0.005
SCAN_SLOWDOWN_LIMIT = 1.5
SCAN_SLOWDOWN_FLOOR_SECONDS = 3.0


def read_corpus(path):
    entries = []
    for line in Path(path).read_text().splitlines():
        fields = line.split("#", 1)[0].split()
        if fields:
            name, repo, *rest = fields
            ref = rest[0] if rest else "HEAD"
            subdir = rest[1] if len(rest) > 1 else ""
            entries.append((name, Path(repo).expanduser(), ref, subdir))
    return entries


def export(repo, ref, subdir, target):
    shutil.rmtree(target, ignore_errors=True)
    target.mkdir(parents=True)
    commit = subprocess.run(
        ["git", "-C", repo, "rev-parse", "--short", ref], capture_output=True, text=True, check=True
    ).stdout.strip()
    archive = subprocess.run(
        ["git", "-C", repo, "archive", ref, *([subdir] if subdir else [])],
        capture_output=True,
        check=True,
    ).stdout
    subprocess.run(["tar", "-x", "-C", target], input=archive, check=True)
    return (target / subdir if subdir else target), commit


class Side:
    def __init__(self, label, bin_dir, scratch):
        self.label = label
        self.code_kb = str(Path(bin_dir).expanduser().resolve() / "code-kb")
        extractor = Path(bin_dir).expanduser().resolve() / "julie-extract"
        self.env = dict(os.environ)
        if extractor.exists():
            self.env["JULIE_EXTRACT_BIN"] = str(extractor)
        self.env["CODE_KB_NO_TELEMETRY"] = "1"
        self.env["CODE_KB_TELEMETRY_DIR"] = str(scratch / f"telemetry-{label}")
        self.extractor = subprocess.run(
            [self.env.get("JULIE_EXTRACT_BIN", "julie-extract"), "--version"], capture_output=True, text=True
        ).stdout.split()[-1]
        self.version = f"{self.run(['--version'], Path.cwd())[1].strip()}, julie-extract {self.extractor}"

    def run(self, args, root, db=None):
        cmd = [self.code_kb, "--root", str(root), *(["--db", str(db)] if db else []), *args]
        started = time.perf_counter()
        done = subprocess.run(cmd, capture_output=True, text=True, env=self.env, timeout=900)
        return done.returncode, done.stdout + done.stderr, time.perf_counter() - started


def remove_index(db):
    """Deletes an index with its `-wal` and `-shm` files. A read-only reader leaves them behind,
    and SQLite replays a stale `-wal` onto a new file at the same path."""
    for suffix in ("", "-wal", "-shm"):
        Path(f"{db}{suffix}").unlink(missing_ok=True)


def indexed_by(db):
    """The julie-extract version that wrote the index. code-kb prefers the pinned extractor, so
    this can differ from the binary the side offered."""
    with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as conn:
        row = conn.execute("SELECT value FROM artifact_metadata WHERE key = 'binary_version'").fetchone()
        return row[0] if row else "unknown"


def counts(db):
    with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as conn:
        found = {row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        return {t: conn.execute(f"SELECT count(*) FROM {t}").fetchone()[0] if t in found else 0 for t in COUNTED_TABLES}


def edges(db):
    with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as conn:
        return set(
            conn.execute(
                "SELECT f.name, t.name, r.path, r.start_line FROM relationships r "
                "JOIN symbols f ON f.symbol_id = r.from_symbol_id "
                "JOIN symbols t ON t.symbol_id = r.to_symbol_id"
            )
        )


def typed_receiver_calls(db):
    """`local.method()` calls whose local has a type fact naming a type with that method. code-kb
    resolves these at query time (`pending_target_predicate`), so `relationships` never shows them."""
    with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as conn:
        return set(
            conn.execute(
                "SELECT DISTINCT f.name, owner.name || '.' || m.name, p.path, p.start_line "
                "FROM pending_relationships p "
                "JOIN symbols f ON f.symbol_id = p.from_symbol_id "
                "JOIN symbols r ON r.name = p.target_receiver AND r.parent_symbol_id = p.from_symbol_id "
                "JOIN type_facts t ON t.symbol_id = r.symbol_id "
                "JOIN symbols owner ON owner.name = t.resolved_type "
                "JOIN symbols m ON m.parent_symbol_id = owner.symbol_id AND m.name = p.target_terminal_name"
            )
        )


def probes(db, name):
    """Tool calls chosen from the baseline index, the same for both sides."""
    rng = random.Random(name)
    with sqlite3.connect(f"file:{db}?mode=ro", uri=True) as conn:
        files = [row[0] for row in conn.execute(
            "SELECT path FROM symbols GROUP BY path ORDER BY count(*) DESC, path LIMIT 3")]
        others = [row[0] for row in conn.execute("SELECT DISTINCT path FROM symbols ORDER BY path")]
        files += rng.sample([f for f in others if f not in files], min(3, max(0, len(others) - len(files))))
        marks = ",".join("?" * len(PROBE_KINDS))
        candidates = conn.execute(
            f"SELECT name, path FROM symbols WHERE kind IN ({marks}) AND is_test = 0 "
            "GROUP BY name, path HAVING count(*) = 1 ORDER BY path, name",
            PROBE_KINDS,
        ).fetchall()
    calls = [["outline"], ["facts"]]
    calls += [["skeleton", path] for path in files]
    for symbol, path in rng.sample(candidates, min(8, len(candidates))):
        if re.fullmatch(r"lambda_\d+", symbol):
            continue  # a generated name holds its line, which extractor versions may count differently
        calls += [
            ["lookup", symbol],
            ["search", " ".join(symbol.replace("_", " ").split()[:3]) or symbol],
            ["body", symbol, "-f", path],
            ["context", symbol, "-f", path],
            ["refs", symbol, "-f", path],
            ["refs", symbol, "-f", path, "--direction", "callees"],
            ["blast-radius", symbol, "-f", path],
        ]
    return calls


def looks_broken(code, output):
    return code != 0 or "panicked" in output or not output.strip()


def source_line(root, path, line):
    try:
        return (root / path).read_text(errors="replace").splitlines()[line - 1].strip()[:120]
    except (OSError, IndexError, TypeError):
        return ""


def check_entry(name, root, commit, old, new, out, report):
    failures = []
    report.append(f"\n## {name} ({commit})\n")
    scans = {}
    for side in (old, new):
        db = out / side.label / f"{name}.db"
        db.parent.mkdir(parents=True, exist_ok=True)
        remove_index(db)
        code, output, seconds = side.run(["scan", "--force"], root, db)
        scans[side.label] = (code, seconds, db)
        if code != 0:
            report.append(f"- {side.label} scan failed (exit {code}): `{output.strip()[-300:]}`")
    if scans["new"][0] != 0:
        return [f"{name}: candidate scan failed"]
    if scans["old"][0] != 0:
        report.append("- Baseline scan failed, so only the candidate scan was checked.")
        return failures
    for side in (old, new):
        used = indexed_by(scans[side.label][2])
        if used != side.extractor:
            return failures + [
                f"{name}: the {side.label} index was written by julie-extract {used}, not {side.extractor}. "
                "code-kb prefers its pinned extractor; build the side with that pin."
            ]

    before, after = counts(scans["old"][2]), counts(scans["new"][2])
    report.append("| measure | old | new |\n|---|---:|---:|")
    report.append(f"| scan seconds | {scans['old'][1]:.1f} | {scans['new'][1]:.1f} |")
    report.append(f"| index MB | {scans['old'][2].stat().st_size / 1e6:.1f} | {scans['new'][2].stat().st_size / 1e6:.1f} |")
    for table in COUNTED_TABLES:
        report.append(f"| {table} | {before[table]} | {after[table]} |")
    if after["symbols"] < before["symbols"] * (1 - SYMBOL_LOSS_LIMIT):
        failures.append(f"{name}: symbols dropped from {before['symbols']} to {after['symbols']}")
    if scans["new"][1] > max(scans["old"][1] * SCAN_SLOWDOWN_LIMIT, SCAN_SLOWDOWN_FLOOR_SECONDS):
        failures.append(f"{name}: scan took {scans['new'][1]:.1f}s, was {scans['old'][1]:.1f}s")

    upgraded = out / "upgrade" / f"{name}.db"
    upgraded.parent.mkdir(parents=True, exist_ok=True)
    remove_index(upgraded)
    shutil.copyfile(scans["old"][2], upgraded)
    code, output, seconds = new.run(["outline"], root, upgraded)
    report.append(f"| upgrade of the old index, seconds | | {seconds:.1f} |")
    if looks_broken(code, output) or counts(upgraded)["symbols"] != after["symbols"]:
        failures.append(
            f"{name}: the candidate did not bring the old index up to date "
            f"(exit {code}, {counts(upgraded)['symbols']} symbols, fresh scan has {after['symbols']})"
        )

    rng = random.Random(name)
    for title, collect in (("Call edges", edges), ("Typed receiver calls", typed_receiver_calls)):
        before_rows, after_rows = collect(scans["old"][2]), collect(scans["new"][2])
        gained, lost = sorted(after_rows - before_rows), sorted(before_rows - after_rows)
        report.append(
            f"\n{title}: {len(after_rows)} ({len(gained)} gained, {len(lost)} lost). "
            "Check that each sample is a real call to that target.\n"
        )
        for label, rows in (("gained", gained), ("lost", lost)):
            for caller, callee, path, line in rng.sample(rows, min(10, len(rows))):
                report.append(f"- {label}: `{caller}` -> `{callee}` at {path}:{line} `{source_line(root, path, line)}`")

    changed = 0
    for call in probes(scans["old"][2], name):
        old_code, old_out, _ = old.run(call, root, scans["old"][2])
        new_code, new_out, _ = new.run(call, root, scans["new"][2])
        shown = " ".join(call)
        if looks_broken(new_code, new_out) and not looks_broken(old_code, old_out):
            failures.append(f"{name}: `{shown}` broke (exit {new_code}): {new_out.strip()[:200]}")
        elif old_out != new_out:
            changed += 1
            diff = difflib.unified_diff(old_out.splitlines(), new_out.splitlines(), "old", "new", lineterm="", n=1)
            report.append(f"\n<details><summary>changed: <code>{shown}</code></summary>\n\n```diff")
            report.extend(list(diff)[:60])
            report.append("```\n</details>")
    report.append(f"\nTool calls with changed output: {changed}.")
    return failures


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--old", required=True, help="Directory with the previous release's code-kb and julie-extract")
    parser.add_argument("--new", required=True, help="Directory with the candidate's code-kb and julie-extract")
    parser.add_argument("--corpus", default=str(Path(__file__).with_name("release-corpus.txt")))
    parser.add_argument("--out", default="target/release-corpus", help="Scratch and report directory")
    parser.add_argument("--only", nargs="*", help="Corpus names to run (default: all)")
    args = parser.parse_args()

    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    old, new = Side("old", args.old, out), Side("new", args.new, out)
    report = [f"# Release corpus check\n\nold: {old.version} | new: {new.version}"]
    failures = []
    for name, repo, ref, subdir in read_corpus(args.corpus):
        if args.only and name not in args.only:
            continue
        print(f"== {name}", flush=True)
        root, commit = export(repo, ref, subdir, out / "corpus" / name)
        failures += check_entry(name, root, commit, old, new, out, report)

    report.insert(1, "\n## Result: " + ("FAIL\n\n" + "\n".join(f"- {f}" for f in failures) if failures else "PASS"))
    (out / "report.md").write_text("\n".join(report) + "\n")
    print(f"Report: {out / 'report.md'}")
    print("FAIL:\n" + "\n".join(failures) if failures else "PASS")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
