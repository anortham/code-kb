#!/usr/bin/env python3
"""Ground-truth gate for `code-kb refs` callers answers.

The gate pins a corpus of real projects, runs a code-kb binary over frozen, adjudicated cases
(evals/reach/cases/<repo>.json), and scores each label tier per language and split against the
truth lines. Only code-kb and the case files are needed; SCIP indexes are used offline by
reach_truth.py to build the cases.

Usage:
  scripts/reach_gate.py materialize
  scripts/reach_gate.py run [--binary PATH] [--repos flask,gson] [--out target/reach-gate]
  scripts/reach_gate.py disagreements --repo flask [--split dev]

`run` exits 0 on PASS, 1 on FAIL (the report is still written), 2 on a setup error.
"""

import argparse
import datetime
import json
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CORPUS_PATH = REPO_ROOT / "evals/reach/corpus.json"
CASES_DIR = REPO_ROOT / "evals/reach/cases"
DEFAULT_OUT = REPO_ROOT / "target/reach-gate"
DEFAULT_CACHE = Path.home() / ".cache/code-kb-reach"

LABELS = ("proven", "likely", "unresolved", "excluded")
MARKS = {"proven_precision": 0.98, "excluded_precision": 0.98, "accounted": 0.98, "recall_target": 0.90}
SCAN_TIMEOUT_SECONDS = 3600
CALL_TIMEOUT_SECONDS = 180
SOURCE_TEXT_LIMIT = 160


def load_corpus(path=CORPUS_PATH):
    return json.loads(Path(path).read_text())


def corpus_entry(corpus, name):
    for entry in corpus:
        if entry["name"] == name:
            return entry
    raise SystemExit(f"error: {name!r} is not in the corpus ({', '.join(e['name'] for e in corpus)})")


def repo_dir(entry, cache=DEFAULT_CACHE):
    return Path(cache) / f"{entry['name']}-{entry['commit'][:12]}"


def git(*args, cwd=None):
    done = subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise RuntimeError(f"git {' '.join(args)} failed: {done.stderr.strip()}")
    return done.stdout.strip()


def materialize_repo(entry, cache=DEFAULT_CACHE):
    """Returns (path, created); an existing copy at the pinned commit is reused untouched."""
    dest = repo_dir(entry, cache)
    commit = entry["commit"]
    if dest.exists():
        if git("rev-parse", "HEAD", cwd=dest) != commit:
            git("checkout", "--detach", commit, cwd=dest)
        return dest, False
    local = entry.get("local")
    source = local if local and Path(local).is_dir() else entry["url"]
    dest.parent.mkdir(parents=True, exist_ok=True)
    try:
        git("clone", "--no-checkout", source, str(dest))
        try:
            git("checkout", "--detach", commit, cwd=dest)
        except RuntimeError:
            git("fetch", entry["url"], commit, cwd=dest)
            git("checkout", "--detach", commit, cwd=dest)
    except RuntimeError:
        shutil.rmtree(dest, ignore_errors=True)
        raise
    return dest, True


def load_cases(cases_dir, name):
    path = Path(cases_dir) / f"{name}.json"
    return json.loads(path.read_text()) if path.exists() else None


def effective_truth(symbol):
    """Truth lines after adjudication: a false verdict removes a line, a true verdict adds one."""
    lines = {(row["path"], row["line"]) for row in symbol.get("truth", [])}
    for row in symbol.get("adjudicated", []):
        key = (row["path"], row["line"])
        if row["verdict"] == "true":
            lines.add(key)
        elif row["verdict"] == "false":
            lines.discard(key)
        else:
            raise ValueError(f"adjudication verdict must be true or false, got {row['verdict']!r}")
    return lines


def site_covers(site, path, line):
    start = site.get("start_line")
    if start is None or site["path"] != path:
        return False
    return start <= line <= (site.get("end_line") or start)


def parse_answer(data):
    """Normalizes `refs --json` callers output: a legacy array scores as all proven, no text lines."""
    if isinstance(data, list):
        return {"shape": "legacy", "sites": [dict(row, label="proven") for row in data], "text_only": set()}
    if isinstance(data, dict) and isinstance(data.get("sites"), list):
        for row in data["sites"]:
            if row.get("label") not in LABELS:
                raise ValueError(f"unknown site label {row.get('label')!r}")
        text_only = {(entry["path"], line) for entry in data.get("text_only", []) for line in entry["lines"]}
        return {"shape": "report", "sites": data["sites"], "text_only": text_only}
    raise ValueError("refs output is neither a site array nor a report object")


