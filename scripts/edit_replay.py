#!/usr/bin/env python3
"""Offline replay of `code-kb check` and the Stop hook on real commits.

The clean set holds commits that change the shape of a symbol (a removal, a rename, parameters,
fields, variants). The author's commit is the truth, so a Stop hook block on such a commit is
false unless a hand check finds a site that still uses the old shape.

Usage:
  scripts/edit_replay.py select clean [--repos evals/reach/corpus.json] [--per-repo 15]
  scripts/edit_replay.py run clean --code-kb PATH --out DIR [--split dev|holdout|all]
  scripts/edit_replay.py score clean --run DIR [--split dev|holdout] [--todo FILE]
  scripts/edit_replay.py verdicts clean --from FILE

`run` and `score` take the dev half unless `--split` names another; tune on dev only.
"""

import argparse
import ast
import collections
import datetime
import hashlib
import itertools
import json
import math
import os
import posixpath
import re
import subprocess
import sys
import time
from pathlib import Path

import reach_gate

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "evals/reach/corpus.json"
CASES_DIR = REPO_ROOT / "evals/replay"
WORK_DIR = Path.home() / ".cache/code-kb-accuracy/phase3"
REPOS_DIR = WORK_DIR / "repos"
CHECK_TIMEOUT_SECONDS = 600
FALSE_BLOCK_MARK = 0.02
MIN_SHAPE_COMMITS = 50
MIN_LANGUAGES = 5
SITE_LINE = re.compile(r"^(?P<path>.+?):(?P<line>\d+)  (?P<display>.+?): (?P<change>.+)$")
TODO_SAMPLES = 5
CATCH_MARK = 0.80
OUTSIDE_PRECISION_MARK = 0.90
MIN_TRUTH_SITES = 10
OUTSIDE_JUDGED_CAP = 50
MISS_CAUSES = ("shape not seen", "site not indexed", "import not listed", "site labeled unresolved", "site excluded",
               "not a call", "fits the new shape", "cut by a limit", "other")
CALL_KINDS = {"calls", "call", "instantiates"}
CHECKED_KINDS = {"function", "method", "constructor", "class", "struct", "interface", "trait", "enum", "record",
                 "constant", "variable", "type"}

LANGUAGE_OF_EXTENSION = {
    ".py": "python",
    ".js": "javascript", ".mjs": "javascript", ".cjs": "javascript", ".jsx": "javascript",
    ".ts": "typescript", ".tsx": "typescript", ".mts": "typescript", ".cts": "typescript",
    ".java": "java", ".go": "go", ".cs": "csharp", ".rs": "rust",
}
FAMILY = {"javascript": "script", "typescript": "script"}
MODIFIERS = (r"(?:(?:public|private|protected|internal|static|final|abstract|synchronized|native|virtual|"
             r"override|sealed|async|new|extern|unsafe|partial|default|strictfp|readonly)\s+)*")
RUST_PREFIX = r'^\s*(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?'
DEFINITIONS = {
    "python": [
        ("callable", re.compile(r"^\s*(?:async\s+)?def\s+(?P<name>\w+)\s*(?P<rest>\(.*)$")),
        ("type", re.compile(r"^\s*class\s+(?P<name>\w+)\b")),
    ],
    "script": [
        ("callable", re.compile(r"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:async\s+)?function\s*\*?\s*"
                                r"(?P<name>\w+)\s*(?:<[^()]*>)?\s*(?:(?P<rest>\(.*)|<[^()>]*)$")),
        ("callable", re.compile(r"=\s*(?:async\s+)?function\s*\*?\s*(?P<name>\w+)\s*(?P<rest>\(.*)$")),
        ("type", re.compile(r"^\s*(?:export\s+)?(?:default\s+)?(?:declare\s+)?(?:abstract\s+)?"
                            r"(?:class|interface|enum|const\s+enum)\s+(?P<name>\w+)")),
        ("type", re.compile(r"^\s*(?:export\s+)?(?:declare\s+)?type\s+(?P<name>\w+)\s*(?:<[^=]*>)?\s*=")),
    ],
    "java": [
        ("type", re.compile(r"^\s*" + MODIFIERS + r"(?:class|interface|enum|record|struct)\s+(?P<name>\w+)")),
        ("callable", re.compile(r"^\s*" + MODIFIERS + r"(?:<[^()=;]+>\s+)?(?P<type>[\w.?\[\]]+(?:<[^()=;]*>)?(?:\[\])*)\s+"
                                r"(?P<name>\w+)\s*(?:<[^()=;]*>)?\s*(?P<rest>\(.*)$")),
    ],
    "go": [
        ("callable", re.compile(r"^func\s+(?:\([^)]*\)\s*)?(?P<name>\w+)\s*(?:\[[^\]]*\])?\s*(?P<rest>\(.*)$")),
        ("type", re.compile(r"^type\s+(?P<name>\w+)\b")),
    ],
    "rust": [
        ("callable", re.compile(RUST_PREFIX + r"fn\s+(?P<name>\w+)\s*(?:<[^()]*>)?\s*(?:(?P<rest>\(.*)|<[^()>]*)$")),
        ("type", re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:struct|enum|trait|type|union)\s+(?P<name>\w+)")),
    ],
}
DEFINITIONS["csharp"] = DEFINITIONS["java"]
NOT_A_METHOD = {"return", "new", "throw", "else", "case", "await", "yield", "if", "for", "while", "switch", "catch",
                "using", "lock", "foreach", "sizeof", "typeof", "nameof", "goto", "do", "try", "in", "is", "as",
                "public", "private", "protected", "internal", "static", "final", "abstract", "synchronized", "native",
                "virtual", "override", "sealed", "async", "extern", "unsafe", "partial", "default", "strictfp",
                "readonly"}
STRING_LITERAL = re.compile(r"""(["'`])(?:\\.|(?!\1)[^\\])*\1""")
IMPORT_LINE = re.compile(r"^\s*(?:import\b|from\s+\S+\s+import\b|export\s+.*\bfrom\b)|\brequire\s*\(")
SCRIPT_IMPORTS = (
    re.compile(r"""\b(?:import|export)\s+(?:type\s+)?(?P<clause>\{[^}]*\}|\*(?:\s+as\s+\w+)?|\w+(?:\s*,\s*(?:\{[^}]*\}|"""
               r"""\*\s+as\s+\w+))?)\s+from\s+["'](?P<source>[^"'\n]+)["']"""),
    re.compile(r"""(?:(?:\b(?:const|let|var)\s+|,\s*)(?P<clause>\{[^}]*\}|\w+)\s*=\s*)?\brequire\s*\(\s*["'](?P<source>[^"'\n]+)["']\s*\)"""),
)
SCRIPT_EXTENSION = re.compile(r"\.(?:[cm]?[jt]sx?)$")
RECEIVER = re.compile(r"(?:(?P<receiver>\w+)|[)\]]|^)\s*\??\.\s*$")
KEY_BEFORE = re.compile(r"(?:^|[{,(])\s*$")
KEY_AFTER = re.compile(r"\s*\??:(?!:)")
COMMENT_START = ("#", "//", "/*", "*", '"""', "'''")
IMPORT_START = re.compile(r"^\s*(?:(?:pub(?:\([^)]*\))?\s+)?use\s|import\s|from\s+\S+\s+import\b|export\s.*\bfrom\b|"
                          r"export\s*\{|using\s)")
LONG_LINE = 2000


def split_of(sha):
    return "dev" if int(sha[:8], 16) % 2 == 0 else "holdout"


def git(cwd, *args):
    done = subprocess.run(["git", "-C", str(cwd), *args], capture_output=True, text=True, errors="replace", check=False)
    if done.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed in {cwd}: {done.stderr.strip()}")
    return done.stdout.strip()


def load_json(path):
    return json.loads(Path(path).read_text())


def write_json(path, data):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(data, indent=2) + "\n")


def clone_dir(repos_dir, entry):
    clone = Path(repos_dir) / entry["name"]
    if not (clone / ".git").exists():
        clone.parent.mkdir(parents=True, exist_ok=True)
        git(clone.parent, "clone", "--no-checkout", entry["url"], clone.name)
    return clone


def replay_tree(clone, trees_dir, name):
    """A detached worktree of the full clone; code-kb copies and reconciles the clone's index."""
    tree = Path(trees_dir) / name
    if not (tree / ".git").exists():
        tree.parent.mkdir(parents=True, exist_ok=True)
        git(clone, "worktree", "prune")
        git(clone, "worktree", "add", "--detach", "--force", str(tree), "HEAD")
    return tree


def checkout(tree, commit):
    git(tree, "checkout", "-q", "--detach", "--force", commit)
    git(tree, "clean", "-q", "-fd", "-e", ".code-kb")


def binary_version(binary):
    done = subprocess.run([str(binary), "--version"], capture_output=True, text=True, timeout=30, check=False)
    return done.stdout.strip() or "unknown"


def trim_site(site):
    return {key: site[key] for key in ("path", "line", "caller") if key in site}


def trim_change(change):
    return {
        "kind": change["kind"],
        "display": change["display"],
        "path": change["path"],
        "line": change["line"],
        "change": change["change"],
        "in_diff": len(change.get("in_diff", [])),
        "proven": [trim_site(site) for site in change.get("proven", [])],
        "likely": [trim_site(site) for site in change.get("likely", [])],
        "unresolved": change.get("unresolved", 0),
    }


def run_check(binary, tree, base):
    started = time.perf_counter()
    try:
        done = subprocess.run([str(binary), "--root", str(tree), "check", "--base", base, "--json"],
                              capture_output=True, text=True, timeout=CHECK_TIMEOUT_SECONDS, check=False)
    except subprocess.TimeoutExpired:
        return {"status": "error", "error": f"timeout after {CHECK_TIMEOUT_SECONDS} s",
                "seconds": round(time.perf_counter() - started, 2)}
    record = {"rc": done.returncode, "seconds": round(time.perf_counter() - started, 2)}
    try:
        report = json.loads(done.stdout)
    except json.JSONDecodeError:
        return record | {"status": "error", "error": (done.stderr or done.stdout)[-400:]}
    if done.returncode != 0:
        return record | {"status": "error", "error": (done.stderr or done.stdout)[-400:]}
    return record | {
        "status": "ok",
        "files_changed": report.get("files_changed"),
        "cuts": report.get("cuts", []),
        "changes": [trim_change(change) for change in report.get("changes", [])],
    }


