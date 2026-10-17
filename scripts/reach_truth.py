#!/usr/bin/env python3
"""Build reach-gate truth and case samples from SCIP indexes.

A truth line is a reference occurrence of a symbol (overloads unioned), minus definitions and
import or `use` lines. SCIP document paths must be repo-relative; pass --path-prefix when the
indexer ran in a subfolder (for example `Src/Newtonsoft.Json/`).

Usage:
  scripts/reach_truth.py sample --repo flask --index flask.scip --count 30 --seed 20261017 [--out FILE]
  scripts/reach_truth.py truth  --repo flask --index flask.scip [--write]
  scripts/reach_truth.py truth  --repo flask --index flask.scip --symbol Config.from_mapping

`sample` prints a case skeleton (30 symbols, alternating dev and holdout, empty truth). `truth`
fills the truth of every `scip` and `scip+manual` symbol in evals/reach/cases/<repo>.json and
leaves adjudications and manual symbols alone; with --symbol it prints the lines of one name.
"""

import argparse
import hashlib
import json
import re
import sys
from collections import Counter, defaultdict
from functools import cache, lru_cache
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import reach_gate
import scip_decode

DEFAULT_COUNT = 30
DEFAULT_SEED = 20261017
MAX_REF_LINES = 250
MANY_REFS = 20
COMMON_NAME_DEFINITIONS = 3
MAX_STATEMENT_LINES = 200
STRATA = ("common-name", "overridden", "many-refs", "value-ref", "decorator", "plain")