def within_scope(answer, scope):
    """Keeps the sites and text lines under the case file's `scope` path prefixes, when it has one."""
    if not scope:
        return answer
    inside = lambda path: any(path.startswith(prefix) for prefix in scope)
    return dict(
        answer,
        sites=[row for row in answer["sites"] if inside(row["path"])],
        text_only={key for key in answer["text_only"] if inside(key[0])},
    )


def score_symbol(truth, answer):
    counts = {"truth_lines": len(truth), "recalled_lines": 0, "accounted_lines": 0}
    for label in LABELS:
        counts[f"sites_{label}"] = 0
        counts[f"true_{label}"] = 0
    covered = set()
    recalled = set()
    for site in answer["sites"]:
        hits = {line for line in truth if site_covers(site, *line)}
        counts[f"sites_{site['label']}"] += 1
        counts[f"true_{site['label']}"] += bool(hits)
        covered |= hits
        if site["label"] in ("proven", "likely"):
            recalled |= hits
    counts["recalled_lines"] = len(recalled)
    counts["accounted_lines"] = len(covered | (truth & answer["text_only"]))
    return counts


def ratio(numerator, denominator):
    return numerator / denominator if denominator else None


def aggregate(counts_list):
    total = {"symbols": len(counts_list), "truth_lines": 0, "recalled_lines": 0, "accounted_lines": 0}
    for label in LABELS:
        total[f"sites_{label}"] = 0
        total[f"true_{label}"] = 0
    for counts in counts_list:
        for key in total:
            if key != "symbols":
                total[key] += counts[key]
    total["proven_precision"] = ratio(total["true_proven"], total["sites_proven"])
    total["excluded_precision"] = ratio(total["sites_excluded"] - total["true_excluded"], total["sites_excluded"])
    total["recall"] = ratio(total["recalled_lines"], total["truth_lines"])
    total["accounted"] = ratio(total["accounted_lines"], total["truth_lines"])
    return total


def decide(report):
    failures = []
    if report["not_scored"]:
        failures.append("not scored: " + ", ".join(report["not_scored"]))
    if not report["languages"]:
        failures.append("no symbols were scored")
    for language, tiers in report["languages"].items():
        metrics = tiers["all"]
        precision = metrics["proven_precision"]
        if precision is None:
            failures.append(f"{language}: proven precision has no proven sites")
        elif precision < MARKS["proven_precision"]:
            failures.append(f"{language}: proven precision {precision:.3f} is below {MARKS['proven_precision']}")
        excluded = metrics["excluded_precision"]
        if excluded is not None and excluded < MARKS["excluded_precision"]:
            failures.append(f"{language}: excluded precision {excluded:.3f} is below {MARKS['excluded_precision']}")
        accounted = metrics["accounted"]
        if accounted is None:
            failures.append(f"{language}: accounted share has no truth lines")
        elif accounted < MARKS["accounted"]:
            failures.append(f"{language}: accounted share {accounted:.3f} is below {MARKS['accounted']}")
    return failures


def build_report(results, scored, corpus_names, show_holdout=False):
    languages = {}
    for language in sorted({row["language"] for row in results}):
        rows = [row for row in results if row["language"] == language]
        languages[language] = {
            "all": aggregate([row["counts"] for row in rows]),
            **{
                split: aggregate([row["counts"] for row in rows if row["split"] == split])
                for split in ("dev", "holdout")
            },
        }
    shapes = sorted({row["shape"] for row in results if row.get("shape")})
    report = {
        "marks": MARKS,
        "shapes": shapes,
        "scored": sorted(scored),
        "not_scored": sorted(set(corpus_names) - set(scored)),
        "languages": languages,
        "overall": aggregate([row["counts"] for row in results]),
        "show_holdout": show_holdout,
        "symbols": [row for row in results if show_holdout or row["split"] != "holdout"],
    }
    report["failures"] = decide(report)
    report["result"] = "FAIL" if report["failures"] else "PASS"
    return report


def fmt(value):
    return "n/a" if value is None else f"{value:.3f}"