def parse_hook_output(stdout):
    """Returns the hook decision: no output is no block; a block lists its sites by path and line."""
    text = stdout.strip()
    if not text:
        return {"block": False, "count": 0, "rows": []}
    decision = json.loads(text)
    reason = decision.get("reason", "")
    first = reason.splitlines()[0] if reason else ""
    count = re.search(r"Before you finish: (\d+) sites", first)
    rows = []
    for line in reason.splitlines()[1:]:
        match = SITE_LINE.match(line)
        if match:
            rows.append({"path": match["path"], "line": int(match["line"]),
                         "display": match["display"], "change": match["change"]})
    return {"block": decision.get("decision") == "block", "count": int(count[1]) if count else len(rows),
            "rows": rows}


def run_stop_hook(binary, tree, base, session_id):
    """Runs `code-kb hook Stop` with a fresh session whose base is `base`. The hook prints nothing on
    any error, so `ran` is true only when it wrote `checked_diff` to the session file."""
    sessions = Path(tree) / ".code-kb/sessions"
    sessions.mkdir(parents=True, exist_ok=True)
    session_file = sessions / f"{session_id}.json"
    session_file.write_text(json.dumps({"base": base}))
    payload = json.dumps({"session_id": session_id, "cwd": str(tree), "hook_event_name": "Stop"})
    try:
        done = subprocess.run([str(binary), "hook", "Stop"], input=payload, cwd=tree, capture_output=True,
                              text=True, timeout=CHECK_TIMEOUT_SECONDS, check=False)
        state = json.loads(session_file.read_text()) if session_file.exists() else {}
        return parse_hook_output(done.stdout) | {"ran": "checked_diff" in state}
    finally:
        session_file.unlink(missing_ok=True)
        (sessions / f"{session_id}.lock").unlink(missing_ok=True)


def listed_rows(change):
    return [(site, "proven") for site in change["proven"]] + [(site, "likely") for site in change["likely"]]


def listed_block_rows(record):
    """The rows `check_edit` lists outside the diff: every proven or likely site of every change."""
    return [
        {"path": site["path"], "line": site["line"], "display": change["display"], "change": change["change"],
         "label": label}
        for change in record.get("changes", [])
        for site, label in listed_rows(change)
    ]


def hook_can_block(record):
    return any(change["proven"] or change["likely"] for change in record.get("changes", []))


def walk_commits(clone, start, max_walk):
    for line in git(clone, "rev-list", "--no-merges", "--parents", f"--max-count={max_walk}", start).splitlines():
        shas = line.split()
        if len(shas) == 2:
            yield shas[0], shas[1]


def cmd_select_clean(args):
    corpus = load_json(args.repos)
    binary = Path(args.code_kb).resolve()
    trees = Path(args.work) / "trees"
    cases = []
    for entry in corpus:
        clone = clone_dir(args.repos_dir, entry)
        tree = replay_tree(clone, trees, entry["name"])
        kept = walked = 0
        for commit, parent in walk_commits(clone, entry["commit"], args.max_walk):
            walked += 1
            checkout(tree, commit)
            record = run_check(binary, tree, parent)
            if record["status"] == "ok" and record["changes"]:
                cases.append({"repo": entry["name"], "language": entry["language"], "commit": commit,
                              "parent": parent, "split": split_of(commit), "verdicts": []})
                kept += 1
                if kept >= args.per_repo:
                    break
        print(f"{entry['name']}: {kept} commits with a shape change in {walked} walked", file=sys.stderr)
    languages = {case["language"] for case in cases}
    write_json(args.out, {
        "selected_with": binary_version(binary),
        "selected": datetime.date.today().isoformat(),
        "per_repo": args.per_repo,
        "max_walk": args.max_walk,
        "cases": cases,
    })
    print(f"{args.out}: {len(cases)} commits over {len(languages)} languages")
    if len(cases) < MIN_SHAPE_COMMITS or len(languages) < MIN_LANGUAGES:
        print(f"warning: the clean mark needs at least {MIN_SHAPE_COMMITS} commits over "
              f"{MIN_LANGUAGES} languages", file=sys.stderr)
        return 1
    return 0


def wanted_cases(cases, split, repos=None):
    names = set(repos.split(",")) if repos else None
    return [case for case in cases
            if (split == "all" or case["split"] == split) and (names is None or case["repo"] in names)]


def done_commits(path):
    if not path.exists():
        return set()
    return {json.loads(line)["commit"] for line in path.read_text().splitlines() if line.strip()}


def cmd_run_clean(args):
    data = load_json(args.cases)
    corpus = {entry["name"]: entry for entry in load_json(args.repos)}
    binary = Path(args.code_kb).resolve()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    meta = out / "run.json"
    if not meta.exists():
        write_json(meta, {"binary": str(binary), "version": binary_version(binary),
                          "started": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")})
    errors = 0
    for case in wanted_cases(data["cases"], args.split, args.only):
        sink_path = out / "clean" / f"{case['repo']}.jsonl"
        sink_path.parent.mkdir(parents=True, exist_ok=True)
        if case["commit"] in done_commits(sink_path):
            continue
        tree = replay_tree(clone_dir(args.repos_dir, corpus[case["repo"]]), out / "trees", case["repo"])
        checkout(tree, case["commit"])
        record = {key: case[key] for key in ("repo", "language", "commit", "parent", "split")}
        record |= run_check(binary, tree, case["parent"])
        record["hook"] = None
        if record["status"] == "ok" and hook_can_block(record):
            record["hook"] = run_stop_hook(binary, tree, case["parent"], f"replay-{case['commit'][:12]}")
            if not record["hook"]["ran"]:
                record |= {"status": "error", "error": "the Stop hook did not run its check"}
        errors += record["status"] != "ok"
        with sink_path.open("a") as sink:
            sink.write(json.dumps(record) + "\n")
        hook = record["hook"] or {}
        print(f"{case['repo']} {case['commit'][:10]} {record['status']} changes {len(record.get('changes', []))} "
              f"hook-block {hook.get('block', False)} {record['seconds']}s", file=sys.stderr)
    return 1 if errors else 0


def load_run(run_dir, kind):
    records = []
    for path in sorted((Path(run_dir) / kind).glob("*.jsonl")):
        records.extend(json.loads(line) for line in path.read_text().splitlines() if line.strip())
    return records


def row_key(row):
    return (row["path"], int(row["line"]), row["display"])


def verdict_key(verdict):
    """A row verdict names one site; a group verdict has no path and covers every row of one change."""
    if verdict.get("path") is None:
        return ("group", verdict["display"], verdict["change"])
    return row_key(verdict)


def verdict_map(case):
    return {verdict_key(verdict): verdict["verdict"] for verdict in case.get("verdicts", [])}


def row_verdict(row, verdicts):
    return verdicts.get(row_key(row)) or verdicts.get(("group", row["display"], row["change"]))


def judge_block(rows, verdicts):
    """A block is false when a hand check finds every one of its rows false; any unjudged row leaves
    it unjudged unless another row is true."""
    marks = [row_verdict(row, verdicts) for row in rows]
    if "true" in marks:
        return "true"
    if None in marks:
        return "unjudged"
    return "false"


def percentile(values, share):
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, math.ceil(share * len(ordered)) - 1)]


def score_clean(records, cases):
    """Counts per language: commits with a shape change, hook and listed blocks, and false blocks."""
    by_commit = {case["commit"]: case for case in cases}
    tallies = {}
    todo = []
    seconds = []
    for record in records:
        case = by_commit.get(record["commit"])
        if case is None:
            continue
        tally = tallies.setdefault(record["language"], {
            "commits": 0, "errors": 0, "shape": 0, "hook_blocks": 0, "hook_false": 0, "hook_unjudged": 0,
            "listed_blocks": 0, "listed_false": 0, "listed_unjudged": 0, "false_rows": 0})
        tally["commits"] += 1
        if record["status"] != "ok":
            tally["errors"] += 1
            continue
        seconds.append(record["seconds"])
        if not record["changes"]:
            continue
        tally["shape"] += 1
        verdicts = verdict_map(case)
        blocks = {"listed": listed_block_rows(record)}
        hook = record.get("hook") or {}
        blocks["hook"] = hook.get("rows", []) if hook.get("block") else []
        for kind, rows in blocks.items():
            if not rows:
                continue
            tally[f"{kind}_blocks"] += 1
            outcome = judge_block(rows, verdicts)
            if outcome != "true":
                tally[f"{kind}_{outcome}"] += 1
        tally["false_rows"] += sum(row_verdict(row, verdicts) == "false" for row in blocks["listed"])
        hook_groups = {(row["display"], row["change"]) for row in blocks["hook"]}
        groups = {}
        for row in blocks["listed"]:
            if row_verdict(row, verdicts) is None:
                group = groups.setdefault((row["display"], row["change"]), {
                    key: record[key] for key in ("repo", "commit", "parent", "split")} | {
                    "display": row["display"], "change": row["change"],
                    "hook": (row["display"], row["change"]) in hook_groups, "rows": 0, "samples": []})
                group["rows"] += 1
                if len(group["samples"]) < TODO_SAMPLES:
                    group["samples"].append({key: row[key] for key in ("path", "line", "label")})
        todo.extend(groups.values())
    return tallies, seconds, todo


def share(numerator, denominator):
    return numerator / denominator if denominator else 0.0


def render_clean(tallies, seconds, split, version):
    total = {key: sum(t[key] for t in tallies.values()) for key in next(iter(tallies.values()), {})}
    lines = [f"Clean set, {split} half, {version}", "",
             "| Language | Commits | Errors | Shape | Hook blocks | Hook false | Hook false share | "
             "Listed blocks | Listed false | Listed false share | Unjudged |",
             "|---|---|---|---|---|---|---|---|---|---|---|"]
    for language, tally in sorted(tallies.items()) + ([("total", total)] if total else []):
        unjudged = tally["hook_unjudged"] + tally["listed_unjudged"]
        lines.append(
            f"| {language} | {tally['commits']} | {tally['errors']} | {tally['shape']} | {tally['hook_blocks']} | "
            f"{tally['hook_false']} | {share(tally['hook_false'], tally['shape']):.1%} | {tally['listed_blocks']} | "
            f"{tally['listed_false']} | {share(tally['listed_false'], tally['shape']):.1%} | {unjudged} |")
    lines += ["", f"Check time: p50 {percentile(seconds, 0.5):.2f} s, p95 {percentile(seconds, 0.95):.2f} s", ""]
    if not total:
        return "\n".join(lines + ["Result: NO DATA"]) + "\n"
    if total["errors"] or total["hook_unjudged"]:
        result = "INCOMPLETE (errors or unjudged hook blocks)"
    elif share(total["hook_false"], total["shape"]) <= FALSE_BLOCK_MARK:
        result = "PASS"
    else:
        result = "FAIL"
    lines.append(f"Result: {result} (mark: hook false-block share <= {FALSE_BLOCK_MARK:.0%})")
    return "\n".join(lines) + "\n"