SKIPPED_NAME = re.compile(r"^(__\w+__|<.*)$")
TEST_PATH = re.compile(
    r"(^|/)(tests?|__tests__|specs?|testing|autotests)/|(^|/)test_[^/]*$|_test\.[^/]+$"
    r"|\.(test|spec)\.[^/.]+$|Tests?\.(java|cs|kt)$"
)
GENERATED_PATH = re.compile(r"(^|/)(target|build|obj|dist|node_modules|vendor|generated[^/]*)/")
JS_SUFFIXES = {".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".vue"}
JVM_SUFFIXES = {".java", ".kt", ".kts", ".scala", ".groovy"}
PYTHON_IMPORT = re.compile(r"(import|from)\s")
JS_IMPORT = re.compile(r"import\b(?!\s*[(.])")
JS_EXPORT_FROM = re.compile(r"export\s+(type\s+)?[{*]")
FROM_QUOTE = re.compile(r"\bfrom\s*['\"]")
RUST_USE = re.compile(r"(pub(\([^)]*\))?\s+)?use\s")
CSHARP_USING = re.compile(r"using\s+(static\s+)?([\w.]+\s*=\s*)?[\w.<>,\s:]+;\s*$")
GO_IMPORT = re.compile(r"import\s+(\w+\s+)?\"")


def balanced_end(rows, start, opener, closer, continuation=None):
    depth = 0
    for index in range(start, min(len(rows), start + MAX_STATEMENT_LINES)):
        text = rows[index]
        depth += text.count(opener) - text.count(closer)
        continued = continuation is not None and text.rstrip().endswith(continuation)
        if depth <= 0 and not continued:
            return index
    return start


def line_end(rows, start, matches):
    for index in range(start, min(len(rows), start + MAX_STATEMENT_LINES)):
        if matches(rows[index]):
            return index
    return start


def import_statement_end(suffix, rows, start):
    """Returns the 0-based last line of the import statement that starts at `start`, else None."""
    line = rows[start].strip()
    if suffix == ".py":
        if PYTHON_IMPORT.match(line):
            return balanced_end(rows, start, "(", ")", continuation="\\")
    elif suffix in JS_SUFFIXES:
        if JS_IMPORT.match(line):
            return balanced_end(rows, start, "{", "}")
        if JS_EXPORT_FROM.match(line):
            end = balanced_end(rows, start, "{", "}")
            return end if FROM_QUOTE.search(rows[end]) else None
    elif suffix in JVM_SUFFIXES:
        return start if line.startswith("import ") else None
    elif suffix == ".cs":
        return start if CSHARP_USING.match(line) else None
    elif suffix == ".go":
        if line.startswith("import ("):
            return line_end(rows, start, lambda row: row.strip() == ")")
        return start if GO_IMPORT.match(line) else None
    elif suffix == ".rs" and RUST_USE.match(line):
        return line_end(rows, start, lambda row: row.split("//")[0].rstrip().endswith(";"))
    return None


def import_line_numbers(rel_path, text):
    """1-based numbers of the lines that belong to import, `use`, or `using` statements."""
    suffix = Path(rel_path).suffix.lower()
    rows = text.split("\n")
    marked = set()
    number = 0
    while number < len(rows):
        end = import_statement_end(suffix, rows, number)
        if end is not None:
            marked.update(range(number + 1, end + 2))
            number = end
        number += 1
    return marked


class RepoFiles:
    def __init__(self, root):
        self.root = Path(root)
        self.rows = {}
        self.imports = {}

    def lines(self, path):
        if path not in self.rows:
            try:
                self.rows[path] = (self.root / path).read_text(errors="replace").split("\n")
            except OSError:
                self.rows[path] = []
        return self.rows[path]

    def import_lines(self, path):
        if path not in self.imports:
            self.imports[path] = import_line_numbers(path, "\n".join(self.lines(path)))
        return self.imports[path]


@lru_cache(maxsize=8)
def repo_files(root):
    return RepoFiles(root)


@cache
def group_key(symbol):
    """Overloads share a key: the package name and the descriptor names without disambiguators."""
    parts = symbol.split(" ", 4)
    if len(parts) < 5:
        return None
    try:
        descriptors = tuple(scip_decode.descriptors(symbol))
    except ValueError:
        return None
    return (parts[2], descriptors) if descriptors else None


class Group:
    def __init__(self, key):
        self.key = key
        self.symbols = set()
        self.defs = []
        self.refs = []

    @property
    def name(self):
        return self.key[1][-1][0]

    @property
    def is_callable(self):
        return self.key[1][-1][1] == "method"

    @property
    def kind(self):
        owned = any(suffix in ("type", "typeparam") for _, suffix in self.key[1][:-1])
        return "method" if owned else "function"


class ScipIndex:
    def __init__(self, path, prefix=""):
        _, documents, _ = scip_decode.load(path)
        self.groups = {}
        self.implemented = set()
        self.has_relationships = False
        self.unparsed = 0
        for document in documents:
            if document["path"].startswith(".."):
                continue
            doc_path = prefix + document["path"]
            for info in document["symbols"]:
                for relation in info["relationships"]:
                    self.has_relationships = True
                    if relation["is_implementation"]:
                        self.implemented.add(relation["symbol"])
            for occurrence in document["occurrences"]:
                symbol = occurrence["symbol"]
                if symbol.startswith("local "):
                    continue
                key = group_key(symbol)
                if key is None:
                    self.unparsed += 1
                    continue
                group = self.groups.setdefault(key, Group(key))
                group.symbols.add(symbol)
                if occurrence["roles"] & 1:
                    group.defs.append((doc_path, occurrence["start_line"], occurrence["enc_start"], occurrence["enc_end"]))
                elif not occurrence["roles"] & 2:
                    group.refs.append(
                        (doc_path, occurrence["start_line"], occurrence["start_col"], occurrence["end_line"], occurrence["end_col"])
                    )
        self.by_name = defaultdict(list)
        for group in self.groups.values():
            self.by_name[group.name].append(group)

    def truth_lines(self, group, repo_dir):
        files = repo_files(str(repo_dir))
        lines = {(path, line) for path, line, *_ in group.refs if line not in files.import_lines(path)}
        return sorted(lines)

    def match_query(self, query):
        want = scip_decode.query_parts(query)
        exact, loose = [], []
        for group in self.groups.values():
            if not group.defs:
                continue
            names = [name for name, _ in group.key[1]]
            if names[-len(want):] == want:
                exact.append(group)
            elif names[-1] == want[-1] and scip_decode.is_subsequence(want[:-1], names[:-1]):
                loose.append(group)
        return exact or loose


def find_group(index, name, file, line):
    """The group with a definition of `name` in `file` that contains or is nearest to `line`."""
    best, best_distance = None, None
    for group in index.by_name.get(name, []):
        for path, start, enclosing_start, enclosing_end in group.defs:
            if path != file:
                continue
            inside = enclosing_start and enclosing_start <= line <= enclosing_end
            distance = 0 if inside else abs(start - line)
            if best is None or distance < best_distance:
                best, best_distance = group, distance
    return best


def use_kinds(group, files):
    """Which reference shapes the group has: `decorator` (Python `@name`) and `value` (not followed by `(`)."""
    kinds = set()
    for path, line, column, end_line, end_column in group.refs:
        rows = files.lines(path)
        if line in files.import_lines(path) or not 0 < line <= len(rows) or not 0 < end_line <= len(rows):
            continue
        # shortcut: SCIP columns are read as characters, so lines with wide characters can mis-tag; fix when a tag misfires
        before = rows[line - 1][:column]
        after = rows[end_line - 1][end_column:]
        if path.endswith(".py") and before.lstrip().startswith("@"):
            kinds.add("decorator")
        elif not after.lstrip().startswith("("):
            kinds.add("value")
    return kinds


def candidates(index, repo_dir):
    """Callable symbols with one defining file, a bounded number of reference lines, and their tags."""
    pool = [
        group for group in index.groups.values()
        if group.defs and group.is_callable and not SKIPPED_NAME.match(group.name)
    ]
    name_counts = Counter(group.name for group in pool)
    owners = defaultdict(set)
    for group in pool:
        if group.kind == "method":
            owners[group.name].add(group.key[1][:-1])
    files = repo_files(str(repo_dir))
    rows = []
    for group in pool:
        def_files = {path for path, *_ in group.defs}
        if len(def_files) != 1:
            continue
        file = next(iter(def_files))
        if TEST_PATH.search(file) or GENERATED_PATH.search(file):
            continue
        refs = index.truth_lines(group, repo_dir)
        if not refs or len(refs) > MAX_REF_LINES:
            continue
        line = min(start for _, start, *_ in group.defs)
        kinds = use_kinds(group, files)
        overridden = bool(group.symbols & index.implemented) or (
            not index.has_relationships and len(owners[group.name]) >= 2 and group.kind == "method"
        )
        tags = [
            tag for tag, present in (
                ("common-name", name_counts[group.name] >= COMMON_NAME_DEFINITIONS),
                ("overridden", overridden),
                ("many-refs", len(refs) >= MANY_REFS),
                ("value-ref", "value" in kinds),
                ("decorator", "decorator" in kinds),
            ) if present
        ]
        rows.append({
            "id": f"{group.name}@{file}:{line}",
            "name": group.name,
            "file": file,
            "line": line,
            "kind": group.kind,
            "tags": tags,
        })
    return sorted(rows, key=lambda row: row["id"])


def choose_split(in_stratum, overall):
    if in_stratum["dev"] != in_stratum["holdout"]:
        return "dev" if in_stratum["dev"] < in_stratum["holdout"] else "holdout"
    return "dev" if overall["dev"] <= overall["holdout"] else "holdout"


def sample(index, repo_dir, entry, count, seed):
    """Round-robin over the strata in hash order; each pick takes the split that keeps its stratum and the total even."""
    order = sorted(
        candidates(index, repo_dir),
        key=lambda row: hashlib.sha256(f"{seed}:{row['id']}".encode()).hexdigest(),
    )
    chosen, symbols = set(), []
    per_stratum = {stratum: Counter() for stratum in STRATA}
    overall = Counter()
    while len(symbols) < count:
        progressed = False
        for stratum in STRATA:
            if len(symbols) >= count:
                break
            row = next(
                (row for row in order if row["id"] not in chosen and stratum in (row["tags"] or ["plain"])),
                None,
            )
            if row is None:
                continue
            split = choose_split(per_stratum[stratum], overall)
            per_stratum[stratum][split] += 1
            overall[split] += 1
            chosen.add(row["id"])
            symbols.append({
                "id": row["id"],
                "split": split,
                "name": row["name"],
                "file": row["file"],
                "line": row["line"],
                "kind": row["kind"],
                "tags": row["tags"],
                "truth_source": "scip",
                "truth": [],
                "adjudicated": [],
            })
            progressed = True
        if not progressed:
            break
    return {"repo": entry["name"], "commit": entry["commit"], "language": entry["language"], "symbols": symbols}


def truth_rows(lines):
    return [{"path": path, "line": line} for path, line in lines]


def open_index(args):
    entry = reach_gate.corpus_entry(reach_gate.load_corpus(args.corpus), args.repo)
    root = reach_gate.prepared_root(entry, args.cache_dir)
    index = ScipIndex(args.index, args.path_prefix)
    if index.unparsed:
        print(f"warning: {index.unparsed} occurrences have symbols this reader cannot parse; they are skipped", file=sys.stderr)
    return entry, root, index


def cmd_sample(args):
    entry, root, index = open_index(args)
    found = sample(index, root, entry, args.count, args.seed)
    if len(found["symbols"]) < args.count:
        print(f"warning: only {len(found['symbols'])} of {args.count} symbols qualify", file=sys.stderr)
    text = json.dumps(found, indent=2) + "\n"
    if args.out:
        Path(args.out).write_text(text)
    else:
        print(text, end="")
    return 0


def cmd_truth(args):
    _, root, index = open_index(args)
    if args.symbol:
        groups = index.match_query(args.symbol)
        lines = sorted({line for group in groups for line in index.truth_lines(group, root)})
        print(json.dumps(truth_rows(lines), indent=2))
        return 0
    path = Path(args.cases_dir) / f"{args.repo}.json"
    case = json.loads(path.read_text())
    for symbol in case["symbols"]:
        if symbol["truth_source"] not in ("scip", "scip+manual"):
            continue
        group = find_group(index, symbol["name"], symbol["file"], symbol["line"])
        if group is None:
            print(f"warning: no SCIP definition for {symbol['id']}; its truth is empty", file=sys.stderr)
        symbol["truth"] = truth_rows(index.truth_lines(group, root)) if group else []
    text = json.dumps(case, indent=2) + "\n"
    if args.write:
        path.write_text(text)
    else:
        print(text, end="")
    return 0


def build_parser():
    common = argparse.ArgumentParser(add_help=False)
    common.add_argument("--repo", required=True)
    common.add_argument("--index", required=True, help="SCIP index file")
    common.add_argument("--path-prefix", default="", help="prepended to every SCIP document path")
    common.add_argument("--corpus", default=str(reach_gate.CORPUS_PATH))
    common.add_argument("--cache-dir", default=str(reach_gate.DEFAULT_CACHE))
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sampling = commands.add_parser("sample", parents=[common], help="pick a stratified case skeleton")
    sampling.add_argument("--count", type=int, default=DEFAULT_COUNT)
    sampling.add_argument("--seed", type=int, default=DEFAULT_SEED)
    sampling.add_argument("--out", help="write the skeleton here instead of stdout")
    truth = commands.add_parser("truth", parents=[common], help="fill case truth from the index")
    truth.add_argument("--symbol", help="print the truth lines of a dotted name instead of updating a case file")
    truth.add_argument("--cases-dir", default=str(reach_gate.CASES_DIR))
    truth.add_argument("--write", action="store_true", help="rewrite the case file in place")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    handler = {"sample": cmd_sample, "truth": cmd_truth}[args.command]
    try:
        return handler(args)
    except (RuntimeError, ValueError, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