def render_markdown(report):
    marks = report["marks"]
    out = [
        "# Reach gate report",
        "",
        f"Commit: {report.get('commit', 'unknown')}",
        f"Binary: {report.get('binary', 'unknown')} ({report.get('version', 'unknown')})",
        f"Generated: {report.get('generated', 'unknown')}",
        f"Answer shape: {', '.join(report['shapes']) or 'none'}",
        f"Scored repos: {', '.join(report['scored']) or 'none'}",
        "",
        (
            f"Marks: proven precision >= {marks['proven_precision']}, excluded precision >= {marks['excluded_precision']}, "
            f"and accounted share >= {marks['accounted']} in every language. "
            f"Recall target (report only): proven+likely >= {marks['recall_target']}."
        ),
        "",
        "## Languages",
        "",
        "| Language | Split | Symbols | Truth lines | Proven sites | Proven precision | Excluded precision | Recall | Accounted |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    for language, tiers in report["languages"].items():
        for split in ("all", "dev", "holdout"):
            m = tiers[split]
            out.append(
                f"| {language} | {split} | {m['symbols']} | {m['truth_lines']} | {m['sites_proven']} | "
                f"{fmt(m['proven_precision'])} | {fmt(m['excluded_precision'])} | {fmt(m['recall'])} | {fmt(m['accounted'])} |"
            )
    overall = report["overall"]
    verdict = "met" if overall["recall"] is not None and overall["recall"] >= marks["recall_target"] else "not met"
    out += ["", f"Overall recall {fmt(overall['recall'])}: target {verdict}.", ""]
    out += [
        "## Label tiers",
        "",
        "Cells are sites that cover a truth line / all sites. A high excluded share is a wrong exclusion.",
        "",
        "| Language | Split | proven | likely | unresolved | excluded |",
        "|---|---|---|---|---|---|",
    ]
    for language, tiers in report["languages"].items():
        for split in ("all", "dev", "holdout"):
            m = tiers[split]
            cells = " | ".join(f"{m['true_' + label]}/{m['sites_' + label]}" for label in LABELS)
            out.append(f"| {language} | {split} | {cells} |")
    out += ["", "## Symbols", ""]
    if not report["show_holdout"]:
        out += ["Holdout symbols are not listed; their numbers are in the tables above.", ""]
    out += ["| Repo | Symbol | Split | Status | Truth lines | Proven | Proven true | Recalled | Accounted |", "|---|---|---|---|---|---|---|---|---|"]
    for row in report["symbols"]:
        c = row["counts"]
        out.append(
            f"| {row['repo']} | {row['id']} | {row['split']} | {row['status']} | {c['truth_lines']} | "
            f"{c['sites_proven']} | {c['true_proven']} | {c['recalled_lines']} | {c['accounted_lines']} |"
        )
    problems = [row for row in report["symbols"] if row["status"] != "ok"]
    if problems:
        out += ["", "## Symbols without an answer", ""]
        out += [f"- {row['id']}: {row['status']}: {row['detail']}" for row in problems]
    out += ["", "## Failures", ""]
    out += [f"- {failure}" for failure in report["failures"]] or ["None."]
    out += ["", f"## Result: {report['result']}"]
    return "\n".join(out) + "\n"


def run_binary(binary, root, *args, timeout=CALL_TIMEOUT_SECONDS, as_json=False):
    command = [str(binary), "--root", str(root), *(["--json"] if as_json else []), *args]
    done = subprocess.run(command, capture_output=True, text=True, timeout=timeout, check=False)
    if done.returncode != 0:
        raise RuntimeError(f"{' '.join(args[:2])} exited {done.returncode}: {done.stderr.strip()[:300]}")
    return json.loads(done.stdout) if as_json else done.stdout


def pick_symbol_id(rows, symbol):
    """Chooses the lookup row on the case line, else the nearest row with the case name in its file."""
    if isinstance(rows, dict):
        rows = rows.get("names", []) + rows.get("ranked", [])
    flat = [row["symbol"] if isinstance(row.get("symbol"), dict) else row for row in rows]
    here = [
        row for row in flat
        if row["path"] == symbol["file"] and row["name"] == symbol["name"] and row["kind"] != "import"
    ]
    if not here:
        return None
    best = min(here, key=lambda row: (abs(row["start_line"] - symbol["line"]), row["kind"] != symbol.get("kind")))
    return best["symbol_id"]


def fetch_answer(binary, root, symbol):
    """Returns (status, detail, answer) for one case symbol; a failure becomes an empty answer."""
    empty = parse_answer([])
    try:
        rows = run_binary(binary, root, "lookup", symbol["name"], "--path", symbol["file"], "--limit", "50", as_json=True)
        symbol_id = pick_symbol_id(rows, symbol)
        if symbol_id is None:
            return "not_found", f"lookup found no {symbol['name']} in {symbol['file']}", empty
        data = run_binary(binary, root, "refs", "--symbol-id", symbol_id, "--limit", "200", as_json=True)
        return "ok", "", parse_answer(data)
    except (RuntimeError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        return "error", str(error), empty


def disagreements(truth, answer):
    wrong = [
        site for site in answer["sites"]
        if site["label"] == "proven" and not any(site_covers(site, *line) for line in truth)
    ]
    wrongly_excluded = [
        site for site in answer["sites"]
        if site["label"] == "excluded" and any(site_covers(site, *line) for line in truth)
    ]
    missed = sorted(line for line in truth if not any(site_covers(site, *line) for site in answer["sites"]))
    return {
        "proven_without_truth": wrong,
        "excluded_with_truth": wrongly_excluded,
        "truth_without_site": [(path, line, (path, line) in answer["text_only"]) for path, line in missed],
    }


class SourceLines:
    def __init__(self, root):
        self.root = Path(root)
        self.cache = {}

    def text(self, path, line):
        if path not in self.cache:
            try:
                self.cache[path] = (self.root / path).read_text(errors="replace").split("\n")
            except OSError:
                self.cache[path] = []
        rows = self.cache[path]
        return rows[line - 1].strip()[:SOURCE_TEXT_LIMIT] if 0 < line <= len(rows) else ""


def default_binary():
    built = REPO_ROOT / "target/release/code-kb"
    return str(built) if built.exists() else shutil.which("code-kb") or "code-kb"


def binary_version(binary):
    try:
        done = subprocess.run([str(binary), "--version"], capture_output=True, text=True, timeout=30, check=False)
        return done.stdout.strip()
    except (OSError, subprocess.TimeoutExpired):
        return "unknown"


def head_commit():
    try:
        return git("rev-parse", "HEAD", cwd=REPO_ROOT)
    except (RuntimeError, OSError):
        return "unknown"


def prepared_root(entry, cache):
    root = repo_dir(entry, cache)
    if not root.is_dir():
        raise SystemExit(f"error: {root} is missing; run scripts/reach_gate.py materialize")
    return root


def cmd_materialize(args):
    corpus = load_corpus(args.corpus)
    wanted = args.repos.split(",") if args.repos else [entry["name"] for entry in corpus]
    for name in wanted:
        path, created = materialize_repo(corpus_entry(corpus, name), args.cache_dir)
        print(f"{name}: {'created' if created else 'reused'} {path}")
    return 0


def cmd_run(args):
    corpus = load_corpus(args.corpus)
    names = [entry["name"] for entry in corpus]
    wanted = args.repos.split(",") if args.repos else names
    for name in wanted:
        corpus_entry(corpus, name)
    results, scored = [], []
    for name in wanted:
        entry = corpus_entry(corpus, name)
        cases = load_cases(args.cases_dir, name)
        if cases is None:
            print(f"{name}: no case file in {args.cases_dir}; not scored", file=sys.stderr)
            continue
        root = prepared_root(entry, args.cache_dir)
        run_binary(args.binary, root, "scan", timeout=SCAN_TIMEOUT_SECONDS)
        scored.append(name)
        for number, symbol in enumerate(cases["symbols"], 1):
            status, detail, answer = fetch_answer(args.binary, root, symbol)
            answer = within_scope(answer, cases.get("scope"))
            results.append({
                "repo": name,
                "language": entry["language"],
                "split": symbol["split"],
                "id": symbol["id"],
                "name": symbol["name"],
                "file": symbol["file"],
                "line": symbol["line"],
                "tags": symbol.get("tags", []),
                "status": status,
                "detail": detail,
                "shape": answer["shape"] if status == "ok" else None,
                "counts": score_symbol(effective_truth(symbol), answer),
            })
            print(f"{name}: {number}/{len(cases['symbols'])} {symbol['id']} {status}", file=sys.stderr)
    report = build_report(results, scored, names, args.show_holdout)
    report["commit"] = head_commit()
    report["binary"] = str(args.binary)
    report["version"] = binary_version(args.binary)
    report["generated"] = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    (out / "report.md").write_text(render_markdown(report))
    print(f"Report: {out / 'report.md'} ({report['result']})")
    return 0 if report["result"] == "PASS" else 1


def adjudication_mark(verdicts, path, line):
    verdict = verdicts.get((path, line))
    return f" [adjudicated {verdict}]" if verdict else ""


def cmd_disagreements(args):
    corpus = load_corpus(args.corpus)
    entry = corpus_entry(corpus, args.repo)
    cases = load_cases(args.cases_dir, args.repo)
    if cases is None:
        raise SystemExit(f"error: no case file for {args.repo} in {args.cases_dir}")
    root = prepared_root(entry, args.cache_dir)
    run_binary(args.binary, root, "scan", timeout=SCAN_TIMEOUT_SECONDS)
    source = SourceLines(root)
    hidden = {"symbols": 0, "wrong": 0, "excluded": 0, "missed": 0}
    for symbol in cases["symbols"]:
        if args.split != "all" and symbol["split"] != args.split:
            continue
        status, detail, answer = fetch_answer(args.binary, root, symbol)
        answer = within_scope(answer, cases.get("scope"))
        found = disagreements(effective_truth(symbol), answer)
        if symbol["split"] == "holdout" and not args.show_holdout:
            hidden["symbols"] += 1
            hidden["wrong"] += len(found["proven_without_truth"])
            hidden["excluded"] += len(found["excluded_with_truth"])
            hidden["missed"] += len(found["truth_without_site"])
            continue
        print(f"{args.repo} {symbol['split']} {symbol['id']} (answer: {answer['shape']}, {status})")
        if detail:
            print(f"  {detail}")
        verdicts = {(row["path"], row["line"]): row["verdict"] for row in symbol.get("adjudicated", [])}
        for site in found["proven_without_truth"]:
            path, line = site["path"], site.get("start_line") or 0
            print(f"  proven, no truth line{adjudication_mark(verdicts, path, line)}: {path}:{line}  {source.text(path, line)}")
        for site in found["excluded_with_truth"]:
            path, line = site["path"], site.get("start_line") or 0
            print(
                f"  excluded, covers a truth line{adjudication_mark(verdicts, path, line)}: {path}:{line}  "
                f"({site.get('basis', '')})  {source.text(path, line)}"
            )
        for path, line, text_only in found["truth_without_site"]:
            note = (" [text only]" if text_only else "") + adjudication_mark(verdicts, path, line)
            print(f"  truth line, no site{note}: {path}:{line}  {source.text(path, line)}")
    if hidden["symbols"]:
        print(
            f"holdout: {hidden['symbols']} symbols, {hidden['wrong']} proven sites with no truth line, "
            f"{hidden['excluded']} excluded sites that cover a truth line, "
            f"{hidden['missed']} truth lines with no site (add --show-holdout for detail)"
        )
    return 0


def build_parser():
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--corpus", default=str(CORPUS_PATH))
    common.add_argument("--cache-dir", default=str(DEFAULT_CACHE))
    scoring = argparse.ArgumentParser(add_help=False, parents=[common])
    scoring.add_argument("--binary", default=default_binary(), help="code-kb binary to score")
    scoring.add_argument("--cases-dir", default=str(CASES_DIR))
    scoring.add_argument("--show-holdout", action="store_true", help="list holdout symbols in detail")
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    materialize = commands.add_parser("materialize", parents=[common], help="clone the corpus at its pinned commits")
    materialize.add_argument("--repos", help="comma-separated corpus names")
    run = commands.add_parser("run", parents=[scoring], help="score a binary and write the report")
    run.add_argument("--repos", help="comma-separated corpus names; a partial run cannot PASS")
    run.add_argument("--out", default=str(DEFAULT_OUT))
    diff = commands.add_parser("disagreements", parents=[scoring], help="show where a binary and the truth differ")
    diff.add_argument("--repo", required=True)
    diff.add_argument("--split", choices=["dev", "holdout", "all"], default="dev")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    handler = {"materialize": cmd_materialize, "run": cmd_run, "disagreements": cmd_disagreements}[args.command]
    try:
        return handler(args)
    except (RuntimeError, ValueError, subprocess.TimeoutExpired) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