def cmd_score_clean(args):
    cases = wanted_cases(load_json(args.cases)["cases"], args.split)
    records = [r for r in load_run(args.run, "clean") if r["split"] == args.split]
    meta = Path(args.run) / "run.json"
    version = load_json(meta)["version"] if meta.exists() else "unknown binary"
    tallies, seconds, todo = score_clean(records, cases)
    print(render_clean(tallies, seconds, args.split, version), end="")
    if args.todo:
        write_json(args.todo, todo)
        print(f"{len(todo)} unjudged groups ({sum(g['rows'] for g in todo)} rows) written to {args.todo}")
    return 0


def cmd_verdicts_clean(args):
    data = load_json(args.cases)
    by_commit = {case["commit"]: case for case in data["cases"]}
    added = 0
    for verdict in load_json(getattr(args, "from")):
        if verdict.get("verdict") not in ("true", "false"):
            raise ValueError(f"verdict must be true or false: {verdict}")
        case = by_commit[verdict["commit"]]
        key = verdict_key(verdict)
        kept = [v for v in case["verdicts"] if verdict_key(v) != key]
        kept.append({field: verdict[field] for field in ("path", "line", "display", "change", "label", "verdict",
                                                          "evidence") if verdict.get(field) is not None})
        case["verdicts"] = sorted(kept, key=lambda v: [str(part) for part in verdict_key(v)])
        added += 1
    write_json(args.cases, data)
    print(f"{added} verdicts merged into {args.cases}")
    return 0


def language_of(path):
    return LANGUAGE_OF_EXTENSION.get(Path(path).suffix)


def family_of(language):
    return FAMILY.get(language, language)


