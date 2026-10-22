#!/usr/bin/env python3
"""Planted-bug replay of the tests `blast_radius` lists, on real commits.

For each commit, `truth` plants one bug on a source line the commit added and runs the full test
suite with and without it. The tests that fail only with the bug are the truth. `run` asks
`code-kb blast-radius` for the tests to run, seeded with the symbol around the bug and with the git
diff of the bug, and `score` counts the failing tests the answers left out.

Usage:
  scripts/bug_replay.py truth --repo NAME [--commits 50] [--tries 4]
  scripts/bug_replay.py recheck --repo NAME
  scripts/bug_replay.py run --code-kb PATH --out DIR [--repo NAME] [--split dev|holdout|all]
  scripts/bug_replay.py score --run DIR [--split dev|holdout|all] [--json]

`run` and `score` take the dev half unless `--split` names another; tune on dev only.
"""

import argparse
import hashlib
import json
import os
import random
import re
import sqlite3
import statistics
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from pathlib import Path

from edit_replay import (
    checkout,
    clone_dir,
    git,
    load_json,
    prepare_run,
    replay_tree,
    split_of,
)

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "evals/reach/corpus.json"
TRUTH_DIR = REPO_ROOT / "evals/bug-replay"
REPOS_DIR = Path.home() / ".cache/code-kb-accuracy/phase3/repos"
TRUTH_TREES = Path.home() / ".cache/code-kb-accuracy/bug-replay/trees"
SUITE_TIMEOUT_SECONDS = 1800
BLAST_TIMEOUT_SECONDS = 300
LIMIT = 200
MODES = ("symbol", "diff")
WARNING_FIELD = "open_ends"
DEPTH_FIELD = "tests_past_depth"
COLLAPSED_FILE = "constructor-only callers"
SEED_KINDS = ("function", "method", "constructor", "class", "struct")

C_LIKE_MUTATIONS = [
    (r"==", "!="), (r"!=", "=="), (r"&&", "||"), (r"\|\|", "&&"), (r"\btrue\b", "false"),
    (r"\bfalse\b", "true"), (r"!(?=[A-Za-z_(])", ""), (r"<=", ">"), (r"(?<![=<>])>=", "<"),
    (r"\+ 1\b", "- 1"), (r"- 1\b", "+ 1"), (r"; ok \{", "; !ok {"),
]
PYTHON_MUTATIONS = [
    (r"==", "!="), (r"!=", "=="), (r"\bis not\b", "is"), (r"\bis\b(?! not)", "is not"),
    (r"\bnot\s+", ""), (r"\bTrue\b", "False"), (r"\bFalse\b", "True"), (r"\band\b", "or"),
    (r"\bor\b", "and"), (r"<=", ">"), (r"(?<![-=<>])>=", "<"), (r"(?<![-=<>])>(?![=>])", "<="),
    (r"(?<![<])<(?![=<])", ">="), (r"\+ 1\b", "- 1"), (r"- 1\b", "+ 1"),
    (r"\breturn\s+(?!None\b)\S.*$", "return None"),
]
SCRIPT_MUTATIONS = [(r"===", "!=="), (r"!==", "===")] + C_LIKE_MUTATIONS


class BrokenRun(Exception):
    """The suite did not run: a build failure, a missing tool, or a timeout."""