def parse_diff(text):
    """Splits `git diff -U0` output into files with removed and added lines, by line number."""
    files = []
    current = None
    old_line = new_line = 0
    for line in text.splitlines():
        if line.startswith("diff --git "):
            current = {"old": None, "new": None, "removed": [], "added": []}
            files.append(current)
        elif current is None:
            continue
        elif line.startswith("--- "):
            current["old"] = None if line[4:] == "/dev/null" else line[6:]
        elif line.startswith("+++ "):
            current["new"] = None if line[4:] == "/dev/null" else line[6:]
        elif line.startswith("@@ "):
            match = re.match(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@", line)
            old_line, new_line = int(match[1]), int(match[2])
        elif line.startswith("-"):
            current["removed"].append((old_line, line[1:]))
            old_line += 1
        elif line.startswith("+"):
            current["added"].append((new_line, line[1:]))
            new_line += 1
    return files


def balanced(text, start):
    """The text from the `(` at `start` to its matching `)`, or to the end of the line."""
    depth = 0
    for index in range(start, len(text)):
        depth += {"(": 1, ")": -1}.get(text[index], 0)
        if depth == 0:
            return text[start:index + 1]
    return text[start:]


def top_level_parts(text):
    parts, depth, start = [], 0, 0
    for index, char in enumerate(text):
        if char in "([{<":
            depth += 1
        elif char in ")]}>":
            depth -= 1
        elif char == "," and depth == 0:
            parts.append(text[start:index])
            start = index + 1
    return parts + [text[start:]]


def blank_name(family, part, grouped):
    if family == "rust":
        return re.sub(r"^[^:]*?(?=:(?!:))", "_", part, count=1)
    if family == "go":
        return re.sub(r"^\s*\w+", "_", part, count=1) if grouped else part
    head = re.split(r"[?:=]", part, maxsplit=1)[0] if family == "script" else part
    name = re.search(r"\w+(?=\W*$)", head)
    if not name or not re.match(r"\W*$", head[name.end():]):
        return part
    return part[:name.start()] + "_" + part[name.end():]


def parameter_shape(family, params):
    """The parameter text with spaces removed. Where a caller cannot pass an argument by its name (all
    but Python and C#), each top-level parameter name is blanked, so a rename alone is no change."""
    if family in ("python", "csharp") or not params.startswith("("):
        return re.sub(r"\s+", "", params)
    inner = params[1:-1] if params.endswith(")") else params[1:]
    parts = top_level_parts(inner.replace("=>", "\0"))
    grouped = any(re.match(r"\s*\w+\s+\S", part) for part in parts)
    blanked = ",".join(blank_name(family, part, grouped) for part in parts)
    return re.sub(r"\s+", "", "(" + blanked + ")").replace("\0", "=>")


def definitions(language, text):
    """The definitions one source line holds, as (kind, name, parameter text with no spaces). A
    callable head whose type parameters go on to the next line has no parameter text (None)."""
    found = []
    for kind, pattern in DEFINITIONS.get(family_of(language), []):
        match = pattern.search(text)
        if not match or match["name"] in NOT_A_METHOD:
            continue
        if "type" in pattern.groupindex and match["type"] in NOT_A_METHOD:
            continue
        rest = match.groupdict().get("rest")
        params = parameter_shape(family_of(language), balanced(rest, 0)) if rest else (None if kind == "callable" else "")
        found.append((kind, match["name"], params))
        break
    return found


def shape_changes(language, file_diff):
    """Definitions the diff removes or whose parameters it changes, read from the diff lines only.
    A type counts only when the diff removes it; a field change is not a definition line."""
    added = collections.defaultdict(set)
    for _, text in (row for row in file_diff["added"] if len(row[1]) <= LONG_LINE):
        for kind, name, params in definitions(language, text):
            added[(kind, name)].add(params)
    changes = {}
    for line, text in (row for row in file_diff["removed"] if len(row[1]) <= LONG_LINE):
        for kind, name, params in definitions(language, text):
            kept = added.get((kind, name))
            if kept is None:
                change = "removed or renamed"
            elif kind == "callable" and params not in kept and params is not None and None not in kept:
                change = "parameters changed"
            else:
                continue
            changes.setdefault(name, {"symbol": name, "kind": kind, "line": line, "change": change})
    return list(changes.values())


def use_snippets(text, name, reaches=None):
    """Each use of `name` on a line: the name with its argument text when a call follows, else the name.
    A key, label, or named argument (`{name: v}`, `f(name: v)`) is no use, and neither is a use that
    `reaches(text, start)` rejects."""
    snippets = []
    text = STRING_LITERAL.sub('""', text)
    for match in re.finditer(r"\b" + re.escape(name) + r"\b", text):
        if KEY_BEFORE.search(text[:match.start()]) and KEY_AFTER.match(text, match.end()):
            continue
        if reaches and not reaches(text, match.start()):
            continue
        after = match.end()
        while after < len(text) and text[after] in " \t":
            after += 1
        if after < len(text) and text[after] == "(":
            snippets.append(name + re.sub(r"\s+", "", balanced(text, after)))
        else:
            snippets.append(name)
    return snippets


def is_comment(text):
    return text.strip().startswith(COMMENT_START)


def is_code_line(text):
    """A line a person wrote: not a comment, and not a generated line longer than `LONG_LINE`."""
    return len(text) <= LONG_LINE and not is_comment(text)


def block_comment_lines(language, text):
    """Line numbers (from 1) that start inside a block comment, in Python inside a triple-quoted
    string, or in Go inside a raw string (a template)."""
    family = family_of(language)
    inside = False
    lines = set()
    for number, line in enumerate((text or "").splitlines(), 1):
        if inside:
            lines.add(number)
        if family == "python":
            inside ^= (line.count('"""') + line.count("'''")) % 2 == 1
            continue
        if family == "go":
            code = line if not inside else (line[line.index("`") + 1:] if "`" in line else None)
            if code is not None:
                inside = STRING_LITERAL.sub('""', code).count("`") % 2 == 1
            continue
        code = line if inside else STRING_LITERAL.sub('""', line)
        position = 0
        while True:
            if inside:
                end = code.find("*/", position)
                if end < 0:
                    break
                inside, position = False, end + 2
            else:
                start, slashes = code.find("/*", position), code.find("//", position)
                if start < 0 or 0 <= slashes < start:
                    break
                inside, position = True, start + 2
    return lines


def truth_lines(file_diff, name, reaches=None):
    """Removed lines that name `name` and whose use of it the commit changed: a use whose text the
    file's added lines do not keep. A line that changed for another reason keeps its use, and a line
    that assigns the name binds a local, not a use. `reaches` filters the removed uses (see
    `script_use_filter`)."""
    binding = re.compile(r"^\s*(?:(?:const|let|var|val|auto|mut)\s+)*" + re.escape(name) + r"\s*(?::[^=]+)?=(?!=)")
    removed = [(line, text) for line, text in file_diff["removed"] if is_code_line(text) and not binding.match(text)]
    kept = collections.Counter(s for _, text in file_diff["added"] if len(text) <= LONG_LINE
                               for s in use_snippets(text, name))
    lost = collections.Counter(s for _, text in removed for s in use_snippets(text, name, reaches))
    changed = {s for s, count in lost.items() if count > kept[s]}
    return [line for line, text in removed if changed.intersection(use_snippets(text, name, reaches))]


def calls_or_reads_member(text, name):
    """True when the line calls `name` or reads it as a member: the uses a parameter change can break.
    An import or a type use cannot."""
    text = STRING_LITERAL.sub('""', text)
    return bool(re.search(r"\b" + re.escape(name) + r"\s*\(", text) or re.search(r"\.\s*" + re.escape(name) + r"\b", text))


def import_start(lines, number):
    """The first line (from 1) of the import or `use` statement that holds line `number`, or None:
    the lines above it that end in `,`, `{`, or `(` continue one statement."""
    start = number
    while start > 1 and number - start < 50 and re.search(r"[,{(\\]\s*$", lines[start - 2]):
        start -= 1
    return start if IMPORT_START.match(lines[start - 1]) else None


def script_imports(text):
    """(clause, source) of each import, re-export, and `require` in a JavaScript or TypeScript file."""
    return [(match["clause"] or "", match["source"]) for pattern in SCRIPT_IMPORTS for match in pattern.finditer(text)]


def source_reaches(source, use_path, defining_path, module):
    """True when an import source can name the defining file: a relative source that resolves to it
    or to a folder or `index` file above it (which can re-export it), or a package source that names
    its module."""
    if not source.startswith("."):
        return bool(re.search(r"\b" + re.escape(module) + r"\b", source))
    resolved = SCRIPT_EXTENSION.sub("", posixpath.normpath(posixpath.join(posixpath.dirname(use_path), source)))
    target = SCRIPT_EXTENSION.sub("", defining_path)
    folder = resolved.removesuffix("/index")
    return target == resolved or target.startswith(folder + "/")


def module_of(defining_path):
    stem = Path(defining_path).stem
    return Path(defining_path).parent.name if stem in ("index", "__init__", "mod") else stem


def script_use_filter(text, name, defining_path, use_path):
    """For a JavaScript or TypeScript function or type statement, which only an import reaches: a
    `reaches(line, start)` predicate that takes a bare use when an import that can reach the defining
    file names it, and `recv.name` when `recv` is bound by such an import of the whole module or by
    any import from a package (which can re-export it)."""
    module = module_of(defining_path)
    word = re.compile(r"\b" + re.escape(name) + r"\b")
    named = False
    receivers = set()
    for clause, source in script_imports(text or ""):
        reaches = source_reaches(source, use_path, defining_path, module)
        named = named or (reaches and bool(word.search(clause)))
        if not source.startswith(".") or (reaches and not clause.startswith("{")):
            receivers.update(alias or binding for binding, alias in re.findall(r"(\w+)(?:\s+as\s+(\w+))?", clause)
                             if binding not in ("as", "type"))

    def use_reaches(line, start):
        receiver = RECEIVER.search(line[:start])
        if receiver:
            return receiver["receiver"] in receivers
        return named
    return use_reaches


def binds_name_elsewhere(language, text, name, defining_path, use_path=""):
    """True when the file defines `name` itself, or (Python, JavaScript, TypeScript) imports it only
    from sources that do not name the defining module."""
    text = text or ""
    lines = [line for line in text.splitlines() if len(line) <= LONG_LINE and name in line]
    if any(found == name for line in lines for _, found, _ in definitions(language, line)):
        return True
    if family_of(language) not in ("python", "script"):
        return False
    module = module_of(defining_path)
    word = re.compile(r"\b" + re.escape(name) + r"\b")
    if family_of(language) == "python":
        imports = [line for line in lines if IMPORT_LINE.search(line) and word.search(line)]
        return bool(imports) and not any(re.search(r"\b" + re.escape(module) + r"\b", line) for line in imports)
    imports = script_imports(text)
    named = [source for clause, source in imports if word.search(clause)]
    return bool(named) and not any(source_reaches(source, use_path, defining_path, module) for source in named)


def holdback_cases(repo, commit, parent, diff_text, read_parent):
    """The holdback cases of one commit: a definition change in one file and the lines in other
    files of its language family whose use of the name the commit changed. A file that defines the
    name itself in the parent (`read_parent(path)`), or imports it from another module, gives no
    truth: its uses name another definition. A dunder name is skipped, since its callers name the class."""
    files = parse_diff(diff_text)
    cases = []
    for defining in files:
        path = defining["old"]
        language = language_of(path or "")
        if not path or language is None or (defining["new"] not in (None, path)):
            continue
        for change in shape_changes(language, defining):
            name = change["symbol"]
            if name.startswith("__") and name.endswith("__"):
                continue
            head = dict(defining["removed"]).get(change["line"], "")
            statement = family_of(language) == "script" and not re.search(r"=\s*(?:async\s+)?function\b", head)
            truth = []
            for other in files:
                other_path = other["old"]
                if not other_path or other_path == path or other["new"] not in (None, other_path):
                    continue
                other_language = language_of(other_path)
                if other_language is None or family_of(other_language) != family_of(language):
                    continue
                parent_text = read_parent(other_path)
                reaches = script_use_filter(parent_text, name, path, other_path) if statement else None
                comments = block_comment_lines(other_language, parent_text)
                lines = [line for line in truth_lines(other, name, reaches) if line not in comments]
                if change["change"] == "parameters changed":
                    removed = dict(other["removed"])
                    lines = [line for line in lines if calls_or_reads_member(removed[line], name)]
                if lines and not binds_name_elsewhere(other_language, parent_text, name, path, other_path):
                    truth.extend({"path": other_path, "line": line} for line in lines)
            if truth:
                cases.append({
                    "id": f"{commit[:12]}:{path}:{change['symbol']}",
                    "repo": repo, "language": language, "commit": commit, "parent": parent,
                    "split": split_of(commit), "file": path, "deleted": defining["new"] is None,
                    "symbol": change["symbol"], "kind": change["kind"], "line": change["line"],
                    "change": change["change"], "truth": truth, "verdicts": [],
                })
    return cases


def cmd_select_holdback(args):
    corpus = load_json(args.repos)
    cases = []
    for entry in corpus:
        clone = clone_dir(args.repos_dir, entry)
        per_name = collections.Counter()
        kept = walked = 0
        for commit, parent in walk_commits(clone, entry["commit"], args.max_walk):
            walked += 1
            diff = git(clone, "diff", "-U0", "--no-color", "--no-ext-diff", "-M", parent, commit, "--",
                       *[f"*{extension}" for extension in LANGUAGE_OF_EXTENSION])
            in_commit = 0
            def read_parent(path, parent=parent):
                done = subprocess.run(["git", "-C", str(clone), "show", f"{parent}:{path}"], capture_output=True,
                                      text=True, errors="replace", check=False)
                return done.stdout if done.returncode == 0 else None

            for case in holdback_cases(entry["name"], commit, parent, diff, read_parent):
                if per_name[case["symbol"]] >= args.per_symbol or in_commit >= args.per_commit:
                    continue
                per_name[case["symbol"]] += 1
                cases.append(case)
                kept += 1
                in_commit += 1
                if kept >= args.per_repo:
                    break
            if kept >= args.per_repo:
                break
        print(f"{entry['name']}: {kept} cases in {walked} commits", file=sys.stderr)
    by_language = collections.Counter(case["language"] for case in cases)
    write_json(args.out, {"selected": datetime.date.today().isoformat(), "per_repo": args.per_repo,
                          "max_walk": args.max_walk, "per_symbol": args.per_symbol, "per_commit": args.per_commit,
                          "cases": cases})
    print(f"{args.out}: {len(cases)} cases; " + ", ".join(f"{k} {v}" for k, v in sorted(by_language.items())))
    rich = [language for language, count in by_language.items() if count >= 20]
    if len(rich) < MIN_LANGUAGES:
        print(f"warning: the holdback set needs 20 cases in each of {MIN_LANGUAGES} languages", file=sys.stderr)
        return 1
    return 0


def symbol_changes(record_changes, case):
    return [change for change in record_changes
            if change["path"] == case["file"] and re.split(r"\.|::", change["display"])[-1] == case["symbol"]]


def miss_cause(answer, changes, path, line, overloads=1):
    """The cause of a truth site that a reported change did not list, from the parent tree's callers
    report. A call change lists only calls, a parameter change drops a call whose count the new
    range accepts, a parameter rename lists only calls that pass an old name, and a removal that
    leaves `overloads` lists only calls that no remaining overload takes."""
    covering = [site for site in answer["sites"] if reach_gate.site_covers(site, path, line)]
    if not covering:
        return "site not indexed", None
    labels = {site["label"] for site in covering}
    for label, cause in (("unresolved", "site labeled unresolved"), ("excluded", "site excluded")):
        if labels == {label}:
            site = next(s for s in covering if s["label"] == label)
            return cause, site.get("basis")
    texts = [change["change"] for change in changes]
    if any(text.startswith(("parameters", "signature changed", "parameter renamed")) for text in texts):
        if not any(site.get("kind") in CALL_KINDS for site in covering):
            return "not a call", "/".join(sorted({site.get("kind") or "?" for site in covering}))
        if all(text.startswith("parameter renamed") for text in texts):
            return "fits the new shape", "passes no old name"
        counts = {site["arg_count"] for site in covering if site.get("arg_count") is not None}
        ranges = [new_range(text) for text in texts]
        if counts and all(ranges) and any(low <= count <= high for low, high in ranges for count in counts):
            return "fits the new shape", "/".join(f"{count} args" for count in sorted(counts))
    elif overloads > 1 and any(text.startswith(("removed", "renamed")) for text in texts):
        return "fits the new shape", "an overload remains"
    return "other", "/".join(sorted(labels))


def new_range(text):
    """(low, high) of the accepted argument counts after `→` in a `parameters` change, else None."""
    match = re.fullmatch(r"parameters .*→\s*(\d+)(?:-(\d+)|( or more))?", text)
    if not match:
        return None
    low = int(match[1])
    return low, math.inf if match[3] else int(match[2] or low)


def parent_kinds(binary, tree, case):
    """The kinds of the definitions of the case name in the defining file of the parent tree, or None
    when the lookup fails. It leaves the parent's defining file in the tree."""
    git(tree, "checkout", case["parent"], "--", case["file"])
    try:
        rows = reach_gate.run_binary(binary, tree, "lookup", case["symbol"], "--path", case["file"], "--limit", "50",
                                     as_json=True)
    except (RuntimeError, ValueError, subprocess.TimeoutExpired):
        return None
    rows = rows.get("names", []) if isinstance(rows, dict) else rows
    return [row["kind"] for row in (r["symbol"] if isinstance(r.get("symbol"), dict) else r for r in rows)
            if row["path"] == case["file"] and row["name"] == case["symbol"] and row["kind"] != "import"]


def unchecked_cause(kinds, cuts):
    """The cause when the check reports no change of the name: `cut by a limit` when a cut stopped it
    and the parent tree defines the name with a kind the check compares, else `shape not seen`."""
    if not cuts:
        return "shape not seen"
    return "cut by a limit" if kinds is None or CHECKED_KINDS.intersection(kinds) else "shape not seen"


def apply_defining_file(tree, case):
    if case["deleted"]:
        (Path(tree) / case["file"]).unlink(missing_ok=True)
    else:
        git(tree, "checkout", case["commit"], "--", case["file"])


def run_holdback_case(binary, tree, case):
    checkout(tree, case["parent"])
    apply_defining_file(tree, case)
    record = {key: case[key] for key in ("id", "repo", "language", "commit", "parent", "split", "file", "symbol")}
    record |= run_check(binary, tree, case["parent"])
    if record["status"] != "ok":
        return record
    in_file = [change for change in record.pop("changes") if change["path"] == case["file"]]
    changes = symbol_changes(in_file, case)
    listed = {(site["path"], site["line"]): label for change in changes for site, label in listed_rows(change)}
    reached = {(site["path"], site["line"]): label for change in in_file for site, label in listed_rows(change)}
    truth = {(row["path"], row["line"]) for row in case["truth"]}
    record["changes"] = changes
    record["shape_seen"] = bool(changes)
    record["unresolved"] = sum(change["unresolved"] for change in changes)
    sources = {}

    def import_line(path, line):
        if path not in sources:
            file = Path(tree) / path
            sources[path] = file.read_text(errors="replace").splitlines() if file.is_file() else []
        lines = sources[path]
        return import_start(lines, line) if 0 < line <= len(lines) else None

    starts = {(p, n): import_line(p, n) for p, n in truth}
    covered = truth | {(p, starts[(p, n)]) for p, n in truth if starts[(p, n)]}
    record["outside"] = [{"path": p, "line": n, "label": listed[(p, n)]} for p, n in sorted(listed.keys() - covered)]
    caught = {site for site in truth if site in reached or (site[0], starts[site]) in reached}
    record["caught"] = [{"path": p, "line": n, "label": reached.get((p, n)) or reached[(p, starts[(p, n)])]}
                        for p, n in sorted(caught)]
    missed = sorted(truth - caught)
    record["missed"] = []
    if missed:
        kinds = parent_kinds(binary, tree, case)
    if missed and not changes:
        cause = unchecked_cause(kinds, record["cuts"])
        record["missed"] = [{"path": p, "line": n, "cause": cause} for p, n in missed]
    elif missed:
        status, detail, answer = reach_gate.fetch_answer(
            binary, tree, {"name": case["symbol"], "file": case["file"], "line": case["line"]})
        for path, line in missed:
            overloads = sum(kind in ("function", "method", "constructor") for kind in kinds or [])
            cause, basis = (miss_cause(answer, changes, path, line, overloads) if status == "ok"
                            else ("other", detail))
            if cause == "site not indexed" and starts[(path, line)]:
                cause = "import not listed"
            record["missed"].append({"path": path, "line": line, "cause": cause, "basis": basis})
    return record


def cmd_run_holdback(args):
    data = load_json(args.cases)
    corpus = {entry["name"]: entry for entry in load_json(args.repos)}
    binary = Path(args.code_kb).resolve()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if not (out / "run.json").exists():
        write_json(out / "run.json", {"binary": str(binary), "version": binary_version(binary),
                                      "started": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")})
    errors = 0
    for case in wanted_cases(data["cases"], args.split, args.only):
        sink_path = out / "holdback" / f"{case['repo']}.jsonl"
        sink_path.parent.mkdir(parents=True, exist_ok=True)
        done = set()
        if sink_path.exists():
            done = {json.loads(line)["id"] for line in sink_path.read_text().splitlines() if line.strip()}
        if case["id"] in done:
            continue
        tree = replay_tree(clone_dir(args.repos_dir, corpus[case["repo"]]), out / "trees", case["repo"])
        record = run_holdback_case(binary, tree, case)
        errors += record["status"] != "ok"
        with sink_path.open("a") as sink:
            sink.write(json.dumps(record) + "\n")
        print(f"{case['repo']} {case['id']} {record['status']} caught {len(record.get('caught', []))} "
              f"missed {len(record.get('missed', []))} {record['seconds']}s", file=sys.stderr)
    return 1 if errors else 0


def score_holdback(records, cases):
    """Counts per language: truth sites, the ones listed proven or likely, misses by cause, and the
    listed rows outside the truth with their hand-check verdicts."""
    by_id = {case["id"]: case for case in cases}
    tallies = {}
    causes = collections.Counter()
    todo = []
    for record in records:
        case = by_id.get(record["id"])
        if case is None:
            continue
        tally = tallies.setdefault(record["language"], {
            "cases": 0, "errors": 0, "truth": 0, "listed": 0, "neither": 0, "unresolved": 0, "no_shape": 0,
            "outside": 0, "outside_true": 0, "outside_false": 0})
        tally["cases"] += 1
        if record["status"] != "ok":
            tally["errors"] += 1
            continue
        tally["truth"] += len(case["truth"])
        tally["listed"] += len(record["caught"])
        tally["neither"] += len(record["missed"])
        tally["unresolved"] += record["unresolved"]
        tally["no_shape"] += not record["shape_seen"]
        tally["outside"] += len(record["outside"])
        causes.update(miss["cause"] for miss in record["missed"])
        verdicts = {(v["path"], int(v["line"])): v["verdict"] for v in case.get("verdicts", [])}
        for row in record["outside"]:
            mark = verdicts.get((row["path"], row["line"]))
            if mark:
                tally[f"outside_{mark}"] += 1
            else:
                todo.append({key: record[key] for key in ("id", "repo", "language", "commit", "parent", "split", "file",
                                                          "symbol")} | row)
    return tallies, causes, spread(todo)


def round_robin(groups):
    return [row for batch in itertools.zip_longest(*groups) for row in batch if row is not None]


def spread(rows):
    """The rows taken in turn from each language, and within a language in turn from each case, so a
    capped sample reaches every language and case."""
    by_language = collections.defaultdict(lambda: collections.defaultdict(list))
    for row in rows:
        by_language[row["language"]][row["id"]].append(row)
    return round_robin([round_robin(list(cases.values())) for _, cases in sorted(by_language.items())])


def render_holdback(tallies, causes, split, version):
    lines = [f"Holdback set, {split} half, {version}", "",
             "| Language | Cases | Errors | Truth sites | Listed | Neither | Catch share | Unresolved | "
             "No shape | Outside rows | Outside judged | Outside true share |",
             "|---|---|---|---|---|---|---|---|---|---|---|---|"]
    verdicts_ok = catch_ok = True
    for language, t in sorted(tallies.items()):
        judged = t["outside_true"] + t["outside_false"]
        catch = share(t["listed"], t["truth"])
        precision = share(t["outside_true"], judged)
        if t["truth"] >= MIN_TRUTH_SITES and catch < CATCH_MARK:
            catch_ok = False
        if judged and precision < OUTSIDE_PRECISION_MARK:
            verdicts_ok = False
        lines.append(f"| {language} | {t['cases']} | {t['errors']} | {t['truth']} | {t['listed']} | {t['neither']} | "
                     f"{catch:.1%} | {t['unresolved']} | {t['no_shape']} | {t['outside']} | {judged} | "
                     f"{precision:.1%} |")
    lines += ["", "Misses by cause: " + (", ".join(f"{cause} {causes[cause]}" for cause in MISS_CAUSES if causes[cause])
                                         or "none"), ""]
    if not tallies:
        return "\n".join(lines + ["Result: NO DATA"]) + "\n"
    errors = sum(t["errors"] for t in tallies.values())
    result = "INCOMPLETE (errors)" if errors else ("PASS" if catch_ok and verdicts_ok else "FAIL")
    lines.append(f"Result: {result} (marks: catch share >= {CATCH_MARK:.0%} in each language with at least "
                 f"{MIN_TRUTH_SITES} truth sites; outside rows true >= {OUTSIDE_PRECISION_MARK:.0%} of the judged)")
    return "\n".join(lines) + "\n"


def cmd_score_holdback(args):
    cases = wanted_cases(load_json(args.cases)["cases"], args.split)
    records = [r for r in load_run(args.run, "holdback") if r["split"] == args.split]
    meta = Path(args.run) / "run.json"
    version = load_json(meta)["version"] if meta.exists() else "unknown binary"
    tallies, causes, todo = score_holdback(records, cases)
    print(render_holdback(tallies, causes, args.split, version), end="")
    if args.todo:
        write_json(args.todo, todo[:OUTSIDE_JUDGED_CAP])
        print(f"{min(len(todo), OUTSIDE_JUDGED_CAP)} of {len(todo)} unjudged outside rows written to {args.todo}")
    return 0


def cmd_verdicts_holdback(args):
    data = load_json(args.cases)
    by_id = {case["id"]: case for case in data["cases"]}
    added = 0
    for verdict in load_json(getattr(args, "from")):
        if verdict.get("verdict") not in ("true", "false"):
            raise ValueError(f"verdict must be true or false: {verdict}")
        case = by_id[verdict["id"]]
        key = (verdict["path"], int(verdict["line"]))
        kept = [v for v in case["verdicts"] if (v["path"], int(v["line"])) != key]
        kept.append({k: verdict[k] for k in ("path", "line", "label", "verdict", "evidence") if k in verdict})
        case["verdicts"] = sorted(kept, key=lambda v: (v["path"], int(v["line"])))
        added += 1
    write_json(args.cases, data)
    print(f"{added} verdicts merged into {args.cases}")
    return 0


SESSION_GROUPS = ("missed_A1", "missed_A2", "missed_B", "brokeelsewhere_compile", "brokeelsewhere_test",
                  "brokeelsewhere_other")
VERIFIED_GROUP = "verified"
VERIFIED_MARK = 8 / 9
CANDIDATE_MARK = 0.70
LINE_SLACK = 2
CANDIDATE_COMMITS = 30
WORKTREE_LAYOUT = re.compile(r"^(?P<main>.+?)/(?:\.claude/worktrees|\.worktrees|worktrees)/(?P<name>[^/]+)")
GIT_STATE = re.compile(r"\bgit\s+(?:-C\s+(?P<dir>\S+)\s+)?(?:checkout(?!\s+-b\b)|switch(?!\s+-c\b)|reset|"
                       r"stash(?!\s+(?:list|show)\b)|rebase|merge(?!-)|pull|restore(?!\s+--staged\b)|cherry-pick|revert|am)\b")
HEREDOC_HEAD = re.compile(r"\bcat\s*(?:(?P<op>>>?)\s*(?P<target>[^\s;|&()<>]+)\s*<<(?P<tabs>-?)\s*(?P<q>['\"]?)(?P<mark>\w+)(?P=q)"
                          r"|<<(?P<tabs2>-?)\s*(?P<q2>['\"]?)(?P<mark2>\w+)(?P=q2)\s*(?P<op2>>>?)\s*(?P<target2>[^\s;|&()<>]+))")
IN_PLACE = re.compile(r"\bsed\s+(?:-\S+\s+)*-i|\bperl\s+-\w*i|write_text\(|open\([^)]*['\"][wa]['\"]")
REDIRECT = re.compile(r"(?:\btee\s+(?:-a\s+)?|(?<![0-9&<>=-])>>?\s*)(?P<target>[^\s;|&()<>]+)")
CODE_FILE = re.compile(r"[\w./~-]*\.(?:rs|py|pyi|ts|tsx|js|jsx|mjs|cjs|cs|razor|go|java|kt|kts|rb|swift|c|cc|cpp|h|hpp|"
                       r"scala|dart|vb|fs)\b")
LOCATION_PATTERNS = [re.compile(p, re.M) for p in (
    r"-->\s+(?P<path>[^\s:]+):(?P<line>\d+):\d+",
    r"(?P<path>[^\s(]+\.(?:tsx?|cs|razor))\((?P<line>\d+),\d+\): error",
    r"(?P<path>[^\s:]+\.tsx?):(?P<line>\d+):\d+ - error",
    r'File "(?P<path>[^"]+\.py)", line (?P<line>\d+)',
    r"^(?P<path>[^\s:]+\.py):(?P<line>\d+): ",
    r"^\s*(?P<path>[^\s:]+\.go):(?P<line>\d+):\d+:",
    r"\((?P<path>[^\s()]+\.(?:m?[jt]sx?|cjs)):(?P<line>\d+):\d+\)",
    r"^\s*at (?P<path>[^\s:()]+\.(?:mjs|cjs|js|ts)):(?P<line>\d+):\d+",
    r"(?P<path>[^\s:\[]+\.java):\[?(?P<line>\d+)[,:\]]",
)]
LOCATION_SKIP = ("/rustc/", ".cargo/registry", "node_modules", "site-packages", "/usr/lib", "/usr/share", "node:",
                 "<frozen", "dist-packages")


def session_split(log_path):
    digest = hashlib.sha256(str(log_path).encode()).hexdigest()
    return split_of(digest)


def harness_of(log_path):
    """Codex logs open with a `session_meta` entry; Claude Code logs do not."""
    try:
        with open(log_path, errors="replace") as log:
            first = json.loads(log.readline() or "{}")
    except (OSError, json.JSONDecodeError):
        return "unknown"
    return "codex" if first.get("type") == "session_meta" else "claude"


def read_errclass(path):
    rows = []
    for line in Path(path).read_text().splitlines():
        if line.startswith("("):
            log, fail_line, codes, files, _ = ast.literal_eval(line)
            rows.append({"log": log, "fail_line": fail_line, "codes": codes, "files": files})
    return rows


def cmd_select_sessions(args):
    examples = load_json(args.examples)
    cases = {}
    for row in read_errclass(args.errclass):
        cases[(row["log"], row["fail_line"])] = {"group": VERIFIED_GROUP, "log": row["log"],
                                                 "fail_line": row["fail_line"], "fix_files": row["files"]}
    for group in SESSION_GROUPS:
        for example in examples.get(group, []):
            key = (example["file"], example["fail_line"])
            if key in cases:
                continue
            fix_files = example.get("fix_files") or ([example["fix_file"]] if example.get("fix_file") else [])
            cases[key] = {"group": group, "log": example["file"], "fail_line": example["fail_line"],
                          "fix_files": fix_files}
    rows = []
    for number, case in enumerate(sorted(cases.values(), key=lambda c: (c["group"], c["log"], c["fail_line"]))):
        rows.append(case | {"id": f"s{number:03d}", "split": session_split(case["log"]),
                            "harness": harness_of(case["log"])})
    out = Path(args.out)
    write_json(out / "cases.json", {"selected": datetime.date.today().isoformat(), "cases": rows})
    counts = collections.Counter(case["group"] for case in rows)
    print(f"{out / 'cases.json'}: {len(rows)} cases; " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    return 0


def tool_result_text(content):
    if isinstance(content, list):
        return "\n".join(part.get("text", "") for part in content if isinstance(part, dict))
    return content or ""


def claude_edit(name, payload):
    if name == "Edit":
        return {"op": "replace", "path": payload["file_path"],
                "edits": [(payload["old_string"], payload["new_string"], bool(payload.get("replace_all")))]}
    if name == "MultiEdit":
        return {"op": "replace", "path": payload["file_path"],
                "edits": [(e["old_string"], e["new_string"], bool(e.get("replace_all"))) for e in payload["edits"]]}
    if name == "Write":
        return {"op": "write", "path": payload["file_path"], "content": payload["content"]}
    return None


def parse_claude_log(lines, fail_line):
    """Start facts, edits that succeeded before `fail_line`, shell commands, and the failing output: the
    result at `fail_line`, or the result of the tool call that `fail_line` makes."""
    session = {"cwd": None, "branch": None, "started": None, "commit": None, "edits": [], "commands": [],
               "fail_text": ""}
    pending = {}
    failing = set()
    for number, line in enumerate(lines, 1):
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if session["cwd"] is None and entry.get("cwd"):
            session["cwd"], session["branch"] = entry["cwd"], entry.get("gitBranch")
            session["started"] = entry.get("timestamp")
        message = entry.get("message") or {}
        content = message.get("content")
        if entry.get("type") == "assistant" and isinstance(content, list):
            for part in content:
                if isinstance(part, dict) and part.get("type") == "tool_use":
                    if number == fail_line:
                        failing.add(part.get("id"))
                    edit = claude_edit(part.get("name"), part.get("input") or {})
                    if edit:
                        pending[part.get("id")] = edit
                    elif part.get("name") == "Bash":
                        pending[part.get("id")] = {"op": "command", "command": (part.get("input") or {}).get("command", "")}
        elif entry.get("type") == "user" and isinstance(content, list):
            for part in content:
                if not (isinstance(part, dict) and part.get("type") == "tool_result"):
                    continue
                if number == fail_line or part.get("tool_use_id") in failing:
                    session["fail_text"] += tool_result_text(part.get("content"))
                action = pending.pop(part.get("tool_use_id"), None)
                if action is None or number >= fail_line:
                    continue
                if action["op"] == "command":
                    command, writes = heredoc_writes(action["command"])
                    session["commands"].append(command)
                    if not part.get("is_error"):
                        session["edits"].extend(writes)
                elif not part.get("is_error"):
                    session["edits"].append(action)
    return session


def codex_edits(changes):
    edits = []
    for path, change in (changes or {}).items():
        kind = change.get("type")
        if kind == "add":
            edits.append({"op": "write", "path": path, "content": change.get("content", "")})
        elif kind == "delete":
            edits.append({"op": "delete", "path": path})
        elif kind == "update":
            edits.append({"op": "patch", "path": path, "diff": change.get("unified_diff", ""),
                          "move": change.get("move_path")})
    return edits


def parse_codex_log(lines, fail_line):
    session = {"cwd": None, "branch": None, "started": None, "commit": None, "edits": [], "commands": [],
               "fail_text": ""}
    for number, line in enumerate(lines, 1):
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        payload = entry.get("payload") or {}
        if entry.get("type") == "session_meta" and session["cwd"] is None:
            session["cwd"], session["started"] = payload.get("cwd"), payload.get("timestamp")
            session["commit"] = (payload.get("git") or {}).get("commit_hash")
            session["branch"] = (payload.get("git") or {}).get("branch")
            continue
        if entry.get("type") != "event_msg" or payload.get("type") != "item_completed":
            continue
        item = payload.get("item") or {}
        if item.get("type") == "CommandExecution":
            if number == fail_line:
                session["fail_text"] += item.get("aggregated_output") or ""
            elif number < fail_line:
                command, writes = heredoc_writes((item.get("command") or [""])[-1])
                session["commands"].append(command)
                if item.get("exit_code") in (0, None):
                    session["edits"].extend(writes)
        elif item.get("type") == "FileChange" and number < fail_line and item.get("status") == "completed":
            session["edits"].extend(codex_edits(item.get("changes")))
    return session


def apply_hunks(text, diff):
    """Applies a unified diff's hunks by content, nearest to each hunk's stated line; None when one does not fit."""
    lines = text.split("\n")
    hunks = []
    for row in diff.splitlines():
        if row.startswith("@@"):
            match = re.match(r"@@ -(\d+)", row)
            hunks.append({"start": int(match[1]) if match else 1, "old": [], "new": []})
        elif hunks and row[:1] in (" ", "-", "+"):
            if row[0] in " -":
                hunks[-1]["old"].append(row[1:])
            if row[0] in " +":
                hunks[-1]["new"].append(row[1:])
        elif hunks and row == "":
            hunks[-1]["old"].append("")
            hunks[-1]["new"].append("")
    offset = 0
    for hunk in hunks:
        old = hunk["old"]
        if not old:
            at = min(max(hunk["start"] + offset, 0), len(lines))
            lines[at:at] = hunk["new"]
            offset += len(hunk["new"])
            continue
        expected = max(hunk["start"] - 1 + offset, 0)
        spots = [i for i in range(len(lines) - len(old) + 1) if lines[i:i + len(old)] == old]
        if not spots:
            return None
        spot = min(spots, key=lambda i: abs(i - expected))
        lines[spot:spot + len(old)] = hunk["new"]
        offset = spot + len(hunk["new"]) - (hunk["start"] - 1 + len(old))
    return "\n".join(lines)


def apply_edit(text, edit):
    """The file text after one edit, or None when the edit does not fit the text."""
    if edit["op"] == "write":
        return edit["content"]
    if edit["op"] == "append":
        return (text or "") + edit["content"]
    if edit["op"] == "delete":
        return "" if text is not None else None
    if text is None:
        return None
    if edit["op"] == "patch":
        return apply_hunks(text, edit["diff"])
    for old, new, every in edit["edits"]:
        if not old or old not in text:
            return None
        text = text.replace(old, new) if every else text.replace(old, new, 1)
    return text


def session_roots(cwd):
    """(session root, main repository) for a session folder; the main repository is None when it is gone."""
    if not cwd:
        return None, None
    if Path(cwd).is_dir():
        done = subprocess.run(["git", "-C", cwd, "rev-parse", "--show-toplevel", "--path-format=absolute",
                               "--git-common-dir"], capture_output=True, text=True, check=False)
        if done.returncode == 0:
            top, common = done.stdout.split("\n")[:2]
            return top, str(Path(common).parent) if common.endswith("/.git") else None
    match = WORKTREE_LAYOUT.match(cwd)
    root = match.group(0) if match else cwd
    main = match["main"] if match else cwd
    return root, main if (Path(main) / ".git").is_dir() else None


def relative_to_root(path, roots, cwd):
    full = path if path.startswith("/") else str(Path(cwd or "") / path)
    for root in roots:
        if root and full.startswith(root.rstrip("/") + "/"):
            return full[len(root.rstrip("/")) + 1:]
    return None


def replay_edits(clone, commit, edits, roots, cwd):
    """The changed files after the session's edits on `commit`, or None when one edit does not fit."""
    overlay = {}
    for edit in edits:
        rel = relative_to_root(edit["path"], roots, cwd)
        if rel is None:
            continue
        if rel not in overlay:
            done = subprocess.run(["git", "-C", str(clone), "show", f"{commit}:{rel}"], capture_output=True,
                                  text=True, errors="replace", check=False)
            overlay[rel] = done.stdout if done.returncode == 0 else None
        text = apply_edit(overlay[rel], edit)
        if text is None:
            return None
        if edit["op"] == "delete":
            overlay[rel] = None
            continue
        target = relative_to_root(edit["move"], roots, cwd) if edit.get("move") else None
        if target:
            overlay[rel], overlay[target] = None, text
        else:
            overlay[rel] = text
    return overlay


def edit_repository(session):
    """(root, main repository) of the checkout that holds most of the session's edits, or (None, None).
    A session can start in one checkout and edit a worktree inside it, or another repository; its
    recorded commit then names the wrong tree."""
    counts = collections.Counter()
    folders = {}
    for edit in session["edits"]:
        full = os.path.normpath(os.path.join(session["cwd"] or "", edit["path"]))
        match = WORKTREE_LAYOUT.match(full)
        if match and (Path(match["main"]) / ".git").is_dir():
            counts[(match.group(0), match["main"])] += 1
            continue
        folder = Path(full).parent
        while folder != folder.parent and not folder.is_dir():
            folder = folder.parent
        if folder not in folders:
            folders[folder] = session_roots(str(folder)) if folder != folder.parent else (None, None)
        if folders[folder][1]:
            counts[folders[folder]] += 1
    return counts.most_common(1)[0][0] if counts else (None, None)


def start_candidates(clone, session):
    """Start commits to try: the recorded commit, then the newest commits before the session start on
    the branches named like its worktree, on its branch, and on any branch."""
    seen = []
    if session.get("commit"):
        seen.append(session["commit"])
    before = [f"--before={session['started']}"] if session.get("started") else []
    named = []
    if session.get("worktree"):
        refs = git(clone, "for-each-ref", "--format=%(refname:short)", "refs/remotes/origin").split()
        named = [ref.removeprefix("origin/") for ref in refs if ref.rsplit("/", 1)[-1] == session["worktree"]]
    for refs in named + ([session["branch"]] if session.get("branch") else []) + ["--all"]:
        done = subprocess.run(["git", "-C", str(clone), "rev-list", "-n", str(CANDIDATE_COMMITS), *before,
                               *(["--all"] if refs == "--all" else [f"origin/{refs}", "--"])],
                              capture_output=True, text=True, check=False)
        seen.extend(sha for sha in done.stdout.split() if sha not in seen)
    return seen[:CANDIDATE_COMMITS * (len(named) + 1) + 1]


def failing_locations(text):
    found = []
    for pattern in LOCATION_PATTERNS:
        for match in pattern.finditer(text or ""):
            path = match["path"]
            if not any(skip in path for skip in LOCATION_SKIP) and (path, int(match["line"])) not in found:
                found.append((path, int(match["line"])))
    return found


def map_reported_path(path, roots, cwd, tracked):
    rel = relative_to_root(path, roots, cwd) if path.startswith("/") else path.lstrip("./")
    if rel in tracked:
        return rel
    tail = "/" + (rel or path).lstrip("/")
    matches = [file for file in tracked if ("/" + file).endswith(tail)]
    return matches[0] if len(matches) == 1 else None


def in_tree(path, root, cwd):
    """True when a shell path names a place in the session tree: a relative path, or an absolute one
    under the root. A path through a variable is taken as outside (a temporary file)."""
    path = path.strip("'\"")
    if path.startswith("$"):
        return False
    full = os.path.normpath(os.path.join(cwd or "", os.path.expanduser(path)))
    return not path.startswith(("/", "~")) or bool(root) and (full + "/").startswith(root.rstrip("/") + "/")


def same_tree(folder, root, cwd):
    """True when `git -C folder` acts on the session tree, not on another checkout or a removed worktree."""
    full = os.path.normpath(os.path.join(cwd or "", os.path.expanduser(folder.strip("'\""))))
    if root and full == os.path.normpath(root):
        return True
    if not os.path.isdir(full):
        return False
    done = subprocess.run(["git", "-C", full, "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=False)
    return done.returncode == 0 and os.path.normpath(done.stdout.strip()) == os.path.normpath(root or cwd or "")


def heredoc_writes(command):
    """(the command without its replayable `cat > file <<MARK` writes, the write and append edits they
    make). A heredoc whose marker is not quoted and whose body holds `$`, a backtick, or a backslash
    stays in the command, since the shell rewrites such a body."""
    edits, kept, position = [], [], 0
    for head in HEREDOC_HEAD.finditer(command):
        if head.start() < position:
            continue
        mark = head["mark"] or head["mark2"]
        start = command.find("\n", head.end())
        end = re.compile(r"^[ \t]*" + re.escape(mark) + r"[ \t]*$", re.M).search(command, start + 1) if start >= 0 else None
        if end is None:
            continue
        body = command[start + 1:end.start()]
        if not (head["q"] or head["q2"]) and re.search(r"[$`\\]", body):
            continue
        if head["tabs"] or head["tabs2"]:
            body = re.sub(r"^\t+", "", body, flags=re.M)
        edits.append({"op": "append" if ">>" in (head["op"] or head["op2"]) else "write",
                      "path": os.path.expanduser((head["target"] or head["target2"]).strip("'\"")), "content": body})
        kept.append(command[position:head.start()] + ":")
        position = end.end()
    return "".join(kept) + command[position:], edits


def session_blocker(session, root=None):
    """Why the session cannot be rebuilt from its own edits, or None: a git command that changed the
    session tree, or a shell command that wrote a code file of the tree. A formatter keeps every name
    and shape, so it does not block."""
    cwd = session.get("cwd")
    for command in session["commands"]:
        state = GIT_STATE.search(command)
        if state and (not state["dir"] or same_tree(state["dir"], root, cwd)):
            return "git state changed"
        written = [match["target"] for match in REDIRECT.finditer(command)]
        if IN_PLACE.search(command):
            written += CODE_FILE.findall(command)
        if any(CODE_FILE.fullmatch(path.strip("'\"")) and in_tree(path, root, cwd) for path in written):
            return "shell edit"
    return None


def shared_clone(work, main):
    clone = Path(work) / "clones" / (Path(main).name + "-" + hashlib.sha256(main.encode()).hexdigest()[:8])
    if not (clone / ".git").exists():
        clone.parent.mkdir(parents=True, exist_ok=True)
        git(clone.parent, "clone", "-q", "--shared", "--no-checkout", main, clone.name)
    return clone


def run_session_case(binary, out, case):
    record = {key: case[key] for key in ("id", "group", "split", "harness")}
    lines = Path(case["log"]).read_text(errors="replace").splitlines() if Path(case["log"]).exists() else None
    if lines is None:
        return record | {"status": "not rebuilt", "cause": "log gone"}
    parse = parse_codex_log if case["harness"] == "codex" else parse_claude_log
    session = parse(lines, case["fail_line"])
    root, main = session_roots(session["cwd"])
    edit_root, edit_main = edit_repository(session)
    if edit_main and edit_root != root:
        root, main = edit_root, edit_main
        session = session | {"worktree": Path(root).name if root != main else None}
    if main is None:
        return record | {"status": "not rebuilt", "cause": "repository gone"}
    blocker = session_blocker(session, root)
    if blocker:
        return record | {"status": "not rebuilt", "cause": blocker}
    roots = [root, main]
    clone = shared_clone(out / "work", main)
    commit = overlay = None
    for candidate in start_candidates(clone, session):
        overlay = replay_edits(clone, candidate, session["edits"], roots, session["cwd"])
        if overlay is not None:
            commit = candidate
            break
    if commit is None:
        return record | {"status": "not rebuilt", "cause": "start commit not found or an edit does not apply"}
    tree = replay_tree(clone, out / "trees", clone.name)
    checkout(tree, commit)
    for rel, text in overlay.items():
        target = Path(tree) / rel
        if text is None:
            target.unlink(missing_ok=True)
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(text)
    record |= {"edits": len(session["edits"]), "files_edited": len(overlay)}
    record |= run_check(binary, tree, commit)
    if record["status"] != "ok":
        return record
    tracked = set(git(tree, "ls-files").splitlines())
    edited = set(overlay)
    truth = []
    for path, line in failing_locations(session["fail_text"]):
        rel = map_reported_path(path, roots, session["cwd"], tracked)
        if rel and rel not in edited:
            truth.append({"path": rel, "line": line})
    file_level = not truth
    if file_level:
        truth = [{"path": rel, "line": None} for rel in
                 filter(None, (map_reported_path(f, roots, session["cwd"], tracked) for f in case["fix_files"]))
                 if rel not in edited]
    if not truth:
        return record | {"status": "not rebuilt", "cause": "no reported site outside the edited files"}
    listed = [(site["path"], site["line"]) for change in record["changes"] for site, _ in listed_rows(change)]
    caught = [t for t in truth if any(p == t["path"] and (t["line"] is None or abs(n - t["line"]) <= LINE_SLACK)
                                      for p, n in listed)]
    record |= {"truth": len(truth), "file_level": file_level, "caught": bool(caught), "shape_seen": bool(record["changes"])}
    if caught:
        return record
    if not record["changes"]:
        record["cause"] = "cut by a limit" if record["cuts"] else "shape not seen"
        return record
    if file_level:
        record["cause"] = "fixed file not listed"
        return record
    record["cause"] = "site not indexed"
    for change in record["changes"]:
        name = re.split(r"\.|::", change["display"])[-1]
        status, _, answer = reach_gate.fetch_answer(binary, tree, {"name": name, "file": change["path"],
                                                                    "line": change["line"]})
        if status != "ok":
            record["cause"] = "other"
            continue
        for t in truth:
            if t["line"] is None:
                continue
            cause, _ = miss_cause(answer, [change], t["path"], t["line"])
            if cause != "site not indexed":
                record["cause"] = cause
                return record
    if record["cuts"]:
        record["cause"] = "cut by a limit"
    return record


def cmd_run_sessions(args):
    data = load_json(args.cases)
    binary = Path(args.code_kb).resolve()
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if not (out / "run.json").exists():
        write_json(out / "run.json", {"binary": str(binary), "version": binary_version(binary),
                                      "started": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")})
    sink_path = out / "sessions.jsonl"
    done = set()
    if sink_path.exists():
        done = {json.loads(line)["id"] for line in sink_path.read_text().splitlines() if line.strip()}
    for case in data["cases"]:
        if (args.split != "all" and case["split"] != args.split) or case["id"] in done:
            continue
        try:
            record = run_session_case(binary, out, case)
        except (RuntimeError, OSError, subprocess.TimeoutExpired) as error:
            record = {key: case[key] for key in ("id", "group", "split", "harness")} | {
                "status": "error", "error": str(error)[:300]}
        with sink_path.open("a") as sink:
            sink.write(json.dumps(record) + "\n")
        print(f"{case['id']} {case['group']} {record['status']} {record.get('cause', '')} "
              f"caught={record.get('caught')}", file=sys.stderr)
    return 0


def score_sessions(records):
    tallies = {}
    for record in records:
        tally = tallies.setdefault(record["group"], {"cases": 0, "rebuilt": 0, "caught": 0,
                                                     "not_rebuilt": collections.Counter(),
                                                     "missed": collections.Counter()})
        tally["cases"] += 1
        if record["status"] == "ok":
            tally["rebuilt"] += 1
            if record.get("caught"):
                tally["caught"] += 1
            else:
                tally["missed"][record.get("cause", "other")] += 1
        else:
            tally["not_rebuilt"][record.get("cause") or record.get("error", "error")[:60]] += 1
    return tallies


def render_sessions(tallies, split, version):
    lines = [f"Session set, {split} half, {version}", "",
             "| Group | Cases | Rebuilt | Caught | Caught share | Not rebuilt by cause | Missed by cause |",
             "|---|---|---|---|---|---|---|"]
    for group, t in sorted(tallies.items()):
        lines.append(f"| {group} | {t['cases']} | {t['rebuilt']} | {t['caught']} | {share(t['caught'], t['rebuilt']):.1%} | "
                     f"{', '.join(f'{k} {v}' for k, v in t['not_rebuilt'].most_common()) or '-'} | "
                     f"{', '.join(f'{k} {v}' for k, v in t['missed'].most_common()) or '-'} |")
    verified = tallies.get(VERIFIED_GROUP, {"rebuilt": 0, "caught": 0})
    candidates = [t for g, t in tallies.items() if g != VERIFIED_GROUP]
    rebuilt = sum(t["rebuilt"] for t in candidates)
    caught = sum(t["caught"] for t in candidates)
    verified_ok = verified["rebuilt"] == 0 or share(verified["caught"], verified["rebuilt"]) >= VERIFIED_MARK
    candidates_ok = rebuilt == 0 or share(caught, rebuilt) >= CANDIDATE_MARK
    lines += ["", f"Verified: {verified['caught']} of {verified['rebuilt']} rebuilt caught. "
                  f"Candidates: {caught} of {rebuilt} rebuilt caught ({share(caught, rebuilt):.1%}).", ""]
    result = "NO DATA" if not tallies else ("PASS" if verified_ok and candidates_ok else "FAIL")
    lines.append(f"Result: {result} (marks: verified caught >= 8 of 9 of the rebuilt; candidates caught >= "
                 f"{CANDIDATE_MARK:.0%} of the rebuilt)")
    return "\n".join(lines) + "\n"


def cmd_score_sessions(args):
    records = [r for r in load_run_file(Path(args.run) / "sessions.jsonl") if r["split"] == args.split]
    meta = Path(args.run) / "run.json"
    version = load_json(meta)["version"] if meta.exists() else "unknown binary"
    print(render_sessions(score_sessions(records), args.split, version), end="")
    return 0


def load_run_file(path):
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    clean_cases = str(CASES_DIR / "clean.json")

    select = commands.add_parser("select", help="pick the cases of a set")
    select_sets = select.add_subparsers(dest="set", required=True)
    select_clean = select_sets.add_parser("clean")
    select_clean.add_argument("--repos", default=str(CORPUS_PATH), help="the reach corpus file")
    select_clean.add_argument("--repos-dir", default=str(REPOS_DIR), help="full-history clones")
    select_clean.add_argument("--work", default=str(WORK_DIR / "select"))
    select_clean.add_argument("--code-kb", default=str(REPO_ROOT / "target/release/code-kb"))
    select_clean.add_argument("--per-repo", type=int, default=15)
    select_clean.add_argument("--max-walk", type=int, default=400)
    select_clean.add_argument("--out", default=clean_cases)

    run = commands.add_parser("run", help="run a code-kb binary over the cases of a set")
    run_sets = run.add_subparsers(dest="set", required=True)
    run_clean = run_sets.add_parser("clean")
    run_clean.add_argument("--cases", default=clean_cases)
    run_clean.add_argument("--repos", default=str(CORPUS_PATH), help="the reach corpus file")
    run_clean.add_argument("--repos-dir", default=str(REPOS_DIR))
    run_clean.add_argument("--code-kb", required=True)
    run_clean.add_argument("--out", required=True)
    run_clean.add_argument("--split", choices=["dev", "holdout", "all"], default="dev")
    run_clean.add_argument("--only", help="comma-separated repo names")

    score = commands.add_parser("score", help="score a run against the verdicts")
    score_sets = score.add_subparsers(dest="set", required=True)
    score_clean_parser = score_sets.add_parser("clean")
    score_clean_parser.add_argument("--cases", default=clean_cases)
    score_clean_parser.add_argument("--run", required=True)
    score_clean_parser.add_argument("--split", choices=["dev", "holdout"], default="dev")
    score_clean_parser.add_argument("--todo", help="write the unjudged listed rows, grouped by change")

    verdicts = commands.add_parser("verdicts", help="merge hand-check verdicts into a case file")
    verdict_sets = verdicts.add_subparsers(dest="set", required=True)
    verdicts_clean = verdict_sets.add_parser("clean")
    verdicts_clean.add_argument("--cases", default=clean_cases)
    verdicts_clean.add_argument("--from", required=True)

    holdback_cases_path = str(CASES_DIR / "holdback.json")
    select_holdback = select_sets.add_parser("holdback")
    select_holdback.add_argument("--repos", default=str(CORPUS_PATH), help="the reach corpus file")
    select_holdback.add_argument("--repos-dir", default=str(REPOS_DIR))
    select_holdback.add_argument("--per-repo", type=int, default=40)
    select_holdback.add_argument("--per-symbol", type=int, default=3)
    select_holdback.add_argument("--per-commit", type=int, default=3)
    select_holdback.add_argument("--max-walk", type=int, default=3000)
    select_holdback.add_argument("--out", default=holdback_cases_path)
    run_holdback = run_sets.add_parser("holdback")
    run_holdback.add_argument("--cases", default=holdback_cases_path)
    run_holdback.add_argument("--repos", default=str(CORPUS_PATH), help="the reach corpus file")
    run_holdback.add_argument("--repos-dir", default=str(REPOS_DIR))
    run_holdback.add_argument("--code-kb", required=True)
    run_holdback.add_argument("--out", required=True)
    run_holdback.add_argument("--split", choices=["dev", "holdout", "all"], default="dev")
    run_holdback.add_argument("--only", help="comma-separated repo names")
    score_holdback_parser = score_sets.add_parser("holdback")
    score_holdback_parser.add_argument("--cases", default=holdback_cases_path)
    score_holdback_parser.add_argument("--run", required=True)
    score_holdback_parser.add_argument("--split", choices=["dev", "holdout"], default="dev")
    score_holdback_parser.add_argument("--todo", help=f"write up to {OUTSIDE_JUDGED_CAP} unjudged outside rows")
    verdicts_holdback = verdict_sets.add_parser("holdback")
    verdicts_holdback.add_argument("--cases", default=holdback_cases_path)
    verdicts_holdback.add_argument("--from", required=True)

    select_sessions = select_sets.add_parser("sessions")
    select_sessions.add_argument("--examples", required=True)
    select_sessions.add_argument("--errclass", required=True)
    select_sessions.add_argument("--out", required=True, help="a folder outside the repository")
    run_sessions = run_sets.add_parser("sessions")
    run_sessions.add_argument("--cases", required=True)
    run_sessions.add_argument("--code-kb", required=True)
    run_sessions.add_argument("--out", required=True, help="a folder outside the repository")
    run_sessions.add_argument("--split", choices=["dev", "holdout", "all"], default="dev")
    score_sessions_parser = score_sets.add_parser("sessions")
    score_sessions_parser.add_argument("--run", required=True)
    score_sessions_parser.add_argument("--split", choices=["dev", "holdout"], default="dev")
    return parser


HANDLERS = {
    ("select", "clean"): cmd_select_clean,
    ("run", "clean"): cmd_run_clean,
    ("score", "clean"): cmd_score_clean,
    ("verdicts", "clean"): cmd_verdicts_clean,
    ("select", "holdback"): cmd_select_holdback,
    ("run", "holdback"): cmd_run_holdback,
    ("score", "holdback"): cmd_score_holdback,
    ("verdicts", "holdback"): cmd_verdicts_holdback,
    ("select", "sessions"): cmd_select_sessions,
    ("run", "sessions"): cmd_run_sessions,
    ("score", "sessions"): cmd_score_sessions,
}


def main(argv=None):
    args = build_parser().parse_args(argv)
    # The replay runs real Stop hooks, which would add rows to the owner's adoption telemetry.
    os.environ["CODE_KB_NO_TELEMETRY"] = "1"
    try:
        return HANDLERS[(args.command, args.set)](args)
    except (RuntimeError, ValueError, KeyError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