def sh(args, cwd, timeout=SUITE_TIMEOUT_SECONDS, env=None):
    try:
        return subprocess.run(args, cwd=cwd, capture_output=True, text=True, errors="replace", timeout=timeout,
                              env=env, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise BrokenRun(f"{' '.join(args[:3])}: {error}") from error


def outside_strings(line, index):
    return line[:index].count('"') % 2 == 0 and line[:index].count("'") % 2 == 0


def test_key(path, name):
    """The key a test is compared by: its file and the last segment of its name, without parameters."""
    return path, name.split("::")[-1].split("[")[0].split("/")[0]


class Pytest:
    language = "python"
    mutations = PYTHON_MUTATIONS
    skip = re.compile(r"^\s*(#|import |from |@|def |class |async def |\"|')")

    @staticmethod
    def is_source(path):
        return path.endswith(".py") and path.startswith("src/")

    @staticmethod
    def python(tree):
        """`uv run` when the commit has a lock file, else the tree's `.venv` that a newer commit built;
        its editable install reads the checked-out sources."""
        if (tree / "uv.lock").exists():
            return ["uv", "run", "--frozen", "python"]
        return [str(tree / ".venv/bin/python")]

    @staticmethod
    def prepare(tree):
        if not (tree / "uv.lock").exists() and not (tree / ".venv/bin/python").exists():
            raise BrokenRun("no uv.lock and no .venv")

    @staticmethod
    def full_run(tree):
        with tempfile.TemporaryDirectory() as scratch:
            report = Path(scratch) / "junit.xml"
            env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
            done = sh([*Pytest.python(tree), "-m", "pytest", "-q", "-p", "no:cacheprovider", f"--junitxml={report}"],
                      tree, env=env)
            if not report.exists():
                raise BrokenRun(f"pytest wrote no report: {done.stderr[-500:]}")
            return pytest_outcomes(ET.parse(report).getroot(), tree)


def pytest_outcomes(root, tree):
    outcomes = {}
    for case in root.iter("testcase"):
        dotted = case.get("classname") or ""
        path, classes = module_file(dotted, tree)
        name = "::".join(classes + [case.get("name", "")])
        tags = {child.tag for child in case}
        failed = bool(tags & {"failure", "error"})
        key = (path, name.split("[")[0])
        outcomes[key] = "failed" if failed or outcomes.get(key) == "failed" else "passed"
    return outcomes


def module_file(dotted, tree):
    """The file of a dotted pytest class name, with the classes that follow the module."""
    parts = dotted.split(".")
    for end in range(len(parts), 0, -1):
        candidate = "/".join(parts[:end]) + ".py"
        if (Path(tree) / candidate).exists():
            return candidate, parts[end:]
    return "/".join(parts) + ".py", []


class GoTest:
    language = "go"
    mutations = C_LIKE_MUTATIONS
    skip = re.compile(r"^\s*(//|import |package |func |type |\)|\}|\{|$)")

    @staticmethod
    def is_source(path):
        return path.endswith(".go") and not path.endswith("_test.go")

    @staticmethod
    def prepare(tree):
        if not (tree / "go.mod").exists():
            raise BrokenRun("no go.mod")

    @staticmethod
    def full_run(tree):
        packages = sh(["go", "list", "-f", "{{.ImportPath}} {{.Dir}}", "./..."], tree)
        if packages.returncode != 0:
            raise BrokenRun(packages.stderr[-500:])
        dirs = dict(line.split(" ", 1) for line in packages.stdout.splitlines() if " " in line)
        done = sh(["go", "test", "-json", "-count=1", "./..."], tree)
        return go_outcomes(done.stdout, dirs, tree)


def go_outcomes(stream, dirs, tree):
    outcomes, files = {}, {}
    for line in stream.splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        action, package, test = event.get("Action"), event.get("Package"), event.get("Test")
        if action == "output" and not test and "[build failed]" in event.get("Output", ""):
            raise BrokenRun(f"build failed: {package}")
        if action not in ("pass", "fail") or not test:
            continue
        top = test.split("/")[0]
        folder = Path(dirs.get(package, tree))
        if (package, top) not in files:
            files[(package, top)] = go_test_file(folder, top, tree)
        key = (files[(package, top)], top)
        outcomes[key] = "failed" if action == "fail" or outcomes.get(key) == "failed" else "passed"
    return outcomes


def go_test_file(folder, name, tree):
    pattern = re.compile(rf"^func {re.escape(name)}\(")
    for file in sorted(Path(folder).glob("*_test.go")):
        if any(pattern.match(line) for line in file.read_text(errors="replace").splitlines()):
            return file.relative_to(tree).as_posix()
    return (Path(folder) / "?").relative_to(tree).as_posix()


class Mocha:
    language = "javascript"
    mutations = SCRIPT_MUTATIONS
    skip = re.compile(r"^\s*(//|/\*|\*|import |export |require\(|'use strict'|\}|\{|$)")

    @staticmethod
    def is_source(path):
        return path.endswith(".js") and (path.startswith("lib/") or "/" not in path) and not path.startswith("test")

    @staticmethod
    def prepare(tree):
        """Installs the packages of the checked-out `package.json` unless they are installed already."""
        wanted = hashlib.sha256((tree / "package.json").read_bytes()).hexdigest()
        marker = tree / "node_modules/.bug-replay-package-json"
        if marker.exists() and marker.read_text() == wanted:
            return
        done = sh(["npm", "install", "--no-audit", "--no-fund", "--ignore-scripts", "--no-package-lock"], tree)
        if done.returncode != 0:
            raise BrokenRun(done.stderr[-500:])
        marker.write_text(wanted)

    @staticmethod
    def full_run(tree, files=None):
        """All tests of the `test` script, or only the tests in `files`."""
        package = json.loads((tree / "package.json").read_text())
        script = package.get("scripts", {}).get("test", "")
        args = mocha_command(script, files)
        with tempfile.TemporaryDirectory() as scratch:
            output = Path(scratch) / "report.json"
            done = sh([str(tree / "node_modules/.bin/mocha"), *args, "--reporter", "json",
                       "--reporter-option", f"output={output}"], tree)
            try:
                report = json.loads(output.read_text())
            except (OSError, json.JSONDecodeError) as error:
                raise BrokenRun(f"mocha report: {error}: {done.stderr[-500:]}") from error
        return mocha_outcomes(report, tree)


MOCHA_VALUE_OPTIONS = {"--require", "-r", "--timeout", "-t", "--ui", "-u", "--file", "--extension", "--slow",
                       "-s", "--grep", "-g", "--fgrep", "-f", "--config", "--package", "--spec"}


def mocha_command(script, files=None):
    """The mocha arguments of a `test` script without its reporter, `--bail`, and, when `files` names
    the tests to run, its test folders."""
    args = drop_option([arg for arg in script.split()[1:] if arg not in ("--bail", "--exit")], "--reporter")
    if files is None:
        return args + ["--exit"]
    kept, value = [], False
    for arg in args:
        if value or arg.startswith("-"):
            kept.append(arg)
            value = not value and arg in MOCHA_VALUE_OPTIONS
    return kept + ["--exit", *files]


def drop_option(args, option):
    out, skip = [], False
    for arg in args:
        if skip:
            skip = False
        elif arg == option:
            skip = True
        elif not arg.startswith(option + "="):
            out.append(arg)
    return out


def mocha_outcomes(report, tree):
    outcomes = {}
    for group, outcome in (("passes", "passed"), ("failures", "failed")):
        for test in report.get(group, []):
            file = test.get("file") or ""
            path = Path(file).resolve().relative_to(Path(tree).resolve()).as_posix() if file else "?"
            key = (path, test.get("title", ""))
            outcomes[key] = "failed" if outcome == "failed" or outcomes.get(key) == "failed" else outcome
    return outcomes


RUNNERS = {"flask": Pytest, "cobra": GoTest, "express": Mocha}


def confirmed(tree, runner, failing):
    """The failing tests that also fail when their file runs alone. One mocha process runs every file,
    so an error that one bug throws can fail tests in later files; pytest and go tests do not share
    such a process."""
    if runner is not Mocha:
        return failing
    still = set()
    for path in sorted({path for path, _ in failing}):
        try:
            alone = Mocha.full_run(tree, [path])
        except BrokenRun:
            alone = {}
        still |= {key for key, outcome in alone.items() if outcome == "failed"}
    return [key for key in failing if key in still]


def added_lines(tree, commit, runner):
    """The source lines `commit` added, plus each hunk's first line so a removal still has a place."""
    diff = git(tree, "diff", "-U0", f"{commit}^", commit)
    lines, anchors, path, number = [], [], None, 0
    for row in diff.splitlines():
        if row.startswith("+++ "):
            path = row[6:] if row.startswith("+++ b/") and runner.is_source(row[6:]) else None
        elif row.startswith("@@") and path:
            number = int(re.match(r"@@ -\S+ \+(\d+)", row).group(1))
            anchors.append((path, max(number, 1)))
        elif row.startswith("+") and path:
            lines.append((path, number))
            number += 1
    return lines, anchors


def mutants(tree, commit, runner):
    """Candidate bugs: one operator flip per line, on added lines first, then within 15 lines of them."""
    added, anchors = added_lines(tree, commit, runner)
    near = {(path, n + offset) for path, n in added + anchors for offset in range(-15, 16)} - set(added)
    found = []
    for pool in (added, sorted(near)):
        candidates = []
        for path, number in pool:
            text = (Path(tree) / path).read_text(encoding="utf-8", errors="replace").splitlines()
            if not 1 <= number <= len(text) or runner.skip.match(text[number - 1]):
                continue
            mutated = mutate(text[number - 1], runner.mutations)
            if mutated:
                candidates.append({"path": path, "line": number, "before": text[number - 1], "after": mutated})
        random.Random(commit).shuffle(candidates)
        found += candidates
    return found


def mutate(line, mutations):
    for pattern, replacement in mutations:
        match = next((m for m in re.finditer(pattern, line) if outside_strings(line, m.start())), None)
        if match:
            mutated = line[:match.start()] + re.sub(pattern, replacement, match.group(0)) + line[match.end():]
            if mutated != line:
                return mutated
    return None


def apply_mutant(tree, mutant):
    file = Path(tree) / mutant["path"]
    lines = file.read_text(encoding="utf-8").split("\n")
    if lines[mutant["line"] - 1] != mutant["before"]:
        raise RuntimeError(f"{mutant['path']}:{mutant['line']} does not hold the line the bug replaces")
    lines[mutant["line"] - 1] = mutant["after"]
    file.write_text("\n".join(lines), encoding="utf-8")


def corpus_entry(name):
    for entry in load_json(CORPUS_PATH):
        if entry["name"] == name:
            return entry
    raise SystemExit(f"{name} is not in {CORPUS_PATH}")


def truth_commits(clone, start, runner, count):
    commits = []
    for line in git(clone, "log", "--no-merges", "--format=%H", "-n", "400", start).splitlines():
        paths = git(clone, "diff", "--name-only", f"{line}^", line).splitlines()
        if any(runner.is_source(path) for path in paths):
            commits.append(line)
            if len(commits) == count:
                break
    return commits[::-1]


def truth_case(tree, commit, runner, tries):
    record = {"commit": commit, "subject": git(tree, "log", "-1", "--format=%s", commit), "split": split_of(commit)}
    checkout(tree, commit)
    try:
        runner.prepare(tree)
        base = runner.full_run(tree)
    except BrokenRun as error:
        return {**record, "outcome": "broken", "error": str(error)}
    record["suite_tests"] = len(base)
    record["suite_files"] = len({path for path, _ in base})
    candidates = mutants(tree, commit, runner)
    if not candidates:
        return {**record, "outcome": "no-mutant"}
    attempts = 0
    for mutant in candidates:
        if attempts == tries:
            break
        apply_mutant(tree, mutant)
        try:
            bugged = runner.full_run(tree)
            failing = sorted(key for key, outcome in bugged.items()
                             if outcome == "failed" and base.get(key) == "passed")
            failing = confirmed(tree, runner, failing)
        except BrokenRun:
            continue
        finally:
            git(tree, "checkout", "-q", "--", mutant["path"])
        attempts += 1
        if failing:
            return {**record, "outcome": "detected", "attempts": attempts, "mutant": mutant,
                    "failing": [{"path": path, "name": name} for path, name in failing]}
    return {**record, "outcome": "undetected", "attempts": attempts}


def cmd_truth(args):
    entry = corpus_entry(args.repo)
    runner = RUNNERS.get(args.repo)
    if runner is None:
        raise SystemExit(f"no test runner for {args.repo}; known: {', '.join(RUNNERS)}")
    clone = clone_dir(REPOS_DIR, entry)
    tree = replay_tree(clone, TRUTH_TREES, args.repo)
    out = TRUTH_DIR / f"{args.repo}.jsonl"
    kept = [json.loads(line) for line in out.read_text().splitlines()] if out.exists() else []
    kept = [record for record in kept if record["outcome"] != "broken"]
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("".join(json.dumps(record) + "\n" for record in kept))
    done = {record["commit"] for record in kept}
    for commit in truth_commits(clone, args.start or entry["commit"], runner, args.commits):
        if commit in done:
            continue
        started = time.monotonic()
        record = {"repo": args.repo, "language": runner.language, **truth_case(tree, commit, runner, args.tries)}
        record["seconds"] = round(time.monotonic() - started, 1)
        with out.open("a") as sink:
            sink.write(json.dumps(record) + "\n")
        print(f"{args.repo} {commit[:10]} {record['outcome']} {len(record.get('failing', []))} failing", flush=True)


def cmd_recheck(args):
    """Runs each failing file of the stored bugs alone, with the bug, and keeps the tests that fail."""
    entry = corpus_entry(args.repo)
    runner = RUNNERS[args.repo]
    tree = replay_tree(clone_dir(REPOS_DIR, entry), TRUTH_TREES, args.repo)
    out = TRUTH_DIR / f"{args.repo}.jsonl"
    records = [json.loads(line) for line in out.read_text().splitlines() if line.strip()]
    for record in records:
        if record["outcome"] != "detected":
            continue
        checkout(tree, record["commit"])
        runner.prepare(tree)
        apply_mutant(tree, record["mutant"])
        try:
            failing = [(test["path"], test["name"]) for test in record["failing"]]
            kept = confirmed(tree, runner, failing)
        finally:
            git(tree, "checkout", "-q", "--", record["mutant"]["path"])
        record["failing"] = [{"path": path, "name": name} for path, name in kept]
        if not kept:
            record["outcome"] = "undetected"
        print(f"{args.repo} {record['commit'][:10]} {len(failing)} -> {len(kept)} failing", flush=True)
    out.write_text("".join(json.dumps(record) + "\n" for record in records))


def blast(binary, tree, *args):
    env = dict(os.environ, CODE_KB_NO_TELEMETRY="1")
    done = subprocess.run([str(binary), "blast-radius", "--root", str(tree), "--json", "--limit", str(LIMIT), *args],
                          capture_output=True, text=True, errors="replace", timeout=BLAST_TIMEOUT_SECONDS, env=env,
                          check=False)
    if done.returncode != 0:
        return {"error": done.stderr.strip()[-500:]}
    return json.loads(done.stdout)


def enclosing_symbol(tree, path, line):
    """The innermost function, method, or type of the index that holds `path:line`."""
    marks = ",".join("?" * len(SEED_KINDS))
    db = sqlite3.connect(f"file:{Path(tree) / '.code-kb/artifact.db'}?mode=ro", uri=True)
    try:
        row = db.execute(
            f"SELECT symbol_id, name, kind FROM symbols WHERE path = ? AND start_line <= ? AND end_line >= ? "
            f"AND kind IN ({marks}) ORDER BY end_line - start_line, start_line DESC LIMIT 1",
            (path, line, line, *SEED_KINDS)).fetchone()
    finally:
        db.close()
    return row


def trim_answer(answer):
    keep = ("error", "seed_type", "likely_tests_found", "likely_tests_truncated", "traversal_ceiling_reached",
            "stopped_at_depth", WARNING_FIELD, DEPTH_FIELD, "tests_past_depth_cut")
    trimmed = {key: answer[key] for key in keep if key in answer}
    trimmed["tests"] = [{"path": test["path"], "name": test["name"], "reason": test["reason"]}
                        for test in answer.get("likely_tests", [])]
    trimmed["impacted"] = [{key: symbol[key] for key in ("name", "kind", "path", "line", "depth")}
                           for symbol in answer.get("impacted_symbols", [])]
    return trimmed


def run_case(binary, tree, case):
    checkout(tree, case["commit"])
    mutant = case["mutant"]
    record = {"repo": case["repo"], "language": case["language"], "commit": case["commit"], "split": case["split"],
              "path": mutant["path"], "line": mutant["line"]}
    blast(binary, tree, "--file", mutant["path"], "--depth", "1")
    symbol = enclosing_symbol(tree, mutant["path"], mutant["line"])
    if symbol:
        record["symbol"] = {"id": symbol[0], "name": symbol[1], "kind": symbol[2]}
        record["symbol_answer"] = trim_answer(blast(binary, tree, "--symbol-id", symbol[0]))
    apply_mutant(tree, mutant)
    try:
        record["diff_answer"] = trim_answer(blast(binary, tree))
    finally:
        git(tree, "checkout", "-q", "--", mutant["path"])
    return record


def load_truth(repos=None):
    cases = []
    for file in sorted(TRUTH_DIR.glob("*.jsonl")):
        if repos and file.stem not in repos:
            continue
        cases += [json.loads(line) for line in file.read_text().splitlines() if line.strip()]
    return cases


def wanted(cases, split):
    return [case for case in cases if case.get("outcome") == "detected" and split in ("all", case["split"])]


def cmd_run(args):
    binary = Path(args.code_kb).resolve()
    out = Path(args.out).resolve()
    prepare_run(out, binary)
    results = out / "results.jsonl"
    done = ({(row["repo"], row["commit"]) for row in map(json.loads, results.read_text().splitlines())}
            if results.exists() else set())
    for case in wanted(load_truth(args.repo), args.split):
        if (case["repo"], case["commit"]) in done:
            continue
        tree = replay_tree(clone_dir(REPOS_DIR, corpus_entry(case["repo"])), out / "trees", case["repo"])
        started = time.monotonic()
        record = run_case(binary, tree, case)
        record["seconds"] = round(time.monotonic() - started, 1)
        with results.open("a") as sink:
            sink.write(json.dumps(record) + "\n")
        print(f"{case['repo']} {case['commit'][:10]} {record.get('symbol', {}).get('name', '-')}", flush=True)


def judge(answer, failing):
    """How one answer fares against the failing tests: what it missed, and whether it warned."""
    listed, files = set(), set()
    for test in answer.get("tests", []):
        if test["reason"].startswith(COLLAPSED_FILE):
            files.add(test["path"])
        listed.add(test_key(test["path"], test["name"]))
    truth = {test_key(test["path"], test["name"]) for test in failing}
    missed = {key for key in truth if key not in listed and key[0] not in files}
    listed_files = {path for path, _ in listed} | files
    count = answer.get("likely_tests_found") or 0
    cut = bool(answer.get("likely_tests_truncated") or answer.get("traversal_ceiling_reached"))
    warned = bool(answer.get(WARNING_FIELD)) or (count > 0 and (answer.get(DEPTH_FIELD) or 0) > 0)
    return {
        "tests": len(truth),
        "tests_missed": len(missed),
        "files": len({path for path, _ in truth}),
        "files_missed": len({path for path, _ in truth if path not in listed_files}),
        "listed": count,
        "empty": count == 0 and not answer.get("error"),
        "cut": cut,
        "warned": warned,
        "silent_miss": count > 0 and not cut and not warned and bool(missed),
        "needless_warning": warned and not missed,
        "error": bool(answer.get("error")),
    }


def score(records, truth):
    failing = {(case["repo"], case["commit"]): case for case in truth if case["outcome"] == "detected"}
    tallies = {}
    for record in records:
        case = failing.get((record["repo"], record["commit"]))
        if case is None:
            continue
        for mode in MODES:
            answer = record.get(f"{mode}_answer")
            if answer is None:
                continue
            judged = judge(answer, case["failing"])
            judged["share"] = judged["listed"] / case["suite_tests"] if case.get("suite_tests") else 0.0
            tallies.setdefault((record["repo"], mode), []).append(judged)
    return {key: summarize(rows) for key, rows in sorted(tallies.items())}


def summarize(rows):
    total = lambda field: sum(row[field] for row in rows)
    listing = [row for row in rows if row["listed"] and not row["cut"]]
    return {
        "bugs": len(rows),
        "files": total("files"),
        "files_missed": total("files_missed"),
        "tests": total("tests"),
        "tests_missed": total("tests_missed"),
        "bugs_fully_missed": sum(1 for row in rows if row["files_missed"] == row["files"]),
        "empty": total("empty"),
        "cut": total("cut"),
        "errors": total("error"),
        "median_share": statistics.median(row["share"] for row in rows) if rows else 0.0,
        "listing": len(listing),
        "listing_with_miss": sum(1 for row in listing if row["tests_missed"]),
        "warned": total("warned"),
        "silent_misses": total("silent_miss"),
        "needless_warnings": total("needless_warning"),
    }


def pct(part, whole):
    return f"{part}/{whole} ({part / whole:.0%})" if whole else f"{part}/0"


def render(summary, split, version):
    lines = [f"# Planted-bug replay ({split} half, {version})", ""]
    lines.append("| Repo | Seed | Bugs | Failing files missed | Failing tests missed | Bugs fully missed | Empty | Cut |"
                 " Median listed | Lists tests | Silent misses | Warned | Needless warnings |")
    lines.append("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for (repo, mode), row in summary.items():
        lines.append(
            f"| {repo} | {mode} | {row['bugs']} | {pct(row['files_missed'], row['files'])} | "
            f"{pct(row['tests_missed'], row['tests'])} | {row['bugs_fully_missed']} | {row['empty']} | {row['cut']} | "
            f"{row['median_share']:.0%} | {row['listing']} | {pct(row['silent_misses'], row['listing'])} | "
            f"{row['warned']} | {pct(row['needless_warnings'], row['warned'])} |")
    lines += ["", "A silent miss lists tests, is not cut, misses a failing test, and has no warning. A needless",
              "warning misses no failing test. Empty answers say to run the full suite; cut answers say how many",
              "rows they show."]
    return "\n".join(lines)


def cmd_score(args):
    run = Path(args.run)
    records = [json.loads(line) for line in (run / "results.jsonl").read_text().splitlines() if line.strip()]
    records = [record for record in records if args.split in ("all", record["split"])]
    summary = score(records, load_truth())
    if args.json:
        print(json.dumps({f"{repo}/{mode}": row for (repo, mode), row in summary.items()}, indent=2))
        return
    version = load_json(run / "run.json").get("version", "unknown") if (run / "run.json").exists() else "unknown"
    print(render(summary, args.split, version))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    truth = commands.add_parser("truth", help="plant bugs and record the tests that fail")
    truth.add_argument("--repo", required=True, choices=sorted(RUNNERS))
    truth.add_argument("--commits", type=int, default=50)
    truth.add_argument("--tries", type=int, default=4)
    truth.add_argument("--start", help="walk back from this commit instead of the corpus commit")
    truth.set_defaults(func=cmd_truth)
    recheck = commands.add_parser("recheck", help="keep the failing tests that also fail when their file runs alone")
    recheck.add_argument("--repo", required=True, choices=sorted(RUNNERS))
    recheck.set_defaults(func=cmd_recheck)
    run = commands.add_parser("run", help="ask blast-radius for the tests of each bug")
    run.add_argument("--code-kb", required=True)
    run.add_argument("--out", required=True)
    run.add_argument("--repo", action="append")
    run.add_argument("--split", choices=("dev", "holdout", "all"), default="dev")
    run.set_defaults(func=cmd_run)
    scored = commands.add_parser("score", help="count the failing tests the answers left out")
    scored.add_argument("--run", required=True)
    scored.add_argument("--split", choices=("dev", "holdout", "all"), default="dev")
    scored.add_argument("--json", action="store_true")
    scored.set_defaults(func=cmd_score)
    args = parser.parse_args(argv)
    args.func(args)


if __name__ == "__main__":
    sys.exit(main())
